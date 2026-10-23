use std::collections::{BTreeMap, BTreeSet};
use std::{fs, io};

use source_formats::bsp::{Bsp, PakArchive, PakLimits, StaticProp, StaticProps};
use source_formats::vmt::{MAX_PATCH_DEPTH, Material};

use super::closure::{Roots, collect_closure};
use super::tests::{FIRST, FIRST_TEXTURE, Fixture, MODEL, SECOND, VTX, VVD, prop_bsp, texture};
use super::*;
use crate::Control;
use crate::asset_sources::hash;

const BASE: &str = "materials/models/test/edit_base.vmt";
const MIDDLE: &str = "materials/models/test/edit_middle.vmt";
const BLUE: &str = "materials/test/edit_blue.vtf";
const GREEN: &str = "materials/test/edit_green.vtf";
const OTHER_MODEL: &str = "models/props/portable_copy.mdl";
const OTHER_VVD: &str = "models/props/portable_copy.vvd";
const OTHER_VTX: &str = "models/props/portable_copy.dx90.vtx";

fn include(path: &str) -> String {
    format!("Patch {{ include \"{path}\" }}")
}

fn edited(mode: &str, base: &str, texture: &str) -> String {
    format!("Patch {{ include \"{base}\" {mode} {{ $basetexture \"{texture}\" }} }}")
}

fn color(rgb: [u8; 3]) -> Vec<u8> {
    let mut bytes = texture();
    for pixel in bytes[80..].as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    bytes
}

fn edited_fixture(mode: &str) -> Fixture {
    let fixture = unseeded_edited_fixture(mode);
    fixture.seed_roots(false);
    fixture
}

fn unseeded_edited_fixture(mode: &str) -> Fixture {
    let fixture = Fixture::new();
    fixture.write(
        BASE,
        "// original red terminal\nVertexLitGeneric { $basetexture test/one }\n",
    );
    fixture.write(FIRST, edited(mode, BASE, "test/edit_blue"));
    fixture.write(SECOND, edited(mode, BASE, "test/edit_green"));
    fixture.write(FIRST_TEXTURE, color([255, 0, 0]));
    fixture.write(BLUE, color([0, 0, 255]));
    fixture.write(GREEN, color([0, 255, 0]));
    fixture
}

fn nest_edits(fixture: &Fixture, mode: &str) {
    let inner = if mode == "insert" {
        "replace"
    } else {
        "insert"
    };
    fixture.write(MIDDLE, edited(inner, BASE, "test/edit_green"));
    fixture.write(FIRST, edited(mode, MIDDLE, "test/edit_blue"));
    fixture.write(SECOND, include(MIDDLE));
}

