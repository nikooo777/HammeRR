use std::collections::{BTreeMap, BTreeSet};
use std::{fs, io};

use source_formats::bsp::{self, Bsp, PakArchive, PakLimits};
use source_formats::vmf::Entity;
use source_formats::vmt::{MAX_PATCH_DEPTH, Material};
use source_formats::vtf::Vtf;

use super::closure::{Roots, collect_closure};
use super::tests::{
    FIRST, FIRST_TEXTURE, Fixture, MODEL, SECOND, SECOND_TEXTURE, VTX, VVD, prop_bsp, texture,
};
use super::*;
use crate::asset_sources::hash;
use crate::{Control, LightingMode, Options};

const BASE: &str = "materials/models/test/alpha_base.vmt";
const MIDDLE: &str = "materials/models/test/alpha_middle.vmt";
const EDIT_TEXTURE: &str = "materials/test/alpha_edit.vtf";
const FIRST_ALPHA: [u8; 5] = [96, 0, 127, 128, 255];
const SECOND_ALPHA: [u8; 5] = [160, 255, 128, 127, 0];
const EDIT_ALPHA: [u8; 5] = [32, 128, 0, 255, 127];

fn alpha_material(texture: &str) -> String {
    format!("// original cutout\nVertexLitGeneric {{ $basetexture {texture} $alphatest \"1\" }}\n")
}

fn include(path: &str) -> String {
    format!("Patch {{ include \"{path}\" }}")
}

fn edited(mode: &str, base: &str) -> String {
    format!("Patch {{ include \"{base}\" {mode} {{ $basetexture test/alpha_edit }} }}")
}

fn alpha_texture(alphas: [u8; 5]) -> Vec<u8> {
    let mut bytes = texture();
    bytes[20..24].copy_from_slice(&0x2000_u32.to_le_bytes());
    for (pixel, alpha) in bytes[80..].as_chunks_mut::<4>().0.iter_mut().zip(alphas) {
        pixel[3] = alpha;
    }
    bytes
}

fn alpha_fixture(composition: &str) -> Fixture {
    let fixture = Fixture::new();
    fixture.write(FIRST, alpha_material("test/one"));
    fixture.write(SECOND, alpha_material("test/two"));
    fixture.write(FIRST_TEXTURE, alpha_texture(FIRST_ALPHA));
    fixture.write(SECOND_TEXTURE, alpha_texture(SECOND_ALPHA));
    if composition != "plain" {
        fixture.write(BASE, alpha_material("test/one"));
        if composition == "include" {
            fixture.write(FIRST, include(MIDDLE));
            fixture.write(MIDDLE, include(BASE));
        } else {
            fixture.write(FIRST, edited(composition, BASE));
            fixture.write(EDIT_TEXTURE, alpha_texture(EDIT_ALPHA));
        }
    }
    fixture.seed_roots(false);
    fixture
}

fn collect(
    fixture: &Fixture,
    limits: PakLimits,
    control: &Control,
    reads: &mut BTreeMap<String, usize>,
) -> io::Result<BTreeSet<String>> {
    collect_closure(
        &Roots::from_bsp(&prop_bsp(&[0]))?,
        limits,
        control,
        &mut |path, _maximum, _original| {
            *reads.entry(path.into()).or_default() += 1;
            match fs::read(fixture.directory.path().join(path)) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error),
            }
        },
    )
}

fn assert_alpha_payload(bytes: &[u8], alphas: [u8; 5]) {
    let vtf = Vtf::parse(bytes.to_vec()).unwrap();
    assert_eq!(
        (vtf.width, vtf.height, vtf.mip_count, vtf.flags),
        (2, 2, 2, 0x2000)
    );
    assert_eq!(vtf.image_data(1, 0, 0).unwrap(), [31, 63, 127, alphas[0]]);
    assert_eq!(
        vtf.image_data(0, 0, 0).unwrap(),
        alphas[1..]
            .iter()
            .flat_map(|alpha| [31, 63, 127, *alpha])
            .collect::<Vec<_>>()
    );
}

fn persist(bsp: &mut Bsp, archive: &mut PakArchive, manifest: &StaticPropAssetManifest) {
    archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == PACKED_STATIC_PROPS_MANIFEST_PATH)
        .unwrap()
        .data = serde_json::to_vec(manifest).unwrap();
    bsp.set_pak_data(archive, PakLimits::default()).unwrap();
}

