use super::*;
use crate::session::Actor;
use source_formats::gamefs::{GameFs, Mount, MountedPath};

fn fixture() -> (tempfile::TempDir, Session) {
    let root = tempfile::tempdir().unwrap();
    let mounts = ["first", "second"]
        .into_iter()
        .map(|name| {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.join("materials/test")).unwrap();
            MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Directory(path),
            }
        })
        .collect();
    let game = GameFs {
        game_dir: root.path().to_owned(),
        base_dir: root.path().to_owned(),
        mounts,
    };
    (root, Session::new(Ok(game)))
}

#[test]
fn material_source_preserves_winning_paths_raw_text_patch_declarations_and_session_state() {
    let (root, mut session) = fixture();
    let selected = root.path().join("first/materials/test/selected.vmt");
    let base = root.path().join("second/materials/test/base.vmt");
    let text = "Patch { include materials/test/base.vmt replace { $basetexture test/replacement $albedo test/replacement_albedo } insert { $detail test/detail } }";
    std::fs::write(&selected, text).unwrap();
    std::fs::write(
        &base,
        "LightmappedGeneric { $basetexture test/original $albedo test/original_albedo $bumpmap test/normal }",
    )
    .unwrap();
    let id = session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(64.0),
            "test/selected",
            false,
        )
        .unwrap();
    session
        .preview_transform(
            Actor::Agent,
            &[id],
            map_core::edit::Transform {
                translation: glam::DVec3::X * 32.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let source = session.document.vmf().clone();
    let draft = session.render_document().vmf().clone();
    let snapshot = session.history_snapshot();
    let camera = session.camera();
    let selection = session.selected_ids();
    let scene = session.scene_serial;
    let log = session.log_serial;
    let generation = session.materials.generation();
    let report = session
        .inspect_material_source("MATERIALS\\TEST\\SELECTED.VMT")
        .unwrap();
    assert_eq!(report.material, "test/selected");
    assert_eq!(report.vmt_path, "materials/test/selected.vmt");
    assert_eq!(report.raw_source.as_deref(), Some(text));
    assert_eq!(report.raw_source_bytes, text.len());
    assert!(!report.raw_source_truncated);
    assert!(report.error.is_none());
    assert_eq!(report.files.len(), 2);
    assert_eq!(
        report.files[0].origin.as_ref().unwrap().file.as_deref(),
        selected.to_str()
    );
    assert_eq!(
        report.files[1].origin.as_ref().unwrap().file.as_deref(),
        base.to_str()
    );
    let resolved = report.resolved.unwrap();
    assert_eq!(resolved.shader, "lightmappedgeneric");
    let textures = resolved
        .texture_parameters
        .iter()
        .map(|texture| (texture.parameter, texture.value.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        textures,
        [
            ("$basetexture", "test/replacement"),
            ("$albedo", "test/replacement_albedo"),
            ("$bumpmap", "test/normal"),
            ("$detail", "test/detail")
        ]
    );
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.render_document().vmf(), &draft);
    assert_eq!(session.history_snapshot(), snapshot);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.selected_ids(), selection);
    assert_eq!(session.scene_serial, scene);
    assert_eq!(session.log_serial, log);
    assert_eq!(session.materials.generation(), generation);
}

#[test]
fn material_source_reports_failures_without_lower_priority_or_display_fallback() {
    let (root, session) = fixture();
    let first = root.path().join("first/materials/test/selected.vmt");
    std::fs::write(
        root.path().join("second/materials/test/selected.vmt"),
        "UnlitGeneric { $basetexture lower_priority }",
    )
    .unwrap();
    for text in [
        b"invalid".as_slice(),
        b"Patch { include materials/test/absent.vmt }",
        b"UnlitGeneric { $basetexture \xff }",
    ] {
        std::fs::write(&first, text).unwrap();
        let report = session.inspect_material_source("test/selected").unwrap();
        assert!(report.error.is_some());
        assert!(report.resolved.is_none());
        assert_eq!(
            report.files[0].origin.as_ref().unwrap().file.as_deref(),
            first.to_str()
        );
        assert!(
            report
                .files
                .iter()
                .filter_map(|file| file.origin.as_ref())
                .all(|origin| origin.mount.ends_with("first"))
        );
        if text.contains(&0xff) {
            assert!(report.error.unwrap().contains("UTF-8"));
            assert!(report.raw_source.is_none());
        } else {
            assert_eq!(report.raw_source.as_deref(), std::str::from_utf8(text).ok());
        }
    }
    std::fs::remove_file(&first).unwrap();
    let report = session.inspect_material_source("test/selected").unwrap();
    assert!(report.error.is_none());
    assert!(
        report.files[0]
            .origin
            .as_ref()
            .unwrap()
            .mount
            .ends_with("second")
    );
    let report = session.inspect_material_source("test/missing").unwrap();
    assert!(report.resolved.is_none());
    assert!(report.error.unwrap().contains("not found"));
    assert!(report.files[0].origin.is_none());
    for name in [
        "",
        "materials/",
        "../outside",
        "/absolute",
        "test/../../outside",
        &"x".repeat(MAX_PATH_BYTES + 1),
    ] {
        assert!(session.inspect_material_source(name).is_err());
    }
}

