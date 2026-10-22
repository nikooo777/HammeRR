use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;

use source_formats::bsp::{Bsp, PakArchive, PakLimits};
use source_formats::vmt::MAX_PATCH_DEPTH;

use super::closure::{Roots, collect_closure};
use super::tests::{
    FIRST, FIRST_TEXTURE, Fixture, MODEL, SECOND, SECOND_TEXTURE, VTX, VVD, prop_bsp,
};
use super::*;
use crate::Control;
use crate::asset_sources::hash;

const MIDDLE: &str = "materials/models/test/middle.vmt";
const FIRST_BASE: &str = "materials/models/test/first_base.vmt";
const SECOND_BASE: &str = "materials/models/test/second_base.vmt";

fn patch(include: &str) -> String {
    format!("\"Patch\" {{ \"include\" \"{include}\" }}")
}

fn patch_fixture() -> Fixture {
    let fixture = Fixture::new();
    fixture.write(FIRST, patch(MIDDLE));
    fixture.write(MIDDLE, patch(FIRST_BASE));
    fixture.write(
        FIRST_BASE,
        "// original terminal\nVertexLitGeneric { $basetexture test/one }\n",
    );
    fixture.write(SECOND, patch(SECOND_BASE));
    fixture.write(
        SECOND_BASE,
        "VertexLitGeneric { $basetexture test/two } // unused skin\n",
    );
    fixture.seed_roots(false);
    fixture
}

fn collect_files(
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

fn persist(bsp: &mut Bsp, archive: &mut PakArchive, manifest: &StaticPropAssetManifest) {
    archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == PACKED_STATIC_PROPS_MANIFEST_PATH)
        .unwrap()
        .data = serde_json::to_vec(manifest).unwrap();
    bsp.set_pak_data(archive, PakLimits::default()).unwrap();
}

fn padded_material(text: &str, bytes: usize) -> Vec<u8> {
    let mut padded = text.as_bytes().to_vec();
    padded.resize(bytes, b' ');
    padded
}

fn chain_path(index: usize) -> String {
    format!("materials/models/test/chain_{index}.vmt")
}

fn write_chain(fixture: &Fixture, root: &str, edges: usize) {
    let mut path = root.to_owned();
    for index in 0..edges {
        let next = chain_path(index);
        fixture.write(&path, patch(&next));
        path = next;
    }
    fixture.write(&path, "VertexLitGeneric { $basetexture test/one }");
}

#[test]
fn original_patch_chains_cover_all_declared_skins_and_reconstruct_without_sources() {
    let fixture = patch_fixture();
    let mut bsp = prop_bsp(&[0]);
    let before = bsp.clone();
    let plan = fixture.plan(&bsp).unwrap();
    let expected = [
        MODEL,
        VVD,
        VTX,
        FIRST,
        MIDDLE,
        FIRST_BASE,
        SECOND,
        SECOND_BASE,
        FIRST_TEXTURE,
        SECOND_TEXTURE,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    assert_eq!(
        plan.entries()
            .iter()
            .map(|entry| entry.name.clone())
            .collect::<BTreeSet<_>>(),
        expected
    );
    let mut reads = BTreeMap::new();
    let live = collect_files(
        &fixture,
        PakLimits::default(),
        &Control::default(),
        &mut reads,
    )
    .unwrap();
    assert_eq!(live, expected);
    assert!(reads.values().all(|count| *count == 1));
    for (entry, source) in plan.entries().iter().zip(plan.sources()) {
        let original = fs::read(fixture.directory.path().join(&entry.name)).unwrap();
        assert_eq!(entry.data, original);
        assert_eq!(source.path(), entry.name);
        assert_eq!(source.sha256(), hash(&original));
        assert_eq!(source.bytes(), original.len());
        assert!(fixture.sources.is_observed(&entry.name).unwrap());
    }
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
            let entry = archive.entries.iter().find(|entry| entry.name == path);
            if let Some(entry) = entry {
                assert!(entry.data.len() <= maximum);
            }
            Ok(entry.map(|entry| entry.data.clone()))
        },
    )
    .unwrap();
    assert_eq!(offline, live);
    assert_eq!(
        read_packed_static_props(&bsp, &Control::default()).unwrap(),
        Some(manifest.clone())
    );
    verify_packed_static_props(&bsp, &manifest, &Control::default()).unwrap();
}

