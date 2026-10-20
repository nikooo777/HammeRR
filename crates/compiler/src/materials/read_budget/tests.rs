use std::path::Path;

use source_formats::gamefs::{Mount, MountedPath};
use source_formats::vpk::Vpk;

use super::super::{
    DimensionSource, Materials, MetricState, PropSourceMode, ReflectivitySource, Requirements,
};
use super::*;

fn limits(file_bytes: usize, total_bytes: usize, operations: usize) -> ReadLimits {
    ReadLimits {
        label: "Test material source",
        file_bytes,
        total_bytes,
        operations,
    }
}

fn fixture() -> (tempfile::TempDir, GameFs) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/validation/693");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    let mut game = crate::test_common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn write(directory: &Path, path: &str, bytes: impl AsRef<[u8]>) {
    std::fs::write(directory.join(path), bytes).unwrap();
}

fn texture(reflectivity: [f32; 3]) -> Vec<u8> {
    let mut bytes = vec![0; 84];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7_u32), (8, 2), (12, 80)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for offset in [16, 18, 24, 63] {
        bytes[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
    }
    for (axis, value) in reflectivity.into_iter().enumerate() {
        bytes[32 + axis * 4..36 + axis * 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[56] = 1;
    bytes[57..61].copy_from_slice(&(-1_i32).to_le_bytes());
    bytes[80..84].copy_from_slice(&[32, 64, 128, 255]);
    bytes
}

fn vpk(path: &Path, extension: &str, name: &str, payload: &[u8], declared: usize) {
    let mut tree = format!("{extension}\0materials\0{name}\0").into_bytes();
    tree.extend(0u32.to_le_bytes());
    tree.extend(0u16.to_le_bytes());
    tree.extend(0x7fffu16.to_le_bytes());
    tree.extend(0u32.to_le_bytes());
    tree.extend((declared as u32).to_le_bytes());
    tree.extend(0xffffu16.to_le_bytes());
    tree.extend([0, 0, 0]);
    let mut bytes = 0x55aa_1234u32.to_le_bytes().to_vec();
    bytes.extend(1u32.to_le_bytes());
    bytes.extend((tree.len() as u32).to_le_bytes());
    bytes.extend(tree);
    bytes.extend(payload);
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn exact_directory_limits_charge_actual_successes_and_fail_before_an_exhausted_lookup() {
    let (directory, game) = fixture();
    write(directory.path(), "materials/three.vmt", b"abc");
    write(directory.path(), "materials/two.vmt", b"de");
    write(directory.path(), "materials/four.vmt", b"abcd");
    let mut budget = ReadBudget::new(limits(3, 5, 6));
    let error = budget
        .read(&game, "materials/four.vmt", usize::MAX, None)
        .unwrap_err();
    assert!(error.to_string().contains("per-file limit 3 bytes"));
    assert!(error.to_string().contains("byte read limit"));
    assert_eq!((budget.operations, budget.bytes), (1, 0));
    assert_eq!(
        budget
            .read(&game, "materials/three.vmt", usize::MAX, None)
            .unwrap()
            .unwrap(),
        b"abc"
    );
    let error = budget
        .read(&game, "materials/three.vmt", usize::MAX, None)
        .unwrap_err();
    assert!(error.to_string().contains("remaining 2 bytes"));
    assert!(error.to_string().contains("effective read limit 2 bytes"));
    assert_eq!((budget.operations, budget.bytes), (3, 3));
    assert!(
        budget
            .read(&game, "materials/missing.vmt", usize::MAX, None)
            .unwrap()
            .is_none()
    );
    assert_eq!((budget.operations, budget.bytes), (4, 3));
    assert_eq!(
        budget
            .read(&game, "materials/two.vmt", usize::MAX, None)
            .unwrap()
            .unwrap(),
        b"de"
    );
    let error = budget
        .read(&game, "materials/missing-after-full.vmt", usize::MAX, None)
        .unwrap_err();
    assert!(error.to_string().contains("aggregate limit 5 bytes"));
    assert_eq!((budget.operations, budget.bytes), (5, 5));
}

#[test]
fn missing_and_failed_payload_attempts_exhaust_the_lookup_budget_without_spending_bytes() {
    let (directory, game) = fixture();
    write(directory.path(), "materials/too_big.vmt", b"abcde");
    write(directory.path(), "materials/valid.vmt", b"test");
    let mut budget = ReadBudget::new(limits(4, 16, 3));
    assert!(
        budget
            .read(&game, "materials/missing.vmt", 4, None)
            .unwrap()
            .is_none()
    );
    assert!(
        budget
            .read(&game, "materials/too_big.vmt", 4, None)
            .is_err()
    );
    assert_eq!(
        budget
            .read(&game, "materials/valid.vmt", 4, None)
            .unwrap()
            .unwrap(),
        b"test"
    );
    let error = budget
        .read(&game, "materials/missing.vmt", 4, None)
        .unwrap_err();
    assert!(error.to_string().contains("3 source reads"));
    assert_eq!((budget.operations, budget.bytes), (3, 4));
}

#[test]
fn exact_vpk_limit_and_oversized_entry_refusal_keep_the_winning_mount() {
    for declared in [4, 4096] {
        let (directory, mut game) = fixture();
        write(
            directory.path(),
            "materials/payload.vmt",
            b"lower-priority bytes",
        );
        let index = directory.path().join("source_dir.vpk");
        vpk(&index, "vmt", "payload", b"test", declared);
        game.mounts.insert(
            0,
            MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Pak(Vpk::open(&index).unwrap()),
            },
        );
        let mut budget = ReadBudget::new(limits(4, 12, 4));
        if declared == 4 {
            assert_eq!(
                budget
                    .read(&game, "materials/payload.vmt", 4, None)
                    .unwrap()
                    .unwrap(),
                b"test"
            );
            let error = budget
                .read(&game, "materials/payload.vmt", 3, None)
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("VPK entry exceeds the caller's byte read limit")
            );
            assert_eq!((budget.operations, budget.bytes), (2, 4));
        } else {
            let error = budget
                .read(&game, "materials/payload.vmt", 4, None)
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("VPK entry exceeds the caller's byte read limit")
            );
            assert!(!error.to_string().contains("lies outside"));
            assert_eq!((budget.operations, budget.bytes), (1, 0));
        }
    }
}