fn collect(
    fixture: &Fixture,
    bsp: &Bsp,
    limits: PakLimits,
    reads: &mut BTreeMap<String, usize>,
) -> io::Result<BTreeSet<String>> {
    collect_closure(
        &Roots::from_bsp(bsp)?,
        limits,
        &Control::default(),
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

fn two_models(fixture: &Fixture, reversed: bool) -> Bsp {
    let mut mdl = fs::read(fixture.directory.path().join(MODEL)).unwrap();
    mdl[12..76].fill(0);
    let name = b"props/portable_copy.mdl";
    mdl[12..12 + name.len()].copy_from_slice(name);
    fixture.write(OTHER_MODEL, mdl);
    for (source, target) in [(VVD, OTHER_VVD), (VTX, OTHER_VTX)] {
        fixture.write(
            target,
            fs::read(fixture.directory.path().join(source)).unwrap(),
        );
    }
    fixture.seed_roots(false);
    fixture.seed(OTHER_MODEL);
    let mut bsp = prop_bsp(&[0]);
    let mut props = bsp.static_props().unwrap().unwrap();
    props
        .models
        .push(StaticProps::model_record(OTHER_MODEL).unwrap());
    if reversed {
        props.models.reverse();
    }
    props.props.push(StaticProp {
        model: 1,
        leaf_count: 1,
        ..Default::default()
    });
    bsp.set_static_props(&props).unwrap();
    bsp
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
fn both_edit_modes_select_literal_variants_and_pack_original_union_for_all_model_skins_offline() {
    for (mode, nested, first_texture) in [
        ("insert", false, "test/edit_blue"),
        ("replace", false, "test/edit_blue"),
        ("insert", true, "test/edit_green"),
        ("replace", true, "test/edit_blue"),
    ] {
        for reversed in [false, true] {
            let fixture = unseeded_edited_fixture(mode);
            if nested {
                nest_edits(&fixture, mode);
            }
            let mut bsp = two_models(&fixture, reversed);
            let selected_materials = [(FIRST, first_texture), (SECOND, "test/edit_green")];
            for (path, expected) in selected_materials {
                let selected = Material::load(&fixture.game, path).unwrap().unwrap();
                assert_eq!(selected.shader, "vertexlitgeneric");
                assert_eq!(selected.get("$basetexture"), Some(expected));
            }
            assert_ne!(
                fs::read(fixture.directory.path().join(BLUE)).unwrap(),
                fs::read(fixture.directory.path().join(FIRST_TEXTURE)).unwrap()
            );
            let mut expected = [
                MODEL,
                VVD,
                VTX,
                OTHER_MODEL,
                OTHER_VVD,
                OTHER_VTX,
                FIRST,
                SECOND,
                BASE,
                FIRST_TEXTURE,
                BLUE,
                GREEN,
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
            if nested {
                expected.insert(MIDDLE.into());
            }
            let mut reads = BTreeMap::new();
            let live = collect(&fixture, &bsp, PakLimits::default(), &mut reads).unwrap();
            assert_eq!(live, expected);
            assert!(reads.values().all(|count| *count == 1));
            let before = bsp.clone();
            let plan = fixture.plan(&bsp).unwrap();
            assert_eq!(
                plan.entries()
                    .iter()
                    .map(|entry| entry.name.clone())
                    .collect::<BTreeSet<_>>(),
                expected
            );
            assert_eq!(plan.sources().len(), expected.len());
            for (entry, source) in plan.entries().iter().zip(plan.sources()) {
                let original = fs::read(fixture.directory.path().join(&entry.name)).unwrap();
                assert_eq!(entry.data, original);
                assert_eq!(source.path(), entry.name);
                assert_eq!(source.sha256(), hash(&original));
                assert_eq!(source.bytes(), original.len());
                assert!(fixture.sources.is_observed(&entry.name).unwrap());
            }
            assert!(plan.uses().iter().all(|usage| usage.skin == 0));
            let manifest = plan.attach(&mut bsp, &Control::default()).unwrap();
            let mut without_pak = bsp.clone();
            without_pak.lumps[source_formats::bsp::lump::PAKFILE] =
                before.lumps[source_formats::bsp::lump::PAKFILE].clone();
            assert_eq!(without_pak, before);
            let archive = bsp.pak_data(PakLimits::default()).unwrap();
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
            assert_eq!(
                read_packed_static_props(&bsp, &Control::default()).unwrap(),
                Some(manifest.clone())
            );
            verify_packed_static_props(&bsp, &manifest, &Control::default()).unwrap();
            for (path, expected) in selected_materials {
                let selected = Material::load_with_reader(path, &mut |name| {
                    Ok(archive
                        .entries
                        .iter()
                        .find(|entry| entry.name == name)
                        .map(|entry| entry.data.clone()))
                })
                .unwrap()
                .unwrap();
                assert_eq!(selected.get("$basetexture"), Some(expected));
            }
        }
    }
}

#[test]
fn edit_raw_grammar_accepts_source_orders_and_case_but_no_extra_structure_or_resources() {
    for mode in ["insert", "replace"] {
        for body in [
            format!("include {BASE} {mode} {{ $basetexture test/edit_blue }}"),
            format!("{mode} {{ $basetexture test/edit_blue }} include {BASE}"),
            format!(
                "InClUdE {BASE} {} {{ $BaSeTeXtUrE test/edit_blue }}",
                mode.to_ascii_uppercase()
            ),
        ] {
            let fixture = edited_fixture(mode);
            let text = format!("/* original */ PaTcH {{ {body} }} // complete\n");
            fixture.write(FIRST, &text);
            assert!(material::dependency(FIRST, text.as_bytes()).is_ok());
            let selected = Material::load(&fixture.game, FIRST).unwrap().unwrap();
            assert_eq!(selected.get("$basetexture"), Some("test/edit_blue"));
            assert!(fixture.plan(&prop_bsp(&[0])).is_ok());
        }
    }
    for body in [
        format!("include {BASE} insert {{}}"),
        format!("include {BASE} insert scalar"),
        format!("include {BASE} insert {{ $basetexture {{}} }}"),
        format!("include {BASE} insert {{ $basetexture test/edit_blue $BaseTexture test/one }}"),
        format!(
            "include {BASE} insert {{ $basetexture test/edit_blue }} INSERT {{ $basetexture test/one }}"
        ),
        format!(
            "include {BASE} insert {{ $basetexture test/edit_blue }} replace {{ $basetexture test/one }}"
        ),
        format!("include {BASE} insert {{ $envmap env_cubemap }}"),
        format!("include {BASE} replace {{ $bumpmap test/one }}"),
        format!("include {BASE} insert {{ $basetexture test/edit_blue nested {{ value x }} }}"),
        format!("include {BASE} insert {{ $basetexture test/edit_blue }} Proxies {{}}"),
        format!("include {BASE} replace {{ $basetexture test/edit_blue }} [$WIN32]"),
        format!("include {BASE} #include other.vmt insert {{ $basetexture test/edit_blue }}"),
        format!("include {BASE} include {BASE} insert {{ $basetexture test/edit_blue }}"),
    ] {
        let text = format!("Patch {{ {body} }}");
        assert!(
            material::dependency(FIRST, text.as_bytes()).is_err(),
            "accepted {text}"
        );
    }
    for value in [
        "materials/test/edit_blue.vtf",
        "Test/edit_blue",
        "test//edit_blue",
        "test/../edit_blue",
        "test/edit_blue.vtf",
        "_rt_texture",
        " test/edit_blue",
        "test/edit_blue ",
    ] {
        assert!(
            material::dependency(FIRST, edited("insert", BASE, value).as_bytes()).is_err(),
            "accepted {value}"
        );
    }
    for path in [
        "models/test/edit_base.vmt",
        "materials/Models/test/edit_base.vmt",
        "materials/models/test/../edit_base.vmt",
        "materials/models/test/edit_base.VMT",
    ] {
        assert!(
            material::dependency(FIRST, edited("replace", path, "test/edit_blue").as_bytes())
                .is_err()
        );
    }
    assert!(
        material::dependency(FIRST, edited("insert", BASE, &"a".repeat(260)).as_bytes()).is_err()
    );
    for text in [
        format!(
            "{} VertexLitGeneric {{ $basetexture test/one }}",
            edited("replace", BASE, "test/edit_blue")
        ),
        format!("{}\0", edited("insert", BASE, "test/edit_blue")),
        format!("Patch {{ include {BASE} insert {{ $basetexture test/edit_blue }}"),
    ] {
        assert!(material::dependency(FIRST, text.as_bytes()).is_err());
    }
    let mut oversized = edited("insert", BASE, "test/edit_blue").into_bytes();
    oversized.resize(MAX_MATERIAL_BYTES + 1, b' ');
    assert!(material::dependency(FIRST, &oversized).is_err());
    assert!(material::dependency(FIRST, &[0xff]).is_err());
}

#[test]
fn missing_and_corrupt_originals_refuse_nested_edits_without_fallback() {
    for mode in ["insert", "replace"] {
        for path in [MIDDLE, BASE, FIRST_TEXTURE, BLUE, GREEN] {
            for corrupt in [false, true] {
                let fixture = edited_fixture(mode);
                nest_edits(&fixture, mode);
                if corrupt {
                    fixture.write(path, b"not a VTF");
                } else {
                    fs::remove_file(fixture.directory.path().join(path)).unwrap();
                }
                fixture.write(
                    "materials/one.vmt",
                    "VertexLitGeneric { $basetexture test/two }",
                );
                let error = fixture.plan(&prop_bsp(&[0])).unwrap_err();
                assert!(error.to_string().contains(path), "{error}");
                assert!(!fixture.sources.is_observed("materials/one.vmt").unwrap());
            }
        }
        let fixture = edited_fixture(mode);
        nest_edits(&fixture, mode);
        fixture.write(BASE, "VertexLitGeneric {}");
        assert!(fixture.plan(&prop_bsp(&[0])).is_err());
    }
}

#[test]
fn edited_originals_keep_first_read_and_late_source_identity_without_mutating_output() {
    for path in [FIRST, MIDDLE, BASE, FIRST_TEXTURE, BLUE, GREEN] {
        let fixture = edited_fixture("insert");
        nest_edits(&fixture, "insert");
        fixture.seed(path);
        fixture.mutate(path);
        assert!(fixture.plan(&prop_bsp(&[0])).is_err(), "rebaselined {path}");
        let fixture = edited_fixture("replace");
        nest_edits(&fixture, "replace");
        let plan = fixture.plan(&prop_bsp(&[0])).unwrap();
        fixture.mutate(path);
        assert!(plan.verify_source_metadata().is_err(), "unguarded {path}");
        assert!(
            plan.verify_sources(&fixture.game, &Control::default())
                .is_err()
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
fn nested_edits_preserve_inheritance_and_cached_suffix_depth_and_cycle_bounds() {
    for (first, second, selected) in [
        (
            edited("insert", BASE, "test/edit_blue"),
            include(FIRST),
            ["test/edit_blue", "test/edit_blue"],
        ),
        (
            include(SECOND),
            edited("insert", BASE, "test/edit_blue"),
            ["test/edit_blue", "test/edit_blue"],
        ),
        (
            include(BASE),
            edited("replace", FIRST, "test/edit_blue"),
            ["test/one", "test/edit_blue"],
        ),
        (
            edited("replace", SECOND, "test/edit_blue"),
            include(BASE),
            ["test/edit_blue", "test/one"],
        ),
        (
            edited("insert", BASE, "test/edit_blue"),
            edited("replace", FIRST, "test/edit_green"),
            ["test/edit_blue", "test/edit_green"],
        ),
        (
            edited("insert", SECOND, "test/edit_blue"),
            edited("replace", BASE, "test/edit_green"),
            ["test/edit_green", "test/edit_green"],
        ),
    ] {
        let fixture = edited_fixture("insert");
        fixture.write(FIRST, first);
        fixture.write(SECOND, second);
        let mut reads = BTreeMap::new();
        let paths = collect(&fixture, &prop_bsp(&[0]), PakLimits::default(), &mut reads).unwrap();
        assert!(reads.values().all(|count| *count == 1));
        assert!(paths.contains(FIRST_TEXTURE));
        assert!(paths.contains(BLUE));
        assert_eq!(paths.contains(GREEN), selected.contains(&"test/edit_green"));
        for (path, expected) in [FIRST, SECOND].into_iter().zip(selected) {
            let material = Material::load(&fixture.game, path).unwrap().unwrap();
            assert_eq!(material.get("$basetexture"), Some(expected));
        }
    }
    assert_eq!(MAX_PATCH_DEPTH, 8);
    for prefix in [0, 1, 2] {
        for edges in [MAX_PATCH_DEPTH - prefix, MAX_PATCH_DEPTH - prefix + 1] {
            let fixture = edited_fixture("replace");
            let mut path = FIRST.to_owned();
            for index in 0..edges {
                let next = format!("materials/models/test/nested_{index}.vmt");
                let mode = if index % 2 == 0 { "insert" } else { "replace" };
                fixture.write(&path, edited(mode, &next, "test/edit_blue"));
                path = next;
            }
            fixture.write(&path, "VertexLitGeneric { $basetexture test/one }");
            if prefix == 1 {
                fixture.write(SECOND, edited("replace", FIRST, "test/edit_green"));
            } else if prefix == 2 {
                fixture.write(SECOND, include(MIDDLE));
                fixture.write(MIDDLE, edited("insert", FIRST, "test/edit_green"));
            }
            let mut reads = BTreeMap::new();
            let result = collect(&fixture, &prop_bsp(&[0]), PakLimits::default(), &mut reads);
            if edges + prefix == MAX_PATCH_DEPTH {
                assert!(result.is_ok());
            } else {
                assert!(result.unwrap_err().to_string().contains("depth"));
                if prefix == 0 {
                    assert!(!reads.contains_key(&path));
                }
            }
            assert!(reads.values().all(|count| *count == 1));
        }
    }
    for cycle in [FIRST, MIDDLE] {
        let fixture = edited_fixture("replace");
        fixture.write(FIRST, edited("insert", MIDDLE, "test/edit_blue"));
        fixture.write(MIDDLE, edited("replace", cycle, "test/edit_green"));
        let mut reads = BTreeMap::new();
        let error =
            collect(&fixture, &prop_bsp(&[0]), PakLimits::default(), &mut reads).unwrap_err();
        assert!(error.to_string().contains("cycle"));
        assert!(reads.values().all(|count| *count == 1));
        assert!(!reads.contains_key(BLUE) && !reads.contains_key(GREEN));
    }
}

#[test]
fn repeated_texture_declarations_deduplicate_reads_and_preserve_combined_pak_budgets() {
    let fixture = edited_fixture("insert");
    fixture.write(FIRST, edited("insert", MIDDLE, "test/one"));
    fixture.write(MIDDLE, edited("replace", BASE, "test/one"));
    fixture.write(SECOND, edited("replace", MIDDLE, "test/one"));
    let mut reads = BTreeMap::new();
    let paths = collect(&fixture, &prop_bsp(&[0]), PakLimits::default(), &mut reads).unwrap();
    assert_eq!(paths.len(), 8);
    assert_eq!(reads.get(FIRST_TEXTURE), Some(&1));
    assert!(!paths.contains(BLUE) && !paths.contains(GREEN));
    let fixture = edited_fixture("replace");
    nest_edits(&fixture, "replace");
    let bsp = prop_bsp(&[0]);
    let plan = fixture.plan(&bsp).unwrap();
    assert_eq!(plan.entries().len(), 10);
    assert!(
        plan.entries()
            .iter()
            .any(|entry| entry.name == FIRST_TEXTURE)
    );
    assert!(plan.entries().iter().any(|entry| entry.name == BLUE));
    for limits in [
        PakLimits {
            max_entries: plan.entries().len(),
            ..PakLimits::default()
        },
        PakLimits {
            max_total_bytes: plan.asset_bytes() - 1,
            ..PakLimits::default()
        },
        PakLimits {
            max_archive_bytes: plan.combined_pak_bytes() - 1,
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
}

#[test]
fn cancellation_at_base_and_each_original_texture_read_stops_before_another_dependency() {
    for stop in [MIDDLE, BASE, FIRST_TEXTURE, BLUE, GREEN] {
        let fixture = edited_fixture("replace");
        nest_edits(&fixture, "replace");
        let control = Control::default();
        let mut cancelled = false;
        let mut reads = Vec::new();
        let error = collect_closure(
            &Roots::from_bsp(&prop_bsp(&[0])).unwrap(),
            PakLimits::default(),
            &control,
            &mut |path, _maximum, _original| {
                assert!(!cancelled, "read {path} after cancellation at {stop}");
                reads.push(path.to_owned());
                let bytes = fs::read(fixture.directory.path().join(path))?;
                if path == stop {
                    control.cancel().unwrap();
                    cancelled = true;
                }
                Ok(Some(bytes))
            },
        )
        .unwrap_err();
        assert!(cancelled, "did not visit {stop}");
        assert!(error.to_string().contains("cancelled"));
        assert_eq!(reads.last().map(String::as_str), Some(stop));
        let mut bsp = prop_bsp(&[0]);
        let plan = fixture.plan(&bsp).unwrap();
        let before = bsp.clone();
        assert!(plan.attach(&mut bsp, &control).is_err());
        assert_eq!(bsp, before);
    }
}

#[test]
fn offline_edits_recheck_omissions_rehashed_structure_and_original_identity() {
    let fixture = edited_fixture("replace");
    nest_edits(&fixture, "replace");
    let mut original = prop_bsp(&[0]);
    let plan = fixture.plan(&original).unwrap();
    let manifest = plan.attach(&mut original, &Control::default()).unwrap();
    for path in [MIDDLE, BASE, FIRST_TEXTURE, BLUE, GREEN] {
        let mut bsp = original.clone();
        let mut archive = bsp.pak_data(PakLimits::default()).unwrap();
        let mut changed = manifest.clone();
        archive.entries.retain(|entry| entry.name != path);
        changed.assets.retain(|asset| asset.path != path);
        persist(&mut bsp, &mut archive, &changed);
        assert!(
            read_packed_static_props(&bsp, &Control::default()).is_err(),
            "omitted {path}"
        );
    }
    for text in [
        format!("Patch {{ include {BASE} insert {{ $envmap env_cubemap }} }}"),
        edited("replace", FIRST, "test/edit_blue"),
        format!(
            "Patch {{ include {BASE} insert {{ $basetexture test/edit_blue }} replace {{ $basetexture test/edit_green }} }}"
        ),
    ] {
        let mut bsp = original.clone();
        let mut archive = bsp.pak_data(PakLimits::default()).unwrap();
        let mut changed = manifest.clone();
        let entry = archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == FIRST)
            .unwrap();
        entry.data = text.into_bytes();
        let asset = changed
            .assets
            .iter_mut()
            .find(|asset| asset.path == FIRST)
            .unwrap();
        asset.bytes = entry.data.len();
        asset.sha256 = hash(&entry.data);
        persist(&mut bsp, &mut archive, &changed);
        assert!(read_packed_static_props(&bsp, &Control::default()).is_err());
    }
    let mut bsp = original.clone();
    let mut archive = bsp.pak_data(PakLimits::default()).unwrap();
    let mut changed = manifest.clone();
    let entry = archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == FIRST)
        .unwrap();
    entry.data = edited("replace", MIDDLE, "test/edit_green").into_bytes();
    let asset = changed
        .assets
        .iter_mut()
        .find(|asset| asset.path == FIRST)
        .unwrap();
    asset.bytes = entry.data.len();
    asset.sha256 = hash(&entry.data);
    archive.entries.retain(|entry| entry.name != BLUE);
    changed.assets.retain(|asset| asset.path != BLUE);
    persist(&mut bsp, &mut archive, &changed);
    assert_eq!(
        read_packed_static_props(&bsp, &Control::default()).unwrap(),
        Some(changed)
    );
    assert!(verify_packed_static_props(&bsp, &manifest, &Control::default()).is_err());
}
