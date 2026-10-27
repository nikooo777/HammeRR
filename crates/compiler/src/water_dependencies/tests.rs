use std::fs;

use source_formats::gamefs::{GameFs, Mount, MountedPath};

use super::*;
use crate::asset_sources::SourceGuardPolicy;

fn policy() -> SourceGuardPolicy {
    SourceGuardPolicy {
        label: "Material inspection",
        missing_label: "Material inspection candidate",
        asset_bytes: MAX_TEXTURE_BYTES,
        physical_paths: MAX_PHYSICAL_PATHS,
    }
}

fn fixture() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("materials/dev")).unwrap();
    fs::create_dir_all(directory.path().join("materials/test")).unwrap();
    fs::write(
        directory.path().join("materials/dev/water_normal.vtf"),
        texture(false),
    )
    .unwrap();
    let game = GameFs {
        game_dir: directory.path().into(),
        base_dir: directory.path().into(),
        mounts: vec![MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().into()),
        }],
    };
    (directory, game)
}

fn texture(cube: bool) -> Vec<u8> {
    let mut bytes = vec![0; 80];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [
        (4, 7u32),
        (8, 2),
        (12, 80),
        (20, if cube { 0x4000 } else { 0 }),
        (52, 0),
        (57, u32::MAX),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [
        (16, 1u16),
        (18, 1),
        (24, 1),
        (26, if cube { u16::MAX } else { 0 }),
        (63, 1),
    ] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    bytes[56] = 1;
    bytes.extend(std::iter::repeat_n(127, if cube { 24 } else { 4 }));
    bytes
}

fn seed(capture: &mut MaterialInspectionCapture<'_>, root: &str) {
    let _ = vmt::Material::load_with_reader(root, &mut |path| {
        capture
            .read_material(path, MAX_MATERIAL_BYTES)
            .map(|asset| asset.map(|asset| asset.bytes().to_vec()))
    });
}

fn inspect(game: &GameFs, root: &str) -> WaterDependencyReport {
    let control = Control::default();
    let mut capture = MaterialInspectionCapture::new(game, &control, policy());
    seed(&mut capture, root);
    inspect_water_dependencies(root, &mut capture, &control)
}

#[test]
fn sdk_defaults_are_typed_without_runtime_file_lookups() {
    let (directory, game) = fixture();
    let root = "materials/test/root.vmt";
    let source = "Water { $underwateroverlay 0 $waterdepth 123 }";
    fs::write(directory.path().join(root), source).unwrap();
    let report = inspect(&game, root);
    assert_eq!(
        report.state,
        WaterDependencyState::SupportedStaticInventory,
        "{:?}",
        report.issues
    );
    assert_eq!(report.assets.len(), 2);
    assert_eq!(report.counters.material_reads, 1);
    assert_eq!(report.counters.texture_reads, 1);
    assert_eq!(
        report.assets[0].sha256.as_deref(),
        Some(hash(source.as_bytes()).as_str())
    );
    let edges: BTreeMap<_, _> = report
        .edges
        .iter()
        .map(|edge| (edge.key.as_str(), edge))
        .collect();
    assert_eq!(
        edges["$normalmap"].target.as_deref(),
        Some("materials/dev/water_normal.vtf")
    );
    assert_eq!(edges["$normalmap"].origin, DeclarationOrigin::SdkDefault);
    assert_eq!(edges["$reflecttexture"].state, EdgeState::RuntimeResource);
    assert_eq!(edges["$reflecttexture"].reference, "_rt_WaterReflection");
    assert_eq!(edges["$refracttexture"].state, EdgeState::RuntimeResource);
    assert_eq!(edges["$envmap"].state, EdgeState::RuntimeResource);
    assert!(edges["$envmap"].target.is_none());
    assert_eq!(edges["$underwateroverlay"].state, EdgeState::Disabled);
    assert_eq!(
        edges["$underwateroverlay"].evidence_basis,
        "installed_client_and_sdk"
    );
    assert!(
        !report
            .assets
            .iter()
            .any(|asset| asset.path.contains("_rt_") || asset.path.contains("env_cubemap"))
    );
    assert_eq!(
        report.counters.source_bytes,
        report.assets.iter().map(|asset| asset.bytes).sum::<usize>()
    );
}

#[test]
fn patch_effective_edges_use_original_includes_and_shared_assets_once() {
    let (directory, game) = fixture();
    for name in ["normal", "base", "reflect", "refract", "tool"] {
        fs::write(
            directory.path().join(format!("materials/test/{name}.vtf")),
            texture(false),
        )
        .unwrap();
    }
    fs::write(
        directory.path().join("materials/test/cube.vtf"),
        texture(true),
    )
    .unwrap();
    fs::write(directory.path().join("materials/test/base.vmt"), "Water { $normalmap test/normal $basetexture test/base $reflecttexture test/reflect $refracttexture test/refract $envmap test/cube %tooltexture test/tool $bottommaterial test/child $underwateroverlay test/child }").unwrap();
    fs::write(
        directory.path().join("materials/test/child.vmt"),
        "Water_DX90 { $normalmap test/normal }",
    )
    .unwrap();
    let root = "materials/test/root.vmt";
    fs::write(directory.path().join(root), "Patch { /* ordinary source comment */ include materials/test/base.vmt replace { $basetexture test/normal } }").unwrap();
    let report = inspect(&game, root);
    assert_eq!(
        report.state,
        WaterDependencyState::SupportedStaticInventory,
        "{:?}",
        report.issues
    );
    assert_eq!(report.counters.material_reads, 3);
    assert_eq!(report.counters.texture_reads, 5);
    let base = report
        .edges
        .iter()
        .find(|edge| edge.owner == root && edge.key == "$basetexture")
        .unwrap();
    assert_eq!(base.origin, DeclarationOrigin::PatchEffective);
    assert_eq!(base.target.as_deref(), Some("materials/test/normal.vtf"));
    assert!(
        report
            .edges
            .iter()
            .any(|edge| edge.role == DependencyRole::PatchInclude && edge.owner == root)
    );
    assert!(
        report
            .edges
            .iter()
            .any(|edge| edge.role == DependencyRole::CompilerMaterial
                && edge.target.as_deref() == Some("materials/test/child.vmt"))
    );
    assert!(
        report
            .edges
            .iter()
            .any(|edge| edge.role == DependencyRole::RuntimeMaterial
                && edge.target.as_deref() == Some("materials/test/child.vmt"))
    );
    let cube = report
        .assets
        .iter()
        .find(|asset| asset.path == "materials/test/cube.vtf")
        .unwrap()
        .texture
        .as_ref()
        .unwrap();
    assert_eq!(cube.faces, 6);
    assert_eq!(cube.authored_depth, Some(1));
    assert_eq!(cube.layout_scope, "existing_vtf_parser");
    assert!(
        report
            .assets
            .iter()
            .all(|asset| asset.origin.as_ref().unwrap().kind == "directory")
    );
}

#[test]
fn unsupported_raw_forms_and_patch_edits_cannot_hide_in_flattening() {
    for source in [
        "Water { $normalmap test/missing Proxies { AnimatedTexture {} } }",
        "Water { $normalmap dev/water_normal $NORMALMAP test/missing }",
        "Water { $normalmap#1 dev/water_normal }",
        "Water { $flowmap test/unknown }",
        "Water { $normalmap dev/water_normal [$WIN32] }",
        "Water { Water_DX9_HDR { $normalmap dev/water_normal } }",
        "Patch { include materials/test/base.vmt insert { $normalmap test/different } }",
        "Patch { include materials/test/base.vmt replace { $basetexture test/missing } }",
        "Patch { include materials/test/base.vmt insert { $abovewater 1 } replace { $abovewater 1 } }",
    ] {
        let (directory, game) = fixture();
        fs::write(
            directory.path().join("materials/test/base.vmt"),
            "Water { $normalmap dev/water_normal }",
        )
        .unwrap();
        fs::write(directory.path().join("materials/test/root.vmt"), source).unwrap();
        let report = inspect(&game, "materials/test/root.vmt");
        assert_eq!(
            report.state,
            WaterDependencyState::Unsupported,
            "{source}: {:?}",
            report.issues
        );
        assert!(!report.issues.is_empty());
    }
}

#[test]
fn embedded_comment_markers_do_not_rewrite_bare_material_tokens() {
    for source in [
        "Water/*x*/ { $normalmap dev/water_normal }",
        "Water { $normalmap/*x*/ dev/water_normal }",
        "Water { $normalmap dev/water_normal/*x*/ }",
        "Water//x { $normalmap dev/water_normal }",
        "Water { $normalmap//x dev/water_normal }",
        "Water { $normalmap dev//water_normal }",
        "Water { $normalmap dev//[selector] }",
        "Patch { include/*x*/ materials/test/base.vmt }",
        "Patch { include materials/test/base.vmt/*x*/ }",
        "Patch { include materials/test/base.vmt insert { $normalmap/*x*/ dev/water_normal } }",
    ] {
        let parsed = raw::parse(source.as_bytes());
        assert!(
            matches!(
                parsed,
                Err(Failure {
                    state: WaterDependencyState::Unsupported,
                    code: "embedded_comment_marker",
                    ..
                })
            ),
            "{source}"
        );
    }
    let source = "Water { $normalmap/*x*/ dev/water_normal }";
    let ordinary = vmt::Material::parse(source).unwrap();
    assert_eq!(ordinary.get("$normalmap/*x*/"), Some("dev/water_normal"));
    assert!(ordinary.get("$normalmap").is_none());
    let (directory, game) = fixture();
    let root = "materials/test/root.vmt";
    fs::write(directory.path().join(root), source).unwrap();
    let report = inspect(&game, root);
    assert_eq!(report.state, WaterDependencyState::Unsupported);
    assert_eq!(report.counters.texture_reads, 0);
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.code == "embedded_comment_marker")
    );
}