#[test]
fn material_source_bounds_utf8_source_files_include_chains_and_declarations() {
    let (root, session) = fixture();
    let path = root.path().join("first/materials/test/selected.vmt");
    let text =
        "UnlitGeneric { $basetexture test/exact }\n// ".to_owned() + &"雪".repeat(MAX_SOURCE_BYTES);
    std::fs::write(&path, text.as_bytes()).unwrap();
    let report = session.inspect_material_source("test/selected").unwrap();
    assert!(report.error.is_none());
    assert!(report.raw_source_truncated);
    assert_eq!(report.raw_source_bytes, text.len());
    assert!(report.raw_source.as_ref().unwrap().len() <= MAX_SOURCE_BYTES);
    assert!(text.starts_with(report.raw_source.as_ref().unwrap()));
    std::fs::write(&path, vec![b' '; MAX_FILE_BYTES + 1]).unwrap();
    let report = session.inspect_material_source("test/selected").unwrap();
    assert!(report.error.unwrap().contains("limit"));
    assert!(report.resolved.is_none());
    for index in 0..MAX_FILES {
        let next = if index + 1 == MAX_FILES {
            "UnlitGeneric {}".into()
        } else {
            format!("Patch {{ include materials/test/chain_{}.vmt }}", index + 1)
        };
        std::fs::write(
            root.path()
                .join(format!("first/materials/test/chain_{index}.vmt")),
            next,
        )
        .unwrap();
    }
    let report = session.inspect_material_source("test/chain_0").unwrap();
    assert!(report.error.is_none());
    assert_eq!(report.files.len(), MAX_FILES);
    std::fs::write(
        root.path().join("first/materials/test/chain_8.vmt"),
        "Patch { include materials/test/chain_0.vmt }",
    )
    .unwrap();
    let report = session.inspect_material_source("test/chain_0").unwrap();
    assert!(report.error.unwrap().contains("deeply"));
    assert_eq!(report.files.len(), MAX_FILES);
    std::fs::write(
        &path,
        format!(
            "{} {{ $basetexture {} }}",
            "s".repeat(MAX_SHADER_CHARS + 1),
            "t".repeat(MAX_TEXTURE_CHARS + 1)
        ),
    )
    .unwrap();
    let resolved = session
        .inspect_material_source("test/selected")
        .unwrap()
        .resolved
        .unwrap();
    assert!(resolved.shader_truncated);
    assert!(resolved.texture_parameters[0].truncated);
    assert_eq!(resolved.shader.len(), MAX_SHADER_CHARS);
    assert_eq!(
        resolved.texture_parameters[0].value.len(),
        MAX_TEXTURE_CHARS
    );
}

#[test]
fn material_source_enforces_total_read_budget_across_patch_files() {
    let (root, session) = fixture();
    for index in 0..5 {
        let mut text = if index == 4 {
            "UnlitGeneric {}".into()
        } else {
            format!(
                "Patch {{ include materials/test/budget_{}.vmt }}",
                index + 1
            )
        };
        text.push_str(&" ".repeat(MAX_FILE_BYTES - text.len()));
        std::fs::write(
            root.path()
                .join(format!("first/materials/test/budget_{index}.vmt")),
            text,
        )
        .unwrap();
    }
    let report = session.inspect_material_source("test/budget_0").unwrap();
    assert!(report.error.unwrap().contains("limit"));
    assert!(report.resolved.is_none());
    assert_eq!(
        report.files.iter().map(|file| file.bytes).sum::<usize>(),
        MAX_TOTAL_BYTES
    );
    assert_eq!(report.files.len(), 4);
}