#[test]
fn all_declared_alpha_skins_and_compositions_pack_original_mips_and_reconstruct_offline() {
    for composition in ["plain", "include", "insert", "replace"] {
        let fixture = alpha_fixture(composition);
        let mut bsp = prop_bsp(&[0]);
        let before = bsp.clone();
        let mut expected = [
            MODEL,
            VVD,
            VTX,
            FIRST,
            SECOND,
            FIRST_TEXTURE,
            SECOND_TEXTURE,
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        match composition {
            "include" => expected.extend([BASE, MIDDLE].map(str::to_owned)),
            "insert" | "replace" => expected.extend([BASE, EDIT_TEXTURE].map(str::to_owned)),
            _ => {}
        }
        let mut reads = BTreeMap::new();
        assert_eq!(
            collect(
                &fixture,
                PakLimits::default(),
                &Control::default(),
                &mut reads
            )
            .unwrap(),
            expected
        );
        assert!(reads.values().all(|count| *count == 1));
        let plan = fixture.plan(&bsp).unwrap();
        assert_eq!(
            plan.entries()
                .iter()
                .map(|entry| entry.name.clone())
                .collect::<BTreeSet<_>>(),
            expected
        );
        assert_eq!(plan.sources().len(), expected.len());
        assert!(plan.uses().iter().all(|usage| usage.skin == 0));
        for (entry, source) in plan.entries().iter().zip(plan.sources()) {
            let original = fs::read(fixture.directory.path().join(&entry.name)).unwrap();
            assert_eq!(entry.data, original);
            assert_eq!(source.path(), entry.name);
            assert_eq!(source.sha256(), hash(&original));
            assert_eq!(source.bytes(), original.len());
            assert!(fixture.sources.is_observed(&entry.name).unwrap());
        }
        let mdl = &plan
            .entries()
            .iter()
            .find(|entry| entry.name == MODEL)
            .unwrap()
            .data;
        assert_eq!(
            u32::from_le_bytes(mdl[152..156].try_into().unwrap()) & 0x40000,
            0
        );
        let manifest = plan.attach(&mut bsp, &Control::default()).unwrap();
        let mut without_pak = bsp.clone();
        without_pak.lumps[bsp::lump::PAKFILE] = before.lumps[bsp::lump::PAKFILE].clone();
        assert_eq!(without_pak, before);
        let archive = bsp.pak_data(PakLimits::default()).unwrap();
        for original in plan.entries() {
            assert_eq!(
                archive
                    .entries
                    .iter()
                    .find(|entry| entry.name == original.name)
                    .unwrap()
                    .data,
                original.data
            );
        }
        for (path, alphas) in [(FIRST_TEXTURE, FIRST_ALPHA), (SECOND_TEXTURE, SECOND_ALPHA)] {
            assert_alpha_payload(
                &archive
                    .entries
                    .iter()
                    .find(|entry| entry.name == path)
                    .unwrap()
                    .data,
                alphas,
            );
        }
        if matches!(composition, "insert" | "replace") {
            assert_alpha_payload(
                &archive
                    .entries
                    .iter()
                    .find(|entry| entry.name == EDIT_TEXTURE)
                    .unwrap()
                    .data,
                EDIT_ALPHA,
            );
        }
        fs::remove_dir_all(fixture.directory.path().join("materials")).unwrap();
        fs::remove_dir_all(fixture.directory.path().join("models")).unwrap();
        let offline = collect_closure(
            &Roots::from_bsp(&bsp).unwrap(),
            PakLimits::default(),
            &Control::default(),
            &mut |path, maximum, _original| {
                let bytes = archive
                    .entries
                    .iter()
                    .find(|entry| entry.name == path)
                    .map(|entry| entry.data.clone());
                if let Some(bytes) = &bytes {
                    assert!(bytes.len() <= maximum);
                }
                Ok(bytes)
            },
        )
        .unwrap();
        assert_eq!(offline, expected);
        for (path, texture) in [
            (
                FIRST,
                if matches!(composition, "insert" | "replace") {
                    "test/alpha_edit"
                } else {
                    "test/one"
                },
            ),
            (SECOND, "test/two"),
        ] {
            let material = Material::load_with_reader(path, &mut |path| {
                Ok(archive
                    .entries
                    .iter()
                    .find(|entry| entry.name == path)
                    .map(|entry| entry.data.clone()))
            })
            .unwrap()
            .unwrap();
            assert_eq!(material.shader, "vertexlitgeneric");
            assert_eq!(material.get("$basetexture"), Some(texture));
            assert!(material.alpha_test());
            assert_eq!(material.get("$alphatestreference"), None);
        }
        assert_eq!(
            read_packed_static_props(&bsp, &Control::default()).unwrap(),
            Some(manifest.clone())
        );
        verify_packed_static_props(&bsp, &manifest, &Control::default()).unwrap();
    }
}

#[test]
fn alpha_terminal_raw_admission_requires_one_literal_flag_without_custom_reference_or_extra_keys() {
    for text in [
        "VertexLitGeneric { $basetexture test/one $alphatest 1 }",
        "VertexLitGeneric { $alphatest \"1\" $basetexture test/one }",
        "/* original */ vErTeXlItGeNeRiC { $AlphaTest 1 $BaseTexture test/one } // end\n",
    ] {
        let fixture = alpha_fixture("plain");
        fixture.write(FIRST, text);
        assert_eq!(
            material::dependency(FIRST, text.as_bytes()).unwrap(),
            material::Dependency::Texture(FIRST_TEXTURE.into())
        );
        let selected = Material::load(&fixture.game, FIRST).unwrap().unwrap();
        assert!(selected.alpha_test());
        assert!(fixture.plan(&prop_bsp(&[0])).is_ok());
    }
    for flag in [
        "0", "01", "+1", "1.0", "1e0", "NaN", "true", "\"\"", "\" 1\"", "\"1 \"", "{}",
    ] {
        let text = format!("VertexLitGeneric {{ $basetexture test/one $alphatest {flag} }}");
        assert!(
            material::dependency(FIRST, text.as_bytes()).is_err(),
            "accepted {text}"
        );
    }
    for body in [
        "$alphatest 1",
        "$basetexture test/one $alphatest 1 $AlphaTest 1",
        "$basetexture test/one $BaseTexture test/two $alphatest 1",
        "$basetexture test/one $alphatest 1 $alphatestreference 0.5",
        "$basetexture test/one $alphatest 1 $alphatestreference 0",
        "$basetexture test/one $alphatest 1 $nocull 1",
        "$basetexture test/one $alphatest 1 $model 1",
        "$basetexture test/one $alphatest 1 $translucent 1",
        "$basetexture test/one $alphatest { value 1 }",
        "$basetexture test/one $alphatest 1 Proxies {}",
        "$basetexture test/one $alphatest 1 [$WIN32]",
        "$basetexture test/one $alphatest 1 #include other.vmt",
    ] {
        let text = format!("VertexLitGeneric {{ {body} }}");
        assert!(
            material::dependency(FIRST, text.as_bytes()).is_err(),
            "accepted {text}"
        );
    }
    for text in [
        "VertexLitGeneric { $basetexture test/one $alphatest 1 } VertexLitGeneric { $basetexture test/two }".to_owned(),
        format!("Patch {{ include {BASE} replace {{ $alphatest 1 }} }}"),
        format!("{}\0", alpha_material("test/one")),
    ] {
        assert!(material::dependency(FIRST, text.as_bytes()).is_err(), "accepted {text}");
    }
    assert!(material::dependency(FIRST, b"VertexLitGeneric { $basetexture test/one }").is_ok());
}

#[test]
fn selected_unselected_and_overridden_alpha_inputs_remain_required_and_source_owned() {
    for (composition, path) in [
        ("plain", FIRST_TEXTURE),
        ("plain", SECOND_TEXTURE),
        ("replace", BASE),
        ("replace", FIRST_TEXTURE),
        ("replace", EDIT_TEXTURE),
    ] {
        for corrupt in [false, true] {
            let fixture = alpha_fixture(composition);
            if corrupt {
                if path == BASE {
                    fixture.write(
                        path,
                        "VertexLitGeneric { $basetexture test/one $alphatest 1.0 }",
                    );
                } else {
                    let mut truncated = fs::read(fixture.directory.path().join(path)).unwrap();
                    truncated.pop();
                    fixture.write(path, truncated);
                }
            } else {
                fs::remove_file(fixture.directory.path().join(path)).unwrap();
            }
            let error = fixture.plan(&prop_bsp(&[0])).unwrap_err();
            assert!(error.to_string().contains(path), "{error}");
        }
    }
    for path in [BASE, FIRST_TEXTURE, SECOND_TEXTURE, EDIT_TEXTURE] {
        let fixture = alpha_fixture("replace");
        fixture.seed(path);
        fixture.mutate(path);
        assert!(fixture.plan(&prop_bsp(&[0])).is_err(), "rebaselined {path}");
        let fixture = alpha_fixture("replace");
        let plan = fixture.plan(&prop_bsp(&[0])).unwrap();
        fixture.mutate(path);
        assert!(
            plan.verify_sources(&fixture.game, &Control::default())
                .is_err(),
            "unguarded {path}"
        );
        assert!(
            plan.capture_source_guard(&fixture.game, &Control::default())
                .is_err()
        );
        let mut bsp = prop_bsp(&[0]);
        let before = bsp.clone();
        assert!(plan.attach(&mut bsp, &Control::default()).is_err());
        assert_eq!(bsp, before);
    }
}

#[test]
fn alpha_terminals_keep_cached_direct_edits_and_existing_include_depth_without_edited_nesting() {
    for edited_first in [false, true] {
        let fixture = alpha_fixture("plain");
        fixture.write(EDIT_TEXTURE, alpha_texture(EDIT_ALPHA));
        let (plain, patch, base_texture) = if edited_first {
            (SECOND, FIRST, SECOND_TEXTURE)
        } else {
            (FIRST, SECOND, FIRST_TEXTURE)
        };
        fixture.write(patch, edited("replace", plain));
        let mut reads = BTreeMap::new();
        let paths = collect(
            &fixture,
            PakLimits::default(),
            &Control::default(),
            &mut reads,
        )
        .unwrap();
        assert_eq!(
            paths,
            [MODEL, VVD, VTX, FIRST, SECOND, base_texture, EDIT_TEXTURE]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
        assert!(reads.values().all(|count| *count == 1));
        assert_eq!(reads.get(base_texture), Some(&1));
        assert_eq!(reads.get(EDIT_TEXTURE), Some(&1));
        let fixture = alpha_fixture("replace");
        if edited_first {
            fixture.write(SECOND, include(FIRST));
        } else {
            fixture.write(FIRST, include(SECOND));
            fixture.write(SECOND, edited("replace", BASE));
        }
        let error = collect(
            &fixture,
            PakLimits::default(),
            &Control::default(),
            &mut BTreeMap::new(),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("direct plain VertexLitGeneric"),
            "{error}"
        );
    }
    for edges in [MAX_PATCH_DEPTH, MAX_PATCH_DEPTH + 1] {
        let fixture = alpha_fixture("plain");
        let mut path = FIRST.to_owned();
        for index in 0..edges {
            let next = format!("materials/models/test/alpha_chain_{index}.vmt");
            fixture.write(&path, include(&next));
            path = next;
        }
        fixture.write(&path, alpha_material("test/one"));
        let result = collect(
            &fixture,
            PakLimits::default(),
            &Control::default(),
            &mut BTreeMap::new(),
        );
        if edges == MAX_PATCH_DEPTH {
            assert!(result.is_ok());
        } else {
            assert!(result.unwrap_err().to_string().contains("depth"));
        }
    }
}

#[test]
fn alpha_closure_budgets_and_cancellation_include_original_and_unselected_textures() {
    let fixture = alpha_fixture("insert");
    let bsp = prop_bsp(&[0]);
    let plan = fixture.plan(&bsp).unwrap();
    assert_eq!(plan.entries().len(), 9);
    for limits in [
        PakLimits {
            max_entries: 9,
            ..PakLimits::default()
        },
        PakLimits {
            max_total_bytes: plan.asset_bytes() - 1,
            ..PakLimits::default()
        },
    ] {
        assert!(
            collect_static_prop_asset_plan(
                &bsp,
                &fixture.game,
                &fixture.sources,
                &PakArchive::default(),
                limits,
                &Control::default()
            )
            .is_err()
        );
    }
    for stop in [BASE, SECOND_TEXTURE] {
        let control = Control::default();
        let mut stopped = false;
        let error = collect_closure(
            &Roots::from_bsp(&bsp).unwrap(),
            PakLimits::default(),
            &control,
            &mut |path, _maximum, _original| {
                assert!(!stopped, "read {path} after cancellation at {stop}");
                let bytes = fs::read(fixture.directory.path().join(path))?;
                if path == stop {
                    stopped = true;
                    control.cancel().unwrap();
                }
                Ok(Some(bytes))
            },
        )
        .unwrap_err();
        assert!(stopped && error.to_string().contains("cancelled"));
        let mut target = bsp.clone();
        assert!(plan.attach(&mut target, &control).is_err());
        assert_eq!(target, bsp);
    }
}

#[test]
fn rehashed_offline_alpha_bundles_still_check_grammar_mips_and_overridden_dependencies() {
    let fixture = alpha_fixture("replace");
    let mut original = prop_bsp(&[0]);
    let manifest = fixture
        .plan(&original)
        .unwrap()
        .attach(&mut original, &Control::default())
        .unwrap();
    for case in ["custom reference", "truncated mip", "omitted base"] {
        let mut bsp = original.clone();
        let mut archive = bsp.pak_data(PakLimits::default()).unwrap();
        let mut changed = manifest.clone();
        if case == "omitted base" {
            archive.entries.retain(|entry| entry.name != FIRST_TEXTURE);
            changed.assets.retain(|asset| asset.path != FIRST_TEXTURE);
        } else {
            let path = if case == "custom reference" {
                BASE
            } else {
                SECOND_TEXTURE
            };
            let entry = archive
                .entries
                .iter_mut()
                .find(|entry| entry.name == path)
                .unwrap();
            if case == "custom reference" {
                entry.data = b"VertexLitGeneric { $basetexture test/one $alphatest 1 $alphatestreference 0.5 }".to_vec();
            } else {
                entry.data.pop();
            }
            let asset = changed
                .assets
                .iter_mut()
                .find(|asset| asset.path == path)
                .unwrap();
            asset.bytes = entry.data.len();
            asset.sha256 = hash(&entry.data);
        }
        persist(&mut bsp, &mut archive, &changed);
        assert!(
            read_packed_static_props(&bsp, &Control::default()).is_err(),
            "accepted {case}"
        );
    }
}

#[test]
fn packing_unselected_cutouts_does_not_admit_alpha_vertex_receivers_or_request_texture_shadows() {
    let fixture = alpha_fixture("plain");
    fixture.write(FIRST, "VertexLitGeneric { $basetexture test/one }");
    let mut source = crate::test_common::room();
    let owner = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
    let mut prop = Entity::new(owner, "prop_static");
    for (key, value) in [("model", MODEL), ("origin", "32 32 8"), ("solid", "0")] {
        prop.properties.set(key, value);
    }
    source.entities.push(prop);
    let options = Options {
        pack_static_prop_assets: true,
        bake_static_prop_vertices: true,
        lighting: LightingMode::DirectLdrHdr,
        sky_quality: crate::SkyQuality::Fast,
        ..Default::default()
    };
    for (skin, opt_out) in [("0", false), ("1", false), ("1", true)] {
        let prop = source.entities.last_mut().unwrap();
        prop.properties.set("skin", skin);
        prop.properties
            .set("disablevertexlighting", if opt_out { "1" } else { "0" });
        let before = source.clone();
        let output = crate::compile_with_options(&source, &fixture.game, options);
        assert_eq!(source, before);
        if skin == "1" && !opt_out {
            assert!(output.bsp.is_none() && output.report.has_errors());
            assert!(
                output
                    .report
                    .diagnostics
                    .iter()
                    .any(
                        |diagnostic| diagnostic.object == Some(format!("entity:{owner}"))
                            && diagnostic.message.contains("$alphatest")
                            && diagnostic.message.contains("plain opaque")
                    ),
                "{:?}",
                output.report
            );
            continue;
        }
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.prop_shadow_alpha_triangles, 0);
        let bsp = output.bsp.as_ref().unwrap();
        let plan = output.static_prop_assets.as_ref().unwrap();
        verify_packed_static_props(bsp, &plan.manifest(), &Control::default()).unwrap();
        assert!(
            plan.entries()
                .iter()
                .any(|entry| entry.name == SECOND_TEXTURE)
        );
        if opt_out {
            assert!(output.prop_lighting_assets.is_none());
            assert!(
                bsp.pak_data(PakLimits::default())
                    .unwrap()
                    .entries
                    .iter()
                    .all(|entry| !entry.name.ends_with(".vhv"))
            );
            assert!(
                output
                    .report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains("opaque mesh triangles")),
                "{:?}",
                output.report
            );
        } else {
            let lighting = output.prop_lighting_assets.as_ref().unwrap();
            assert_eq!(lighting.manifest().receivers.len(), 1);
            crate::prop_lighting_assets::verify_generated_prop_lighting(
                bsp,
                lighting.manifest(),
                &Control::default(),
            )
            .unwrap();
            let destination = fixture.directory.path().join("published/previous.bsp");
            fs::write(&destination, b"previous BSP").unwrap();
            fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
            let staged = output
                .prepare_write_controlled(&destination, &Control::default())
                .unwrap()
                .unwrap();
            fixture.mutate(SECOND_TEXTURE);
            assert!(staged.commit(&Control::default()).is_err());
            assert!(output.write(&destination).is_err());
            assert_eq!(fs::read(&destination).unwrap(), b"previous BSP");
            assert_eq!(
                fs::read(destination.with_extension("lin")).unwrap(),
                b"previous LIN"
            );
            fixture.write(SECOND_TEXTURE, alpha_texture(SECOND_ALPHA));
        }
    }
}