#[test]
fn boundary_comments_and_quoted_markers_keep_ordinary_declarations() {
    let source = "/* lead */ \"Water\"/* shader boundary */{ /* key boundary */ $normalmap /* value boundary */ \"dev/water_normal\"/* after quoted value */ // line boundary\n $fogcolor \"[0.1 0.2 0.3]\" %keywords \"quoted/*marker*/and//text\" }/* trailing */";
    let ordinary = vmt::Material::parse(source).unwrap();
    let raw = raw::parse(source.as_bytes()).unwrap();
    assert_eq!(raw.shader, ordinary.shader);
    for key in ["$normalmap", "$fogcolor", "%keywords"] {
        assert_eq!(Some(raw.parameters[key].value.as_str()), ordinary.get(key));
    }
    let (directory, game) = fixture();
    let root = "materials/test/root.vmt";
    fs::write(directory.path().join(root), source).unwrap();
    let report = inspect(&game, root);
    assert_eq!(
        report.state,
        WaterDependencyState::SupportedStaticInventory,
        "{:?}",
        report.issues
    );
    assert_eq!(report.counters.texture_reads, 1);
    assert_eq!(
        report
            .edges
            .iter()
            .find(|edge| edge.key == "$normalmap")
            .unwrap()
            .reference,
        "dev/water_normal"
    );
}