#[test]
#[cfg(unix)]
fn material_source_refuses_dangling_higher_priority_paths_instead_of_showing_lower_source() {
    use std::os::unix::fs::symlink;
    let (root, session) = fixture();
    std::fs::write(
        root.path().join("second/materials/test/selected.vmt"),
        "UnlitGeneric { $basetexture wrong_mount }",
    )
    .unwrap();
    let first = root.path().join("first/materials/test/selected.vmt");
    symlink("missing.vmt", &first).unwrap();
    let report = session.inspect_material_source("test/selected").unwrap();
    assert!(report.error.unwrap().contains("dangling"));
    assert!(report.resolved.is_none());
    assert!(report.raw_source.is_none());
    std::fs::remove_file(&first).unwrap();
    std::fs::remove_dir_all(root.path().join("first/materials")).unwrap();
    symlink("missing-directory", root.path().join("first/materials")).unwrap();
    let report = session.inspect_material_source("test/selected").unwrap();
    assert!(report.error.unwrap().contains("dangling"));
    assert!(report.resolved.is_none());
}

#[test]
fn full_virtual_source_filename_retains_nested_materials_directory() {
    let (root, session) = fixture();
    let nested = root.path().join("first/materials/materials");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(
        nested.join("foo.vmt"),
        "Sprite { $basetexture test/nested }",
    )
    .unwrap();
    std::fs::write(
        root.path().join("first/materials/foo.vmt"),
        "Sprite { $basetexture test/root }",
    )
    .unwrap();
    let report = session
        .inspect_material_source("materials/materials/foo.vmt")
        .unwrap();
    assert_eq!(report.material, "materials/foo");
    assert_eq!(report.vmt_path, "materials/materials/foo.vmt");
    assert_eq!(
        report.raw_source.as_deref(),
        Some("Sprite { $basetexture test/nested }")
    );
}

#[test]
fn water_inventory_keeps_legacy_patch_files_and_preserves_shared_session_state() {
    use compiler::water_dependencies::{DependencyRole, WaterDependencyState};
    use sha2::{Digest, Sha256};

    let (root, game) = crate::test_assets::game();
    let folder = root.path().join("materials/test");
    let text =
        "Patch { include materials/test/water_base.vmt replace { $basetexture test/checker } }";
    std::fs::write(folder.join("water.vmt"), text).unwrap();
    std::fs::write(
        folder.join("water_base.vmt"),
        "Water { $normalmap test/checker $basetexture test/checker %tooltexture test/checker $bottommaterial test/bottom $underwateroverlay 0 }",
    )
    .unwrap();
    std::fs::write(
        folder.join("bottom.vmt"),
        "Water_DX90 { $normalmap test/checker }",
    )
    .unwrap();
    let mut session = Session::new(Ok(game));
    session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(64.0),
            "test/checker",
            false,
        )
        .unwrap();
    session
        .begin_property_edit(crate::session::PropertyTarget::World)
        .unwrap();
    let token = session.property_draft().unwrap().token;
    session
        .update_property_edit(
            token,
            vec![crate::session::PropertyChange::Set {
                key: "skyname".into(),
                value: "pending_sky".into(),
            }],
        )
        .unwrap();
    let source = session.document.vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let selection = session.selected_ids();
    let draft = serde_json::to_value(session.property_draft()).unwrap();
    let settings = session.compile_settings();
    let recipes = session.build_recipes();
    let scene = session.scene_serial;
    let log = session.log_serial;
    let generation = session.materials.generation();
    let files = ["water.vmt", "water_base.vmt", "bottom.vmt", "checker.vtf"]
        .map(|name| (name, std::fs::read(folder.join(name)).unwrap()));

    let report = session.inspect_material_source("test/water").unwrap();
    assert!(report.error.is_none());
    assert_eq!(report.raw_source.as_deref(), Some(text));
    assert_eq!(report.resolved.as_ref().unwrap().shader, "water");
    assert_eq!(
        report
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["materials/test/water.vmt", "materials/test/water_base.vmt"]
    );
    let water = &report.water_dependencies;
    assert_eq!(water.state, WaterDependencyState::SupportedStaticInventory);
    assert_eq!(water.model_basis, "css_sdk_dx90_static_water");
    assert!(water.issues.is_empty());
    assert_eq!(water.assets.len(), files.len());
    assert_eq!(water.counters.material_reads, 3);
    assert_eq!(water.counters.texture_reads, 1);
    for (name, bytes) in &files {
        let path = format!("materials/test/{name}");
        let asset = water
            .assets
            .iter()
            .find(|asset| asset.path == path)
            .unwrap();
        assert_eq!(asset.bytes, bytes.len());
        assert_eq!(
            asset.sha256.as_deref(),
            Some(format!("{:x}", Sha256::digest(bytes)).as_str())
        );
        assert_eq!(
            asset.origin.as_ref().unwrap().file.as_deref(),
            folder.join(name).to_str()
        );
        assert_eq!(std::fs::read(folder.join(name)).unwrap(), *bytes);
    }
    assert!(water.edges.iter().any(|edge| {
        edge.key == "$bottommaterial"
            && edge.role == DependencyRole::CompilerMaterial
            && edge.target.as_deref() == Some("materials/test/bottom.vmt")
    }));
    let texture = water
        .assets
        .iter()
        .find_map(|asset| asset.texture.as_ref())
        .unwrap();
    assert_eq!([texture.width, texture.height, texture.depth], [64, 32, 1]);
    assert_eq!(texture.authored_frames, 1);
    assert_eq!(texture.layout_scope, "existing_vtf_parser");
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.selected_ids(), selection);
    assert_eq!(
        serde_json::to_value(session.property_draft()).unwrap(),
        draft
    );
    assert_eq!(session.compile_settings(), settings);
    assert_eq!(session.build_recipes(), recipes);
    assert_eq!(session.scene_serial, scene);
    assert_eq!(session.log_serial, log);
    assert_eq!(session.materials.generation(), generation);
}

