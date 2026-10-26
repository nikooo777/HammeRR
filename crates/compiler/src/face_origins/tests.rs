use super::*;
use crate::{Control, Report, instances, test_common};

fn prepared(vmf: &Vmf) -> FaceOrigins {
    let expanded =
        instances::prepare(vmf, None, &Control::default(), &mut Report::default()).unwrap();
    FaceOrigins::prepare(vmf, &expanded).unwrap()
}

#[test]
fn split_and_merged_contributors_use_retained_ids_and_validate_channel_geometry() {
    let mut vmf = test_common::room();
    for (index, side) in vmf
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
        .enumerate()
    {
        side.id = 900_000 - index as u32 * 7;
    }
    let solid = &vmf.world.solids[0];
    let a = solid.sides[0].id;
    let b = solid.sides[1].id;
    let mut origins = prepared(&vmf);
    origins.reserve_faces(4, 4).unwrap();
    let faces = [
        bsp::Face {
            first_edge: 11,
            ..Default::default()
        },
        bsp::Face {
            first_edge: 22,
            ..Default::default()
        },
        bsp::Face {
            first_edge: 33,
            ..Default::default()
        },
        bsp::Face {
            first_edge: 44,
            ..Default::default()
        },
    ];
    origins.push(&faces[0], 0, &[a]).unwrap();
    origins.push(&faces[1], 0, &[a, b]).unwrap();
    origins.push(&faces[2], 1, &[b]).unwrap();
    origins.push(&faces[3], 0, &[]).unwrap();
    let expected = |side| Origin::Root {
        entity: vmf.world.id,
        solid: solid.id,
        side,
    };
    assert_eq!(
        origins
            .contributors(0)
            .unwrap()
            .cloned()
            .collect::<Vec<_>>(),
        [expected(a)]
    );
    assert_eq!(
        origins
            .contributors(1)
            .unwrap()
            .cloned()
            .collect::<Vec<_>>(),
        [expected(a), expected(b)]
    );
    assert_eq!(
        origins
            .contributors(2)
            .unwrap()
            .cloned()
            .collect::<Vec<_>>(),
        [expected(b)]
    );
    assert_eq!(origins.model(2), Some(1));
    assert_eq!(origins.contributors(3).unwrap().len(), 0);
    let mut bsp = Bsp::new();
    bsp.set_lighting_faces(LightingKind::Ldr, &faces).unwrap();
    let mut hdr = faces;
    hdr[0].light_offset = 64;
    hdr[0].styles = [0, 32, 255, 255];
    hdr[0].texinfo = 3;
    hdr[0].lightmap_size = [8, 16];
    bsp.set_lighting_faces(LightingKind::Hdr, &hdr).unwrap();
    origins.seal_geometry(&bsp).unwrap();
    origins.validate_channel(&bsp, LightingKind::Hdr).unwrap();
    hdr.swap(0, 1);
    bsp.set_lighting_faces(LightingKind::Hdr, &hdr).unwrap();
    assert!(origins.validate_channel(&bsp, LightingKind::Hdr).is_err());
    assert!(origins.push(&faces[0], 0, &[123]).is_err());
}

#[test]
fn mapping_limits_reject_before_reserving_face_or_reference_arrays() {
    assert!(prepared(&test_common::room()).bytes() < instances::MAX_OCCURRENCE_BYTES);
    for (faces, references) in [
        (MAX_FACES + 1, 0),
        (0, MAX_REFERENCES + 1),
        (usize::MAX, usize::MAX),
    ] {
        let mut origins = prepared(&test_common::room());
        assert!(origins.reserve_faces(faces, references).is_err());
        assert_eq!(origins.faces.capacity(), 0);
        assert_eq!(origins.references.capacity(), 0);
    }
    let mut origins = prepared(&test_common::room());
    origins.bytes = MAX_BYTES - 1;
    assert!(origins.reserve_faces(1, 1).is_err());
    assert_eq!(origins.faces.capacity(), 0);
    assert_eq!(origins.references.capacity(), 0);
}

#[test]
fn emitted_world_and_brush_models_retain_source_owners_at_exact_model_indices() {
    use glam::DVec3;
    use map_core::{Document, ObjectId, edit};
    let mut document = Document::new(test_common::room());
    document
        .edit("brush models", |tx| {
            for (x, classname) in [(-96.0, "func_brush"), (64.0, "func_detail")] {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(x, -32.0, 16.0),
                    DVec3::new(x + 32.0, 32.0, 64.0),
                    "dev/test",
                )?
                else {
                    unreachable!()
                };
                edit::tie_to_entity(tx, &[id], classname, &[])?;
            }
            Ok(())
        })
        .unwrap();
    let vmf = document.vmf();
    let output = crate::compile(vmf, &test_common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let origins = output.face_origins.as_ref().unwrap();
    let bsp = output.bsp.as_ref().unwrap();
    origins.validate_channel(bsp, LightingKind::Ldr).unwrap();
    for entity in vmf
        .entities
        .iter()
        .filter(|entity| !entity.solids.is_empty())
    {
        let model = if entity.classname() == Some("func_detail") {
            0
        } else {
            1
        };
        let matches: Vec<_> = (0..origins.len()).filter(|&face| origins.contributors(face).unwrap().any(|origin| matches!(origin, Origin::Root { entity: owner, .. } if *owner == entity.id))).collect();
        assert!(!matches.is_empty(), "{}", entity.id);
        assert!(
            matches
                .iter()
                .all(|&face| origins.model(face) == Some(model))
        );
    }
    assert_eq!(origins.len(), bsp.records::<bsp::Face>().unwrap().len());
    let mut changed = Bsp::parse(&bsp.to_bytes()).unwrap();
    changed.lumps[bsp::lump::VERTEXES].data[0] ^= 1;
    assert!(
        origins
            .validate_channel(&changed, LightingKind::Ldr)
            .is_err()
    );
}