#[test]
fn material_roles_cycles_unknown_children_and_missing_targets_remain_explicit() {
    let (directory, game) = fixture();
    let root = "materials/test/root.vmt";
    fs::write(
        directory.path().join(root),
        "Water { $bottommaterial test/root }",
    )
    .unwrap();
    let cycle = inspect(&game, root);
    assert_eq!(cycle.state, WaterDependencyState::Unsupported);
    assert!(
        cycle
            .issues
            .iter()
            .any(|issue| issue.code == "material_cycle")
    );
    fs::write(
        directory.path().join(root),
        "Water { $underwateroverlay test/overlay }",
    )
    .unwrap();
    fs::write(
        directory.path().join("materials/test/overlay.vmt"),
        "Refract { $normalmap test/not_followed }",
    )
    .unwrap();
    let unknown = inspect(&game, root);
    assert_eq!(unknown.state, WaterDependencyState::Unsupported);
    assert!(
        unknown
            .assets
            .iter()
            .any(|asset| asset.path.ends_with("overlay.vmt")
                && asset.shader.as_deref() == Some("refract"))
    );
    assert!(
        !unknown
            .assets
            .iter()
            .any(|asset| asset.path.contains("not_followed"))
    );
    assert!(
        unknown
            .edges
            .iter()
            .any(|edge| edge.role == DependencyRole::RuntimeMaterial
                && edge.state == EdgeState::Unsupported)
    );
    fs::write(
        directory.path().join(root),
        "Water { $normalmap test/absent }",
    )
    .unwrap();
    let missing = inspect(&game, root);
    assert_eq!(missing.state, WaterDependencyState::UnavailableSource);
    assert!(
        missing
            .edges
            .iter()
            .any(|edge| edge.key == "$normalmap" && edge.state == EdgeState::Missing)
    );
    assert!(
        !missing
            .assets
            .iter()
            .any(|asset| asset.path == "materials/dev/water_normal.vtf")
    );
    for reference in ["", "../outside", " dev/water_normal "] {
        fs::write(
            directory.path().join(root),
            format!("Water {{ $normalmap \"{reference}\" }}"),
        )
        .unwrap();
        assert_eq!(
            inspect(&game, root).state,
            WaterDependencyState::UnavailableSource
        );
    }
}