#[test]
fn tracked_reads_intersect_limits_and_leave_no_observation_after_failed_io() {
    let (directory, game) = fixture();
    write(directory.path(), "materials/payload.vtf", b"test");
    let mut sources = SourceReads::default();
    let mut budget = ReadBudget::new(limits(8, 16, 8));
    assert!(
        budget
            .read(&game, "materials/payload.vtf", 3, Some(&mut sources))
            .is_err()
    );
    assert_eq!((budget.operations, budget.bytes), (1, 0));
    assert_eq!(
        budget
            .read(&game, "materials/payload.vtf", 4, Some(&mut sources))
            .unwrap()
            .unwrap(),
        b"test"
    );
    assert!(
        budget
            .read(&game, "materials/missing.vtf", 4, Some(&mut sources))
            .unwrap()
            .is_none()
    );
    let (observed, missing) = sources.into_parts();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].path, "materials/payload.vtf");
    assert_eq!(observed[0].bytes, 4);
    assert_eq!(
        observed[0].sha256,
        "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
    );
    assert_eq!(missing, ["materials/missing.vtf"]);
    assert_eq!((budget.operations, budget.bytes), (3, 4));

    let mut sources = SourceReads::default();
    sources
        .record("materials/payload.vtf", Some(b"old"))
        .unwrap();
    let mut budget = ReadBudget::new(limits(8, 16, 8));
    let error = budget
        .read(&game, "materials/payload.vtf", 8, Some(&mut sources))
        .unwrap_err();
    assert!(error.to_string().contains("effective read limit 3 bytes"));
    assert!(error.to_string().contains("byte read limit"));
    assert_eq!((budget.operations, budget.bytes), (1, 0));
    let (observed, missing) = sources.into_parts();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].bytes, 3);
    assert!(missing.is_empty());

    let mut sources = SourceReads::default();
    for _ in 0..16_384 {
        sources.record("materials/known_missing.vtf", None).unwrap();
    }
    let mut budget = ReadBudget::new(limits(8, 16, 8));
    let error = budget
        .read(&game, "materials/unread.vtf", 8, Some(&mut sources))
        .unwrap_err();
    assert!(error.to_string().contains("source lookup limit"));
    assert_eq!((budget.operations, budget.bytes), (0, 0));
    let (observed, missing) = sources.into_parts();
    assert!(observed.is_empty());
    assert_eq!(missing, ["materials/known_missing.vtf"]);
}