#[test]
fn patch_syntax_requires_one_canonical_scalar_include() {
    for text in [
        patch(FIRST_BASE),
        format!("PaTcH {{ InClUdE \"{FIRST_BASE}\" }}"),
        format!("/* original */ Patch {{ include \"{FIRST_BASE}\" }} // end"),
    ] {
        assert_eq!(
            material::dependency(FIRST, text.as_bytes()).unwrap(),
            material::Dependency::Include(FIRST_BASE.into())
        );
    }
    for include in [
        "models/test/base.vmt",
        "models/test/base",
        "materials/models/test/base",
        "Materials/models/test/base.vmt",
        "materials/Models/test/base.vmt",
        "materials/models/test/base.VMT",
        " materials/models/test/base.vmt",
        "materials/models/test/base.vmt ",
        "materials//models/test/base.vmt",
        "materials/models/./test/base.vmt",
        "materials/models/../base.vmt",
        "materials\\models\\test\\base.vmt",
        "/materials/models/test/base.vmt",
        "materials/materials/test/base.vmt",
        "materials/_rt_base.vmt",
        "materials/test/base.copy.vmt",
        "materials/test/base.vmt.vmt",
        "materials/test/base?.vmt",
        "materials/test/básé.vmt",
    ] {
        assert!(
            material::dependency(FIRST, patch(include).as_bytes()).is_err(),
            "accepted {include}"
        );
    }
    for body in [
        "",
        "include {}",
        "include materials/models/test/first_base.vmt include materials/models/test/first_base.vmt",
        "include materials/models/test/first_base.vmt replace {}",
        "include materials/models/test/first_base.vmt insert {}",
        "include materials/models/test/first_base.vmt Proxies {}",
        "include materials/models/test/first_base.vmt $basetexture test/one",
        "include materials/models/test/first_base.vmt $envmap env_cubemap",
        "include materials/models/test/first_base.vmt shader VertexLitGeneric",
        "include materials/models/test/first_base.vmt [$WIN32]",
    ] {
        let text = format!("Patch {{ {body} }}");
        assert!(
            material::dependency(FIRST, text.as_bytes()).is_err(),
            "accepted {text}"
        );
    }
    for text in [
        format!("Patch {{ include {FIRST_BASE} }} VertexLitGeneric {{}}"),
        format!("Patch {{ include {FIRST_BASE}"),
        format!("Patch {{ include {FIRST_BASE} }}\0"),
    ] {
        assert!(material::dependency(FIRST, text.as_bytes()).is_err());
    }
    let oversized = padded_material(&patch(FIRST_BASE), MAX_MATERIAL_BYTES + 1);
    assert!(material::dependency(FIRST, &oversized).is_err());
    let long_path = format!("materials/{}.vmt", "a".repeat(260));
    assert!(material::dependency(FIRST, patch(&long_path).as_bytes()).is_err());
}

#[test]
fn missing_or_unsupported_includes_never_fall_back_or_skip_unselected_materials() {
    for root in [FIRST, SECOND] {
        let fixture = Fixture::new();
        fixture.seed_roots(false);
        fixture.write(root, patch(FIRST_BASE));
        fixture.write(
            "materials/one.vmt",
            "VertexLitGeneric { $basetexture test/one }",
        );
        let error = fixture.plan(&prop_bsp(&[0])).unwrap_err();
        assert!(error.to_string().contains(FIRST_BASE));
        assert!(fixture.sources.is_observed(FIRST_BASE).unwrap());
        assert!(!fixture.sources.is_observed("materials/one.vmt").unwrap());
        let guard = fixture
            .sources
            .source_guard(&fixture.game, &Control::default())
            .unwrap();
        fixture.write(FIRST_BASE, "VertexLitGeneric { $basetexture test/one }");
        assert!(
            guard
                .verify_sources(&fixture.game, &Control::default())
                .is_err()
        );
        assert!(fixture.plan(&prop_bsp(&[0])).is_err());
    }
    for terminal in [
        "VertexLitGeneric { $basetexture test/one $envmap env_cubemap }",
        "VertexLitGeneric { $basetexture test/one Proxies {} }",
        "VertexLitGeneric { $basetexture test/one $bumpmap test/two }",
        "UnlitGeneric { $basetexture test/one }",
        "VertexLitGeneric { $basetexture _rt_texture }",
    ] {
        let fixture = patch_fixture();
        fixture.write(FIRST_BASE, terminal);
        assert!(
            fixture.plan(&prop_bsp(&[0])).is_err(),
            "accepted {terminal}"
        );
    }
}