#[test]
fn texture_types_raw_header_fields_and_codec_failures_are_observations_only() {
    for (case, state) in [
        (0, WaterDependencyState::SupportedStaticInventory),
        (1, WaterDependencyState::Unsupported),
        (2, WaterDependencyState::Unsupported),
        (3, WaterDependencyState::Unsupported),
        (4, WaterDependencyState::UnavailableSource),
        (5, WaterDependencyState::Unsupported),
        (6, WaterDependencyState::Unsupported),
        (7, WaterDependencyState::SupportedStaticInventory),
        (8, WaterDependencyState::SupportedStaticInventory),
    ] {
        let (directory, game) = fixture();
        let mut bytes = texture(false);
        match case {
            0 => {
                bytes[8..12].copy_from_slice(&0u32.to_le_bytes());
                bytes[12..16].copy_from_slice(&64u32.to_le_bytes());
                bytes.drain(64..80);
            }
            1 => bytes[24..26].copy_from_slice(&0u16.to_le_bytes()),
            2 => bytes[63..65].copy_from_slice(&0u16.to_le_bytes()),
            3 => bytes[52..56].copy_from_slice(&999u32.to_le_bytes()),
            4 => {
                bytes.pop();
            }
            5 => bytes = texture(true),
            6 | 7 => {
                bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
                bytes[18..20].copy_from_slice(&2u16.to_le_bytes());
                bytes.resize(80 + 16, 0);
                if case == 7 {
                    bytes[20..24].copy_from_slice(&source_formats::vtf::FLAG_NOMIP.to_le_bytes());
                }
            }
            8 => {}
            _ => unreachable!(),
        }
        fs::write(directory.path().join("materials/test/value.vtf"), bytes).unwrap();
        fs::write(
            directory.path().join("materials/test/root.vmt"),
            if case == 8 {
                let mut preview = texture(false);
                preview[16..18].copy_from_slice(&2u16.to_le_bytes());
                preview[18..20].copy_from_slice(&2u16.to_le_bytes());
                preview.resize(80 + 16, 0);
                fs::write(directory.path().join("materials/test/preview.vtf"), preview).unwrap();
                "Water { $normalmap test/value $basetexture test/value %tooltexture test/preview }"
            } else {
                "Water { $normalmap test/value $basetexture test/value }"
            },
        )
        .unwrap();
        let report = inspect(&game, "materials/test/root.vmt");
        assert_eq!(report.state, state, "case {case}: {:?}", report.issues);
        assert_eq!(report.counters.texture_reads, if case == 8 { 2 } else { 1 });
        if case == 6 {
            assert!(
                report
                    .issues
                    .iter()
                    .any(|issue| issue.code == "incomplete_mip_chain")
            );
        }
        if case == 0 {
            let value = report
                .assets
                .iter()
                .find_map(|asset| asset.texture.as_ref())
                .unwrap();
            assert_eq!(value.authored_depth, None);
            assert_eq!(value.format, "Rgba8888");
        }
    }
}