#[test]
fn a_post_read_freshness_rejection_still_spends_the_actual_payload_budget() {
    let (directory, game) = fixture();
    let path = "materials/payload.vtf";
    write(directory.path(), path, b"test");
    let mut sources = SourceReads::default();
    let mut budget = ReadBudget::new(limits(4, 8, 8));
    budget
        .read(&game, path, 4, Some(&mut sources))
        .unwrap()
        .unwrap();
    write(directory.path(), path, b"best");
    let error = budget.read(&game, path, 4, Some(&mut sources)).unwrap_err();
    assert!(error.to_string().contains("changed during baking"));
    assert_eq!((budget.operations, budget.bytes), (2, 8));
    let (observed, missing) = sources.into_parts();
    assert_eq!(observed.len(), 1);
    assert_eq!(
        observed[0].sha256,
        "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
    );
    assert!(missing.is_empty());
    let error = budget
        .read(&game, "materials/unread.vtf", 4, None)
        .unwrap_err();
    assert!(error.to_string().contains("aggregate limit 8 bytes"));
    assert_eq!((budget.operations, budget.bytes), (2, 8));
}

#[test]
fn patch_and_repeated_provider_reads_count_again_while_material_cache_hits_spend_nothing() {
    let (directory, game) = fixture();
    let base = "LightmappedGeneric { $basetexture shared }";
    let patch = "Patch { include materials/base.vmt }";
    write(directory.path(), "materials/base.vmt", base);
    write(directory.path(), "materials/patch.vmt", patch);
    write(
        directory.path(),
        "materials/shared.vtf",
        texture([0.125, 0.25, 0.75]),
    );
    let mut materials = Materials::new(&game);
    materials
        .track_prop_lighting_sources(PropSourceMode::WithReflectivity)
        .unwrap();
    materials.compile_reads = ReadBudget::new(limits(128, patch.len() + base.len() * 2, 3));
    materials.texture_reads = ReadBudget::new(limits(84, 168, 2));
    let first = materials.load("patch").unwrap();
    assert_eq!(materials.load(" PATCH ").unwrap(), first);
    assert_eq!(
        (
            materials.compile_reads.operations,
            materials.compile_reads.bytes
        ),
        (2, patch.len() + base.len())
    );
    assert_eq!(
        (
            materials.texture_reads.operations,
            materials.texture_reads.bytes
        ),
        (1, 84)
    );
    let second = materials.load("base").unwrap();
    assert_eq!(materials.load("BASE").unwrap(), second);
    assert_eq!(
        (
            materials.compile_reads.operations,
            materials.compile_reads.bytes
        ),
        (3, patch.len() + base.len() * 2)
    );
    assert_eq!(
        (
            materials.texture_reads.operations,
            materials.texture_reads.bytes
        ),
        (2, 168)
    );
    assert_eq!(
        materials
            .get(second)
            .texture_metrics()
            .unwrap()
            .reflectivity,
        [0.125, 0.25, 0.75]
    );
    let (sources, missing) = materials.take_prop_lighting_sources().unwrap().into_parts();
    assert_eq!(
        sources
            .iter()
            .map(|source| source.path.as_str())
            .collect::<Vec<_>>(),
        [
            "materials/base.vmt",
            "materials/patch.vmt",
            "materials/shared.vtf"
        ]
    );
    assert!(missing.is_empty());
    assert_eq!(
        sources.iter().map(|source| source.bytes).sum::<usize>(),
        patch.len() + base.len() + 84
    );
}