#[test]
fn unavailable_inspection_metadata_preserves_successful_bsp_publication() {
    let mut output = crate::compile(&test_common::room(), &test_common::empty_game());
    let bytes = output.bsp.as_ref().unwrap().to_bytes();
    output.face_origins = Err("Authored face mapping metadata budget exhausted".into());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("owned.bsp");
    assert!(output.write(&path).unwrap());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    assert!(!output.report.has_errors());
}

#[test]
fn nested_same_id_leaves_keep_file_scopes_and_repeated_authored_sources() {
    use glam::DVec3;
    use map_core::{Document, edit};
    use source_formats::vmf::Entity;
    use std::collections::{BTreeMap, BTreeSet};

    let directory = tempfile::tempdir().unwrap();
    let mut leaf = Document::empty();
    leaf.edit("leaf box", |tx| {
        edit::create_block(
            tx,
            DVec3::new(-8.0, -8.0, 16.0),
            DVec3::new(8.0, 8.0, 32.0),
            "dev/instance_first",
        )?;
        Ok(())
    })
    .unwrap();
    let first = leaf.into_vmf();
    let mut second = first.clone();
    for side in &mut second.world.solids[0].sides {
        side.material = "dev/instance_second".into();
    }
    let mut middle = Document::empty().into_vmf();
    for (id, file, origin) in [
        (100, "first.vmf", "-48 0 0"),
        (101, "second.vmf", "0 0 0"),
        (102, "first.vmf", "48 0 0"),
    ] {
        let mut helper = Entity::new(id, "func_instance");
        helper.properties.set("file", file);
        helper.properties.set("origin", origin);
        middle.entities.push(helper);
    }
    let mut parent = test_common::room();
    let owner = parent.object_ids().max().unwrap() + 1;
    let mut helper = Entity::new(owner, "func_instance");
    helper.properties.set("file", "middle.vmf");
    parent.entities.push(helper);
    for (file, vmf) in [
        ("first.vmf", &first),
        ("second.vmf", &second),
        ("middle.vmf", &middle),
        ("parent.vmf", &parent),
    ] {
        std::fs::write(directory.path().join(file), vmf.write(&Default::default())).unwrap();
    }
    let control = Control::default();
    let context =
        instances::SourceContext::capture(&directory.path().join("parent.vmf"), &control).unwrap();
    let output = crate::compile_with_source_controlled(
        &parent,
        &test_common::empty_game(),
        crate::Options::default(),
        None,
        Some(&context),
        &control,
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let stats = output.stats.instances.as_ref().unwrap();
    assert_eq!(
        (stats.instances, stats.files, stats.max_expansion_depth),
        (4, 3, 2)
    );
    let origins = output.face_origins.as_ref().unwrap();
    let bsp = output.bsp.as_ref().unwrap();
    origins.validate_channel(bsp, LightingKind::Ldr).unwrap();
    let original_solid = first.world.solids[0].id;
    let original_sides = first.world.solids[0]
        .sides
        .iter()
        .map(|side| side.id)
        .collect::<BTreeSet<_>>();
    let mut references = BTreeMap::<(String, u32), usize>::new();
    let mut occurrences = BTreeMap::<Vec<(u32, String)>, BTreeSet<u32>>::new();
    let mut paths = BTreeMap::new();
    for face in 0..origins.len() {
        for origin in origins.contributors(face).unwrap() {
            if let Origin::Instance {
                instance,
                file,
                solid,
                side,
                occurrence,
            } = origin
            {
                assert_eq!(*instance, owner);
                assert!(
                    matches!(file.as_str(), "first.vmf" | "second.vmf"),
                    "{origin:?}"
                );
                assert_eq!(*solid, original_solid);
                assert!(original_sides.contains(side), "{origin:?}");
                let route = occurrence
                    .steps()
                    .map(|step| (step.helper, step.file.clone()))
                    .collect::<Vec<_>>();
                assert_eq!(route.len(), 2);
                assert!(matches!(route[1].0, 100..=102));
                assert_eq!(route[0], (owner, "middle.vmf".into()));
                assert_eq!(route[1].1, *file);
                occurrences.entry(route.clone()).or_default().insert(*side);
                if let Some(path) = paths.insert(route, occurrence) {
                    assert!(std::sync::Arc::ptr_eq(path, occurrence));
                }
                *references.entry((file.clone(), *side)).or_default() += 1;
            }
        }
    }
    assert_eq!(occurrences.len(), 3);
    for (helper, file) in [(100, "first.vmf"), (101, "second.vmf"), (102, "first.vmf")] {
        assert_eq!(
            occurrences[&vec![(owner, "middle.vmf".into()), (helper, file.into())]],
            original_sides
        );
    }
    for side in original_sides {
        assert!(
            references
                .get(&("first.vmf".into(), side))
                .copied()
                .unwrap_or(0)
                >= 2
        );
        assert!(
            references
                .get(&("second.vmf".into(), side))
                .copied()
                .unwrap_or(0)
                >= 1
        );
    }
    output
        .instance_dependencies
        .as_ref()
        .unwrap()
        .verify(&control)
        .unwrap();
}