#[test]
fn original_cached_root_is_not_recollected_after_source_replacement() {
    let (directory, game) = fixture();
    let root = "materials/test/root.vmt";
    fs::write(
        directory.path().join(root),
        "Water { $normalmap dev/water_normal }",
    )
    .unwrap();
    let control = Control::default();
    let mut capture = MaterialInspectionCapture::new(&game, &control, policy());
    seed(&mut capture, root);
    fs::write(
        directory.path().join(root),
        "Water { $normalmap test/replacement }",
    )
    .unwrap();
    let report = inspect_water_dependencies(root, &mut capture, &control);
    assert_eq!(report.state, WaterDependencyState::UnavailableSource);
    assert!(
        report
            .edges
            .iter()
            .any(|edge| edge.target.as_deref() == Some("materials/dev/water_normal.vtf"))
    );
    assert!(
        !report
            .assets
            .iter()
            .any(|asset| asset.path.contains("replacement"))
    );
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.code == "source_changed")
    );
}

#[test]
fn original_include_is_not_captured_late_and_material_budget_is_global() {
    let (directory, game) = fixture();
    let root = "materials/test/root.vmt";
    fs::write(
        directory.path().join(root),
        "Patch { include materials/test/base.vmt }",
    )
    .unwrap();
    fs::write(directory.path().join("materials/test/base.vmt"), "Water {}").unwrap();
    let control = Control::default();
    let mut capture = MaterialInspectionCapture::new(&game, &control, policy());
    capture.read_material(root, MAX_MATERIAL_BYTES).unwrap();
    let report = inspect_water_dependencies(root, &mut capture, &control);
    assert_eq!(report.state, WaterDependencyState::UnavailableSource);
    assert_eq!(report.counters.material_reads, 1);
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.code == "uncaptured_original")
    );
    for index in 0..10 {
        let source = if index == 9 {
            "Water {}".into()
        } else {
            format!("Water {{ $bottommaterial test/node{} }}", index + 1)
        };
        fs::write(
            directory
                .path()
                .join(format!("materials/test/node{index}.vmt")),
            source,
        )
        .unwrap();
    }
    let report = inspect(&game, "materials/test/node0.vmt");
    assert_eq!(report.state, WaterDependencyState::Limited);
    assert_eq!(report.counters.material_reads, MAX_MATERIAL_READS);
    assert!(
        report
            .issues
            .iter()
            .any(|issue| matches!(issue.code, "material_depth_limit" | "material_read_limit"))
    );
}

#[test]
fn cancelled_and_uncaptured_inspections_do_not_establish_a_new_root() {
    let (directory, game) = fixture();
    let root = "materials/test/root.vmt";
    fs::write(directory.path().join(root), "Water {}").unwrap();
    let control = Control::default();
    let mut capture = MaterialInspectionCapture::new(&game, &control, policy());
    let uncaptured = inspect_water_dependencies(root, &mut capture, &control);
    assert_eq!(uncaptured.state, WaterDependencyState::UnavailableSource);
    assert_eq!(uncaptured.counters.asset_lookups, 0);
    seed(&mut capture, root);
    control.cancel().unwrap();
    let cancelled = inspect_water_dependencies(root, &mut capture, &control);
    assert_eq!(cancelled.state, WaterDependencyState::UnavailableSource);
    assert!(
        cancelled
            .issues
            .iter()
            .any(|issue| issue.code == "cancelled")
    );
}