#[test]
fn patch_includes_and_later_names_share_the_vmt_operation_and_aggregate_limits() {
    let (directory, game) = fixture();
    let base = "LightmappedGeneric {}";
    let patch = "Patch { include materials/base.vmt }";
    write(directory.path(), "materials/base.vmt", base);
    write(directory.path(), "materials/patch.vmt", patch);
    for limited_reads in [false, true] {
        let mut materials = Materials::new(&game);
        materials.compile_reads = ReadBudget::new(limits(
            128,
            patch.len() + base.len() + usize::from(limited_reads),
            if limited_reads { 2 } else { 8 },
        ));
        materials.load("patch").unwrap();
        let error = materials.load("base").unwrap_err();
        assert!(error.contains("Material materials/base.vmt"), "{error}");
        assert!(
            error.contains(if limited_reads {
                "2 source reads"
            } else {
                "aggregate limit"
            }),
            "{error}"
        );
        assert_eq!(materials.iter().count(), 1);
        assert_eq!(
            (
                materials.compile_reads.operations,
                materials.compile_reads.bytes
            ),
            (2, patch.len() + base.len())
        );
    }
}

#[test]
fn optional_missing_names_stay_cached_and_required_missing_metrics_stay_unavailable() {
    let (directory, game) = fixture();
    let required = "LightmappedGeneric { $basetexture missing }";
    write(directory.path(), "materials/required.vmt", required);
    let mut materials = Materials::new(&game);
    materials.compile_reads = ReadBudget::new(limits(128, 128, 3));
    materials.texture_reads = ReadBudget::new(limits(84, 84, 1));
    let absent = materials.load("absent").unwrap();
    assert_eq!(materials.load("ABSENT").unwrap(), absent);
    assert!(!materials.get(absent).found);
    let neutral = materials.get(absent).texture_metrics().unwrap();
    assert_eq!(
        (neutral.width, neutral.height, neutral.reflectivity),
        (64, 64, [0.5; 3])
    );
    assert_eq!(
        (
            materials.compile_reads.operations,
            materials.compile_reads.bytes
        ),
        (1, 0)
    );
    let required_index = materials.load("required").unwrap();
    let error = materials.get(required_index).texture_metrics().unwrap_err();
    assert!(
        error.contains("required.vmt")
            && error.contains("missing.vtf")
            && error.contains("unavailable"),
        "{error}"
    );
    assert_eq!(
        (
            materials.texture_reads.operations,
            materials.texture_reads.bytes
        ),
        (1, 0)
    );
    assert!(materials.load("another_absent").is_ok());
    let error = materials.load("unread_absent").unwrap_err();
    assert!(error.contains("3 source reads"), "{error}");
    assert_eq!(
        (
            materials.compile_reads.operations,
            materials.compile_reads.bytes
        ),
        (3, required.len())
    );
    assert_eq!(materials.iter().count(), 3);
}

#[test]
fn independent_headers_share_a_budget_and_same_path_headers_are_read_once() {
    let (directory, game) = fixture();
    write(
        directory.path(),
        "materials/base.vtf",
        texture([0.125, 0.25, 0.75]),
    );
    write(
        directory.path(),
        "materials/tool.vtf",
        texture([0.75, 0.25, 0.125]),
    );
    write(
        directory.path(),
        "materials/split.vmt",
        "LightmappedGeneric { $basetexture base %tooltexture tool }",
    );
    write(
        directory.path(),
        "materials/same.vmt",
        "LightmappedGeneric { $basetexture BASE.vtf %tooltexture materials/base.vtf }",
    );
    for limited_reads in [false, true] {
        let mut split = Materials::new(&game);
        split.texture_reads = ReadBudget::new(limits(
            84,
            if limited_reads { 168 } else { 167 },
            if limited_reads { 1 } else { 4 },
        ));
        let error = split.load("split").unwrap_err();
        assert!(error.contains("Material materials/split.vmt"), "{error}");
        assert!(
            error.contains("$basetexture") && error.contains("materials/base.vtf"),
            "{error}"
        );
        if limited_reads {
            assert!(error.contains("1 source reads"), "{error}");
        } else {
            assert!(
                error.contains("remaining 83 bytes") && error.contains("byte read limit"),
                "{error}"
            );
        }
        assert_eq!(
            (split.texture_reads.operations, split.texture_reads.bytes),
            (if limited_reads { 1 } else { 2 }, 84)
        );
        assert_eq!(split.iter().count(), 0);
    }
    let mut same = Materials::new(&game);
    same.texture_reads = ReadBudget::new(limits(84, 84, 1));
    let index = same.load("same").unwrap();
    let metrics = same.get(index).texture_metrics().unwrap();
    assert_eq!(metrics.dimension_source, DimensionSource::ToolTextureHeader);
    assert_eq!(
        metrics.reflectivity_source,
        ReflectivitySource::TextureHeader
    );
    assert_eq!(metrics.reflectivity, [0.125, 0.25, 0.75]);
    assert_eq!(
        (same.texture_reads.operations, same.texture_reads.bytes),
        (1, 84)
    );
}