#[test]
fn every_original_include_identity_remains_guarded_before_and_after_collection() {
    for path in [MIDDLE, FIRST_BASE, SECOND_BASE] {
        let fixture = patch_fixture();
        fixture.seed(path);
        fixture.mutate(path);
        assert!(fixture.plan(&prop_bsp(&[0])).is_err(), "rebaselined {path}");
        let fixture = patch_fixture();
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
fn include_cycles_fail_before_repeated_reads() {
    for cycle in [FIRST, MIDDLE] {
        let fixture = Fixture::new();
        fixture.write(FIRST, patch(MIDDLE));
        fixture.write(MIDDLE, patch(cycle));
        let mut reads = BTreeMap::new();
        let error = collect_files(
            &fixture,
            PakLimits::default(),
            &Control::default(),
            &mut reads,
        )
        .unwrap_err();
        assert!(error.to_string().contains("cycle"));
        assert_eq!(reads.get(FIRST), Some(&1));
        assert_eq!(reads.get(MIDDLE), Some(&1));
    }
}

#[test]
fn include_edge_depth_is_bounded_before_reading_the_next_material() {
    for edges in [MAX_PATCH_DEPTH, MAX_PATCH_DEPTH + 1] {
        let fixture = Fixture::new();
        write_chain(&fixture, FIRST, edges);
        let mut reads = BTreeMap::new();
        let result = collect_files(
            &fixture,
            PakLimits::default(),
            &Control::default(),
            &mut reads,
        );
        if edges == MAX_PATCH_DEPTH {
            assert!(result.is_ok());
            assert_eq!(reads.get(&chain_path(edges - 1)), Some(&1));
        } else {
            assert!(result.unwrap_err().to_string().contains("depth"));
            assert!(!reads.contains_key(&chain_path(edges - 1)));
        }
    }
}

#[test]
fn shared_include_suffix_is_read_once_without_bypassing_depth_bounds() {
    for prefix in [1, 2] {
        for edges in [MAX_PATCH_DEPTH - prefix, MAX_PATCH_DEPTH - prefix + 1] {
            let fixture = Fixture::new();
            write_chain(&fixture, FIRST, edges);
            if prefix == 1 {
                fixture.write(SECOND, patch(FIRST));
            } else {
                fixture.write(SECOND, patch(SECOND_BASE));
                fixture.write(SECOND_BASE, patch(FIRST));
            }
            let mut reads = BTreeMap::new();
            let result = collect_files(
                &fixture,
                PakLimits::default(),
                &Control::default(),
                &mut reads,
            );
            if edges + prefix == MAX_PATCH_DEPTH {
                assert!(result.is_ok());
            } else {
                assert!(result.unwrap_err().to_string().contains("depth"));
            }
            assert_eq!(reads.get(FIRST), Some(&1));
            assert_eq!(reads.get(SECOND), Some(&1));
            assert!(reads.values().all(|count| *count == 1));
        }
    }
}

#[test]
fn included_declared_material_is_not_read_again_when_its_skin_is_visited() {
    let fixture = Fixture::new();
    fixture.write(FIRST, patch(SECOND));
    let mut reads = BTreeMap::new();
    let paths = collect_files(
        &fixture,
        PakLimits::default(),
        &Control::default(),
        &mut reads,
    )
    .unwrap();
    assert_eq!(paths.len(), 6);
    assert!(paths.contains(SECOND));
    assert!(paths.contains(SECOND_TEXTURE));
    assert!(!paths.contains(FIRST_TEXTURE));
    assert_eq!(reads.get(SECOND), Some(&1));
}

#[test]
fn includes_share_material_and_pak_budgets_and_respect_remaining_read_limits() {
    let fixture = Fixture::new();
    fixture.write(FIRST, patch(FIRST_BASE));
    fixture.write(SECOND, patch(FIRST_BASE));
    fixture.write(
        FIRST_BASE,
        padded_material(&patch(MIDDLE), MAX_MATERIAL_BYTES),
    );
    fixture.write(
        MIDDLE,
        padded_material(&patch(SECOND_BASE), MAX_MATERIAL_BYTES),
    );
    fixture.write(
        SECOND_BASE,
        padded_material(
            "VertexLitGeneric { $basetexture test/one }",
            MAX_MATERIAL_BYTES,
        ),
    );
    fixture.seed_roots(false);
    let plan = fixture.plan(&prop_bsp(&[0])).unwrap();
    assert_eq!(plan.entries().len(), 9);
    for limits in [
        PakLimits {
            max_entry_bytes: MAX_MATERIAL_BYTES - 1,
            ..PakLimits::default()
        },
        PakLimits {
            max_entries: plan.entries().len(),
            ..PakLimits::default()
        },
        PakLimits {
            max_name_bytes: VTX.len().max(FIRST.len()).max(SECOND.len()),
            ..PakLimits::default()
        },
        PakLimits {
            max_total_bytes: plan.asset_bytes() - 1,
            ..PakLimits::default()
        },
    ] {
        assert!(
            collect_static_prop_asset_plan(
                &prop_bsp(&[0]),
                &fixture.game,
                &fixture.sources,
                &PakArchive::default(),
                limits,
                &Control::default(),
            )
            .is_err()
        );
    }
    let fixture = Fixture::new();
    fixture.write(FIRST, patch(FIRST_BASE));
    fixture.write(FIRST_BASE, vec![b' '; MAX_MATERIAL_BYTES + 1]);
    fixture.seed_roots(false);
    assert!(fixture.plan(&prop_bsp(&[0])).is_err());
    let fixture = Fixture::new();
    let mut path = FIRST.to_owned();
    for index in 0..4 {
        let next = chain_path(index);
        fixture.write(&path, padded_material(&patch(&next), MAX_MATERIAL_BYTES));
        path = next;
    }
    fixture.write(&path, "VertexLitGeneric { $basetexture test/one }");
    let mut final_maximum = None;
    let error = collect_closure(
        &Roots::from_bsp(&prop_bsp(&[0])).unwrap(),
        PakLimits::default(),
        &Control::default(),
        &mut |name, maximum, _original| {
            if name == path {
                final_maximum = Some(maximum);
            }
            Ok(Some(fs::read(fixture.directory.path().join(name))?))
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("byte budget"));
    assert_eq!(final_maximum, Some(0));
}

#[test]
fn cancellation_during_include_read_stops_before_the_next_dependency() {
    let fixture = patch_fixture();
    let control = Control::default();
    let mut reads = Vec::new();
    let error = collect_closure(
        &Roots::from_bsp(&prop_bsp(&[0])).unwrap(),
        PakLimits::default(),
        &control,
        &mut |path, _maximum, _original| {
            reads.push(path.to_owned());
            if path == MIDDLE {
                control.cancel().unwrap();
            }
            Ok(Some(fs::read(fixture.directory.path().join(path))?))
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert!(reads.iter().any(|path| path == MIDDLE));
    assert!(!reads.iter().any(|path| path == FIRST_BASE));
    assert!(!reads.iter().any(|path| path == FIRST_TEXTURE));
    let mut bsp = prop_bsp(&[0]);
    let plan = fixture.plan(&bsp).unwrap();
    let before = bsp.clone();
    assert!(plan.attach(&mut bsp, &control).is_err());
    assert_eq!(bsp, before);
}

#[test]
fn offline_verification_requires_every_original_include_and_rechecks_rehashed_syntax() {
    let fixture = patch_fixture();
    let mut original = prop_bsp(&[0]);
    let plan = fixture.plan(&original).unwrap();
    let manifest = plan.attach(&mut original, &Control::default()).unwrap();
    for path in [MIDDLE, FIRST_BASE, SECOND_BASE] {
        for unregister in [false, true] {
            let mut bsp = original.clone();
            let mut archive = bsp.pak_data(PakLimits::default()).unwrap();
            let mut manifest = manifest.clone();
            archive.entries.retain(|entry| entry.name != path);
            if unregister {
                manifest.assets.retain(|asset| asset.path != path);
            }
            persist(&mut bsp, &mut archive, &manifest);
            assert!(read_packed_static_props(&bsp, &Control::default()).is_err());
        }
    }
    for replacement in [
        patch(FIRST),
        patch("materials/models/test/missing.vmt"),
        "Patch { include models/test/first_base.vmt }".into(),
        format!("Patch {{ include {FIRST_BASE} insert {{}} }}"),
    ] {
        let mut bsp = original.clone();
        let mut archive = bsp.pak_data(PakLimits::default()).unwrap();
        let mut changed = manifest.clone();
        let entry = archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == MIDDLE)
            .unwrap();
        entry.data = replacement.into_bytes();
        let asset = changed
            .assets
            .iter_mut()
            .find(|asset| asset.path == MIDDLE)
            .unwrap();
        asset.sha256 = hash(&entry.data);
        asset.bytes = entry.data.len();
        persist(&mut bsp, &mut archive, &changed);
        assert!(read_packed_static_props(&bsp, &Control::default()).is_err());
        assert!(verify_packed_static_props(&bsp, &manifest, &Control::default()).is_err());
    }
}