#[test]
fn original_capture_limits_stay_typed_and_stop_extra_payload_reads() {
    let (directory, game) = fixture();
    let root = "materials/test/root.vmt";
    fs::write(directory.path().join(root), "Water {}").unwrap();
    let control = Control::default();
    let mut capped = policy();
    capped.physical_paths = 0;
    let mut capture = MaterialInspectionCapture::new(&game, &control, capped);
    seed(&mut capture, root);
    let report = inspect_water_dependencies(root, &mut capture, &control);
    assert_eq!(report.state, WaterDependencyState::Limited);
    assert_eq!(report.counters.material_reads, 1);
    assert_eq!(report.counters.texture_reads, 0);
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.code == "source_limit"
                && issue.state == WaterDependencyState::Limited)
    );
}

#[test]
fn texture_payload_and_axis_limits_have_distinct_partial_states() {
    let (directory, game) = fixture();
    let root = "materials/test/root.vmt";
    let value = directory.path().join("materials/test/value.vtf");
    fs::write(
        directory.path().join(root),
        "Water { $normalmap test/value }",
    )
    .unwrap();
    fs::File::create(&value)
        .unwrap()
        .set_len((MAX_TEXTURE_BYTES + 1) as u64)
        .unwrap();
    let oversized = inspect(&game, root);
    assert_eq!(oversized.state, WaterDependencyState::Limited);
    assert_eq!(oversized.counters.texture_reads, 1);
    assert_eq!(oversized.counters.texture_bytes, 0);
    assert!(
        oversized
            .issues
            .iter()
            .any(|issue| issue.code == "texture_read"
                && issue.state == WaterDependencyState::Limited)
    );

    let mut bytes = texture(false);
    let width = (MAX_TEXTURE_AXIS + 1) as u16;
    bytes[16..18].copy_from_slice(&width.to_le_bytes());
    bytes.resize(80 + usize::from(width) * 4, 127);
    fs::write(&value, bytes).unwrap();
    let too_wide = inspect(&game, root);
    assert_eq!(too_wide.state, WaterDependencyState::Unsupported);
    let asset = too_wide
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("value.vtf"))
        .unwrap();
    assert_eq!(asset.state, AssetState::UnsupportedTexture);
    assert_eq!(asset.texture.as_ref().unwrap().width, MAX_TEXTURE_AXIS + 1);
    assert!(asset.sha256.is_some());
}

#[test]
fn runtime_exceptions_are_role_specific_and_validate_the_complete_spelling() {
    let (directory, game) = fixture();
    let root = "materials/test/root.vmt";
    for (source, state) in [
        (
            "Water { $reflecttexture _RT_WATERREFLECTION $refracttexture _rt_waterrefraction $envmap ENV_CUBEMAP }",
            WaterDependencyState::SupportedStaticInventory,
        ),
        (
            "Water { $normalmap env_cubemap }",
            WaterDependencyState::Unsupported,
        ),
        (
            "Water { $reflecttexture _rt_WaterRefraction }",
            WaterDependencyState::Unsupported,
        ),
        (
            "Water { $underwateroverlay 00 }",
            WaterDependencyState::Unsupported,
        ),
        (
            "Water { $reflecttexture \"_rt_WaterReflection \" }",
            WaterDependencyState::UnavailableSource,
        ),
    ] {
        fs::write(directory.path().join(root), source).unwrap();
        let report = inspect(&game, root);
        assert_eq!(report.state, state, "{source}: {:?}", report.issues);
        assert!(!report.assets.iter().any(|asset| asset.path.contains("_rt_") || asset.path.ends_with("env_cubemap.vtf")));
    }
}