#[test]
fn deferred_and_unused_provider_paths_do_not_spend_the_metric_budget() {
    let (directory, game) = fixture();
    for (name, text) in [
        ("ordinary", "LightmappedGeneric { $basetexture oversized }"),
        (
            "nodraw",
            "LightmappedGeneric { %compilenodraw 1 $basetexture oversized }",
        ),
        (
            "sky",
            "UnlitGeneric { %compile2dsky 1 $basetexture oversized }",
        ),
        (
            "authored",
            "LightmappedGeneric { %notooltexture 1 $basetexture oversized $reflectivity \"[0.1 0.2 0.3]\" }",
        ),
    ] {
        write(directory.path(), &format!("materials/{name}.vmt"), text);
    }
    write(
        directory.path(),
        "materials/oversized.vtf",
        b"deliberately invalid unused texture",
    );
    let mut geometry = Materials::with_requirements(&game, Requirements::GeometrySemantics);
    geometry.texture_reads = ReadBudget::new(limits(0, 0, 0));
    let index = geometry.load("ordinary").unwrap();
    assert_eq!(geometry.get(index).metrics, MetricState::Deferred);
    assert_eq!(
        (
            geometry.geometry_reads.operations,
            geometry.compile_reads.operations,
            geometry.texture_reads.operations
        ),
        (1, 0, 0)
    );
    let mut compile = Materials::new(&game);
    compile.texture_reads = ReadBudget::new(limits(0, 0, 0));
    for name in ["nodraw", "sky", "authored"] {
        assert!(compile.load(name).is_ok(), "{name}");
    }
    assert_eq!(
        (
            compile.texture_reads.operations,
            compile.texture_reads.bytes
        ),
        (0, 0)
    );
    let error = compile.load("ordinary").unwrap_err();
    assert!(
        error.contains("$basetexture") && error.contains("0 source reads"),
        "{error}"
    );
}

#[test]
fn prop_material_file_limit_stays_stricter_than_the_ordinary_compile_limit() {
    let (directory, game) = fixture();
    let mut exact = b"LightmappedGeneric {}\n//".to_vec();
    exact.resize(super::super::MAX_GEOMETRY_MATERIAL_BYTES, b'x');
    let mut over = exact.clone();
    over.push(b'x');
    write(directory.path(), "materials/exact.vmt", &exact);
    write(directory.path(), "materials/over.vmt", &over);
    let mut ordinary = Materials::new(&game);
    assert!(ordinary.load("over").is_ok());
    assert_eq!(ordinary.compile_reads.bytes, over.len());
    for mode in [PropSourceMode::Direct, PropSourceMode::WithReflectivity] {
        let mut tracked = Materials::new(&game);
        tracked.track_prop_lighting_sources(mode).unwrap();
        tracked.load("exact").unwrap();
        let error = tracked.load("over").unwrap_err();
        assert!(error.contains("Material materials/over.vmt"), "{error}");
        assert!(
            error.contains("effective read limit 262144 bytes")
                && error.contains("byte read limit"),
            "{error}"
        );
        assert_eq!(
            (
                tracked.compile_reads.operations,
                tracked.compile_reads.bytes
            ),
            (2, exact.len())
        );
        let (sources, missing) = tracked.take_prop_lighting_sources().unwrap().into_parts();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].path, "materials/exact.vmt");
        assert_eq!(sources[0].bytes, exact.len());
        assert!(missing.is_empty());
    }
}