#[test]
fn water_only_failures_leave_source_declarations_available_and_failed_roots_do_not_repair() {
    use compiler::water_dependencies::WaterDependencyState;

    let (root, game) = crate::test_assets::game();
    let path = root.path().join("materials/test/water.vmt");
    let session = Session::new(Ok(game));
    for (text, state) in [
        (
            "Water { $normalmap test/checker $flowmap test/not-read }",
            WaterDependencyState::Unsupported,
        ),
        (
            "Water { $normalmap test/missing }",
            WaterDependencyState::UnavailableSource,
        ),
    ] {
        std::fs::write(&path, text).unwrap();
        let report = session.inspect_material_source("test/water").unwrap();
        assert!(report.error.is_none());
        assert_eq!(report.raw_source.as_deref(), Some(text));
        assert_eq!(report.resolved.as_ref().unwrap().shader, "water");
        assert_eq!(report.files.len(), 1);
        assert_eq!(report.water_dependencies.state, state);
        assert!(!report.water_dependencies.issues.is_empty());
    }
    std::fs::write(&path, "Patch { include materials/test/missing.vmt }").unwrap();
    let report = session.inspect_material_source("test/water").unwrap();
    assert!(report.error.is_some());
    assert!(report.resolved.is_none());
    assert_eq!(report.files.len(), 2);
    assert_eq!(
        report.water_dependencies.state,
        WaterDependencyState::UnavailableSource
    );
    assert!(report.water_dependencies.assets.is_empty());
    assert!(report.water_dependencies.edges.is_empty());
    assert_eq!(report.water_dependencies.counters.asset_lookups, 0);
    std::fs::write(&path, "UnlitGeneric { $basetexture test/not-read }").unwrap();
    let report = session.inspect_material_source("test/water").unwrap();
    assert!(report.error.is_none());
    assert_eq!(
        report.water_dependencies.state,
        WaterDependencyState::NotApplicable
    );
    assert!(report.water_dependencies.assets.is_empty());
    assert!(report.water_dependencies.issues.is_empty());
    assert_eq!(report.water_dependencies.counters.asset_lookups, 0);
}

#[test]
fn water_graph_cannot_reset_the_legacy_material_budget_for_an_extra_material() {
    use compiler::water_dependencies::WaterDependencyState;

    let (root, game) = crate::test_assets::game();
    let folder = root.path().join("materials/test");
    for index in 0..MAX_FILES {
        let text = if index + 1 == MAX_FILES {
            "Water { $normalmap test/checker $bottommaterial test/extra }".into()
        } else {
            format!("Patch {{ include materials/test/water_{}.vmt }}", index + 1)
        };
        std::fs::write(folder.join(format!("water_{index}.vmt")), text).unwrap();
    }
    std::fs::write(
        folder.join("extra.vmt"),
        "Water { $normalmap test/checker }",
    )
    .unwrap();
    let session = Session::new(Ok(game));
    let report = session.inspect_material_source("test/water_0").unwrap();
    assert!(report.error.is_none());
    assert_eq!(report.files.len(), MAX_FILES);
    assert_eq!(report.resolved.as_ref().unwrap().shader, "water");
    assert_eq!(
        report.water_dependencies.state,
        WaterDependencyState::Limited
    );
    assert_eq!(report.water_dependencies.counters.material_reads, MAX_FILES);
    assert!(!report.water_dependencies.issues.is_empty());
    assert!(
        report
            .files
            .iter()
            .all(|file| !file.path.ends_with("/extra.vmt"))
    );
}
