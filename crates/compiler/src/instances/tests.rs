use super::*;
use crate::{Options, compile_with_source_controlled, test_common};
use glam::DMat3;
use map_core::edit;
use source_formats::{
    gamefs::{GameFs, Mount, MountedPath},
    keyvalues::Node,
    vmf::format_vec3,
};

#[path = "terrain_tests.rs"]
mod terrain_tests;

fn nested_helper(id: u32, file: &str, origin: &str, angles: &str) -> Entity {
    let mut helper = Entity::new(id, "func_instance");
    helper.properties.set("file", file);
    helper.properties.set("origin", origin);
    helper.properties.set("angles", angles);
    helper
}

fn helper_file(entities: Vec<Entity>) -> Vmf {
    let mut vmf = Document::empty().into_vmf();
    vmf.entities = entities;
    vmf
}

fn write_child(directory: &Path, name: &str, child: &Vmf) {
    std::fs::write(directory.join(name), child.write(&Default::default())).unwrap();
}

fn manual_flat_brushes(parent: &Vmf, brushes: &[(&Solid, bool, DVec3)]) -> Vmf {
    let mut document = Document::new(parent.clone());
    document
        .edit("literal flat control", |tx| {
            for (original, detail, translation) in brushes {
                let mut solid = manual_solid(original, *translation, DVec3::ZERO);
                solid.id = tx.new_object_id()?;
                for side in &mut solid.sides {
                    side.id = tx.new_side_id()?;
                }
                if *detail {
                    let mut entity = Entity::new(tx.new_object_id()?, "func_detail");
                    entity.solids.push(solid);
                    tx.add_entity(entity)?;
                } else {
                    tx.add_solid(solid)?;
                }
            }
            for helper in parent.entities.iter().filter(|entity| {
                entity
                    .classname()
                    .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
            }) {
                tx.remove(ObjectId::Entity(helper.id))?;
            }
            Ok(())
        })
        .unwrap();
    document.into_vmf()
}

#[test]
fn nested_noncommuting_transforms_preserve_literal_points_normals_and_attached_uvs() {
    for (angles, outer_matrix) in [
        ("0 90 0", DMat3::from_cols(DVec3::Y, -DVec3::X, DVec3::Z)),
        (
            "20 35 10",
            DMat3::from_cols(
                DVec3::new(0.7697511313200572, 0.5389855446957562, -0.3420201433256687),
                DVec3::new(-0.5162121193659357, 0.8407726623973337, 0.16317591116653482),
                DVec3::new(
                    0.37551064385876115,
                    0.050_950_100_827_363_06,
                    0.9254165783983234,
                ),
            ),
        ),
    ] {
        let (directory, mut parent, child, _) = fixture();
        let root = parent.entities.last_mut().unwrap();
        root.properties.set("file", "middle.vmf");
        root.properties.set("origin", "80.125 16.375 8.0625");
        root.properties.set("angles", angles);
        let owner = root.id;
        write_child(
            directory.path(),
            "middle.vmf",
            &helper_file(vec![nested_helper(
                100,
                "child.vmf",
                "4.5 -6.25 12.125",
                "90 0 0",
            )]),
        );
        let before = parent.clone();
        let child_bytes = std::fs::read(directory.path().join("child.vmf")).unwrap();
        let parent_bytes = std::fs::read(directory.path().join("parent.vmf")).unwrap();
        let context = source(directory.path());
        let mut report = Report::default();
        let prepared = prepare(&parent, Some(&context), &Control::default(), &mut report).unwrap();
        let stats = prepared.stats().unwrap();
        assert_eq!(
            (stats.instances, stats.files, stats.max_expansion_depth),
            (2, 2, 2)
        );
        assert_eq!((stats.world_brushes, stats.detail_brushes), (1, 1));
        let generated = prepared.generated_brushes().collect::<Vec<_>>();
        assert_eq!(generated.len(), 2);
        for (actual, original) in generated.iter().zip(child.solids()) {
            assert_eq!(
                (actual.owner, actual.file, actual.original),
                (owner, "child.vmf", original.id)
            );
            for (side, original_side) in actual.solid.sides.iter().zip(&original.sides) {
                for (point, original_point) in side.plane.0.iter().zip(original_side.plane.0) {
                    let inner = DVec3::new(
                        original_point.z + 4.5,
                        original_point.y - 6.25,
                        -original_point.x + 12.125,
                    );
                    let expected = outer_matrix * inner + DVec3::new(80.125, 16.375, 8.0625);
                    assert!(
                        point.abs_diff_eq(expected, 1e-8),
                        "{angles}: {point:?} != {expected:?}"
                    );
                    assert!(
                        map_core::texel_uv(side, expected)
                            .abs_diff_eq(map_core::texel_uv(original_side, original_point), 1e-8)
                    );
                }
                let normal = map_core::Plane::from_points(original_side.plane.0)
                    .unwrap()
                    .normal;
                let expected = outer_matrix * DVec3::new(normal.z, normal.y, -normal.x);
                let actual = map_core::Plane::from_points(side.plane.0).unwrap().normal;
                assert!(
                    actual.abs_diff_eq(expected, 1e-8),
                    "{angles}: {actual:?} != {expected:?}"
                );
                assert_eq!(
                    prepared.face_origin(side.id).unwrap(),
                    (owner, "child.vmf", original.id, original_side.id),
                );
            }
        }
        prepared.vmf.validate_ids().unwrap();
        assert_eq!(parent, before);
        assert_eq!(
            std::fs::read(directory.path().join("parent.vmf")).unwrap(),
            parent_bytes
        );
        assert_eq!(
            std::fs::read(directory.path().join("child.vmf")).unwrap(),
            child_bytes
        );
    }
}

#[test]
fn nested_authored_order_and_outside_markers_match_a_literal_flat_region_control() {
    let (directory, mut parent, mut leaf, game) = fixture();
    for solid in leaf.world.solids.iter_mut().chain(
        leaf.entities
            .iter_mut()
            .flat_map(|entity| &mut entity.solids),
    ) {
        for side in &mut solid.sides {
            for point in &mut side.plane.0 {
                point.x -= 512.0;
            }
        }
    }
    write_child(directory.path(), "child.vmf", &leaf);
    let mut middle = leaf.clone();
    middle
        .entities
        .insert(0, nested_helper(10_000, "child.vmf", "0 64 0", "0 0 0"));
    write_child(directory.path(), "middle.vmf", &middle);
    let root = parent.entities.last_mut().unwrap();
    root.properties.set("file", "middle.vmf");
    root.properties.set("origin", "512 0 0");
    parent.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-128 -128 0"),
            Node::string("maxs", "128 128 256"),
        ],
    ));
    let expected = manual_flat_brushes(
        &parent,
        &[
            (&middle.world.solids[0], false, DVec3::new(512.0, 0.0, 0.0)),
            (&leaf.world.solids[0], false, DVec3::new(512.0, 64.0, 0.0)),
            (
                &leaf.entities[0].solids[0],
                true,
                DVec3::new(512.0, 64.0, 0.0),
            ),
            (
                &middle.entities[1].solids[0],
                true,
                DVec3::new(512.0, 0.0, 0.0),
            ),
        ],
    );
    let context = source(directory.path());
    let actual = compiled(&parent, &game, &context);
    let control = crate::compile(&expected, &game);
    assert!(!actual.report.has_errors(), "{:?}", actual.report);
    assert!(!control.report.has_errors(), "{:?}", control.report);
    assert_eq!(
        actual.bsp.as_ref().unwrap().to_bytes(),
        control.bsp.as_ref().unwrap().to_bytes()
    );
    assert!(actual.stats.compile_region.is_some());
    let stats = actual.stats.instances.as_ref().unwrap();
    assert_eq!(
        (stats.instances, stats.files, stats.max_expansion_depth),
        (2, 2, 2)
    );
    assert_eq!((stats.world_brushes, stats.detail_brushes), (2, 2));
}

#[test]
fn cached_shared_leaves_and_repeated_siblings_keep_root_owners_and_unique_generated_ids() {
    let (directory, mut parent, child, _) = fixture();
    write_child(
        directory.path(),
        "a.vmf",
        &helper_file(vec![
            nested_helper(100, "child.vmf", "-64 0 0", "0 0 0"),
            nested_helper(101, "child.vmf", "0 0 0", "0 0 0"),
        ]),
    );
    write_child(
        directory.path(),
        "b.vmf",
        &helper_file(vec![nested_helper(100, "child.vmf", "0 0 0", "0 0 0")]),
    );
    let first = parent.entities.last_mut().unwrap();
    first.properties.set("file", "a.vmf");
    first.properties.set("origin", "0 0 0");
    let first_owner = first.id;
    let second_owner = first_owner + 1;
    parent
        .entities
        .push(nested_helper(second_owner, "b.vmf", "64 0 0", "0 0 0"));
    let context = source(directory.path());
    let mut report = Report::default();
    let prepared = prepare(&parent, Some(&context), &Control::default(), &mut report).unwrap();
    prepared.vmf.validate_ids().unwrap();
    let stats = prepared.stats().unwrap();
    assert_eq!(
        (stats.instances, stats.files, stats.max_expansion_depth),
        (5, 3, 2)
    );
    assert_eq!((stats.world_brushes, stats.detail_brushes), (3, 3));
    assert_eq!(
        stats
            .dependencies
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["a.vmf", "b.vmf", "child.vmf"],
    );
    let brushes = prepared.generated_brushes().collect::<Vec<_>>();
    assert_eq!(
        brushes
            .iter()
            .filter(|brush| brush.owner == first_owner)
            .count(),
        4
    );
    assert_eq!(
        brushes
            .iter()
            .filter(|brush| brush.owner == second_owner)
            .count(),
        2
    );
    assert!(brushes.iter().all(|brush| brush.file == "child.vmf"));
    let original = child.world.solids[0].id;
    let first_origins = brushes
        .iter()
        .filter(|brush| brush.owner == first_owner && !brush.detail)
        .map(|brush| prepared.face_origin(brush.solid.sides[0].id).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        first_origins,
        vec![
            (
                first_owner,
                "child.vmf",
                original,
                child.world.solids[0].sides[0].id
            );
            2
        ]
    );
    let control = Control::default();
    prepared.dependencies().unwrap().verify(&control).unwrap();
}

#[cfg(unix)]
#[test]
fn nested_paths_stay_root_relative_through_a_saved_root_alias() {
    use std::os::unix::fs::symlink;
    let (directory, mut parent, child, _) = fixture();
    let modules = directory.path().join("modules");
    let physical = directory.path().join("physical");
    std::fs::create_dir(&modules).unwrap();
    std::fs::create_dir(&physical).unwrap();
    write_child(directory.path(), "leaf.vmf", &child);
    let mut wrong_leaf = child.clone();
    for solid in &mut wrong_leaf.world.solids {
        for side in &mut solid.sides {
            for point in &mut side.plane.0 {
                point.x += 64.0;
            }
        }
    }
    write_child(&modules, "leaf.vmf", &wrong_leaf);
    write_child(
        &modules,
        "middle.vmf",
        &helper_file(vec![nested_helper(100, "leaf.vmf", "0 0 0", "0 0 0")]),
    );
    let root = parent.entities.last_mut().unwrap();
    root.properties.set("file", r".\modules\middle.vmf");
    root.properties.set("origin", "0 0 0");
    assert_eq!(
        filename(r".\modules\middle.vmf").unwrap(),
        "modules/middle.vmf"
    );
    write_child(&physical, "root.vmf", &parent);
    let alias = directory.path().join("root-alias.vmf");
    symlink(physical.join("root.vmf"), &alias).unwrap();
    let context = SourceContext::capture(&alias, &Control::default()).unwrap();
    let prepared = resolve(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    let brushes = prepared.generated_brushes().collect::<Vec<_>>();
    assert_eq!(brushes.len(), 2);
    assert!(brushes.iter().all(|brush| brush.file == "leaf.vmf"));
    assert_eq!(
        brushes[0].solid.sides[0].plane.0,
        child.world.solids[0].sides[0].plane.0
    );
    assert_eq!(
        prepared
            .stats()
            .unwrap()
            .dependencies
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["leaf.vmf", "modules/middle.vmf"],
    );
    std::fs::write(
        modules.join("leaf.vmf"),
        b"unused child-relative decoy changed",
    )
    .unwrap();
    prepared
        .dependencies()
        .unwrap()
        .verify(&Control::default())
        .unwrap();
    let used = directory.path().join("leaf.vmf");
    let mut changed = std::fs::read(&used).unwrap();
    changed.extend_from_slice(b"\n// changed used root leaf\n");
    std::fs::write(used, changed).unwrap();
    assert!(
        prepared
            .dependencies()
            .unwrap()
            .verify(&Control::default())
            .is_err()
    );
}

#[test]
fn four_helper_levels_and_global_nested_occurrence_limits_are_independent() {
    let (directory, mut parent, _, _) = fixture();
    let names = ["one.vmf", "two.vmf", "three.vmf", "four.vmf"];
    for (index, name) in names.iter().enumerate() {
        let children = names
            .get(index + 1)
            .map(|next| vec![nested_helper(100, next, "0 0 0", "0 0 0")])
            .unwrap_or_default();
        write_child(directory.path(), name, &helper_file(children));
    }
    let root = parent.entities.last_mut().unwrap();
    root.properties.set("file", "one.vmf");
    root.properties.set("origin", "0 0 0");
    let context = source(directory.path());
    let prepared = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    let stats = prepared.stats().unwrap();
    assert_eq!(
        (stats.instances, stats.files, stats.max_expansion_depth),
        (4, 4, 4)
    );
    write_child(
        directory.path(),
        "four.vmf",
        &helper_file(vec![nested_helper(
            100,
            "unread-fifth.vmf",
            "0 0 0",
            "0 0 0",
        )]),
    );
    let error = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("4 helper levels"), "{error:?}");
    assert!(!error.message.contains("No such file"), "{error:?}");

    let mut middle = helper_file(
        (0..255)
            .map(|index| nested_helper(index + 2, "empty.vmf", "0 0 0", "0 0 0"))
            .collect(),
    );
    write_child(directory.path(), "empty.vmf", &helper_file(Vec::new()));
    write_child(directory.path(), "one.vmf", &middle);
    let prepared = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    let stats = prepared.stats().unwrap();
    assert_eq!(
        (stats.instances, stats.files, stats.max_expansion_depth),
        (256, 2, 2)
    );
    assert_eq!((stats.world_brushes, stats.detail_brushes), (0, 0));
    middle
        .entities
        .push(nested_helper(257, "empty.vmf", "0 0 0", "0 0 0"));
    write_child(directory.path(), "one.vmf", &middle);
    let error = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(
        error.message.contains("256 total helper occurrences"),
        "{error:?}"
    );
}

#[test]
fn nested_file_limit_is_global_and_checked_before_reading_an_extra_path() {
    let (directory, mut parent, _, _) = fixture();
    let mut middle = helper_file(Vec::new());
    for index in 0..31 {
        let file = format!("leaf{index}.vmf");
        write_child(directory.path(), &file, &helper_file(Vec::new()));
        middle
            .entities
            .push(nested_helper(index + 100, &file, "0 0 0", "0 0 0"));
    }
    write_child(directory.path(), "middle.vmf", &middle);
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("file", "middle.vmf");
    let context = source(directory.path());
    let prepared = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    let stats = prepared.stats().unwrap();
    assert_eq!(
        (stats.instances, stats.files, stats.max_expansion_depth),
        (32, 32, 2)
    );
    middle
        .entities
        .push(nested_helper(999, "unread-extra.vmf", "0 0 0", "0 0 0"));
    write_child(directory.path(), "middle.vmf", &middle);
    let error = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("32 unique files"), "{error:?}");
}

#[test]
fn active_cached_ancestors_reject_cycles_while_hidden_references_never_resolve() {
    let (directory, mut parent, _, game) = fixture();
    write_child(
        directory.path(),
        "a.vmf",
        &helper_file(vec![nested_helper(100, "b.vmf", "0 0 0", "0 0 0")]),
    );
    write_child(
        directory.path(),
        "b.vmf",
        &helper_file(vec![nested_helper(100, "a.vmf", "0 0 0", "0 0 0")]),
    );
    let root = parent.entities.last_mut().unwrap();
    root.properties.set("file", "a.vmf");
    let owner = root.id;
    let before = parent.clone();
    let context = source(directory.path());
    let output = compiled(&parent, &game, &context);
    assert!(output.bsp.is_none());
    let error = output.report.diagnostics.last().unwrap();
    assert_eq!(error.object, Some(format!("entity:{owner}")));
    assert!(
        error.message.contains("cycle") && error.message.contains("a.vmf"),
        "{error:?}"
    );
    assert_eq!(parent, before);

    let mut hidden = nested_helper(100, "../unread.vmf", "bad", "bad");
    hidden.hidden = true;
    write_child(
        directory.path(),
        "a.vmf",
        &helper_file(vec![hidden.clone()]),
    );
    let prepared = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    let stats = prepared.stats().unwrap();
    assert_eq!(
        (stats.instances, stats.files, stats.max_expansion_depth),
        (1, 1, 1)
    );
    assert_eq!(stats.dependencies[0].path, "a.vmf");
    hidden.hidden = false;
    hidden.properties.set("file", "child.vmf");
    hidden.properties.set("origin", "0 0 0");
    hidden.properties.set("angles", "0 0 0");
    hidden.properties.set("parentname", "unsupported");
    write_child(directory.path(), "a.vmf", &helper_file(vec![hidden]));
    let error = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(
        error.message.contains("a.vmf") && error.message.contains("Child instance 100"),
        "{error:?}"
    );
    assert!(error.message.contains("parentname"), "{error:?}");
}

#[cfg(unix)]
#[test]
fn nested_hard_link_cycles_reject_physical_ancestors_and_the_saved_root() {
    for root_alias in [false, true] {
        let (directory, mut parent, _, _) = fixture();
        let alias = if root_alias {
            "root-alias.vmf"
        } else {
            "a-alias.vmf"
        };
        write_child(
            directory.path(),
            "a.vmf",
            &helper_file(vec![nested_helper(100, alias, "0 0 0", "0 0 0")]),
        );
        std::fs::hard_link(
            directory
                .path()
                .join(if root_alias { "parent.vmf" } else { "a.vmf" }),
            directory.path().join(alias),
        )
        .unwrap();
        parent
            .entities
            .last_mut()
            .unwrap()
            .properties
            .set("file", "a.vmf");
        let context = source(directory.path());
        let error = prepare(
            &parent,
            Some(&context),
            &Control::default(),
            &mut Report::default(),
        )
        .err()
        .unwrap();
        assert!(
            error.message.contains("ancestor reference cycle"),
            "{error:?}"
        );
        assert!(error.message.contains(alias), "{error:?}");
        assert!(!error.message.contains("helper levels"), "{error:?}");
    }
}

#[test]
fn an_out_of_bounds_intermediate_rejects_even_when_the_final_translation_would_fit() {
    let (directory, mut parent, child, _) = fixture();
    write_child(
        directory.path(),
        "middle.vmf",
        &helper_file(vec![nested_helper(100, "child.vmf", "16360 0 0", "0 0 0")]),
    );
    let root = parent.entities.last_mut().unwrap();
    root.properties.set("file", "middle.vmf");
    root.properties.set("origin", "-16360 0 0");
    let before = parent.clone();
    let context = source(directory.path());
    let error = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("compile bounds"), "{error:?}");
    let expected = manual_flat_brushes(
        &parent,
        &[
            (&child.world.solids[0], false, DVec3::ZERO),
            (&child.entities[0].solids[0], true, DVec3::ZERO),
        ],
    );
    for solid in expected.solids() {
        assert!(checked_solid(solid).is_ok());
    }
    assert_eq!(parent, before);
}

#[test]
fn transitive_dependency_changes_after_staging_preserve_bsp_and_leak_outputs() {
    for leaking in [false, true] {
        for mutation in [
            "leaf_bytes",
            "leaf_inode",
            "middle_missing",
            "directory_rebind",
        ] {
            let (directory, mut parent, child, game) = fixture();
            let modules = directory.path().join("modules");
            std::fs::create_dir(&modules).unwrap();
            write_child(&modules, "leaf.vmf", &child);
            write_child(
                &modules,
                "middle.vmf",
                &helper_file(vec![nested_helper(
                    100,
                    "modules/leaf.vmf",
                    "0 0 0",
                    "0 0 0",
                )]),
            );
            let root = parent.entities.last_mut().unwrap();
            root.properties.set("file", "modules/middle.vmf");
            root.properties.set("origin", "0 0 0");
            if leaking {
                parent.world.solids.remove(0);
            }
            let before = parent.clone();
            let source_bytes = std::fs::read(directory.path().join("parent.vmf")).unwrap();
            let output = compiled(&parent, &game, &source(directory.path()));
            assert_eq!(output.bsp.is_some(), !leaking, "{:?}", output.report);
            assert_eq!(output.leak.is_some(), leaking, "{:?}", output.report);
            let guard = output.instance_dependencies.as_ref().unwrap();
            guard.verify(&Control::default()).unwrap();
            assert_eq!(
                output
                    .stats
                    .instances
                    .as_ref()
                    .unwrap()
                    .dependencies
                    .iter()
                    .map(|file| file.path.as_str())
                    .collect::<Vec<_>>(),
                ["modules/leaf.vmf", "modules/middle.vmf"],
            );
            let destination = directory.path().join("previous.bsp");
            let lin = destination.with_extension("lin");
            std::fs::write(&destination, b"previous complete BSP").unwrap();
            std::fs::write(&lin, b"previous complete LIN").unwrap();
            let control = Control::default();
            let lease_path = if leaking { &lin } else { &destination };
            let lease = crate::OutputLease::acquire(lease_path, &control).unwrap();
            let staged = output
                .prepare_write_controlled(&destination, &control)
                .unwrap()
                .unwrap();
            match mutation {
                "leaf_bytes" => {
                    let text = std::fs::read_to_string(modules.join("leaf.vmf")).unwrap();
                    let changed = text.replace("INSTANCE_WORLD", "INSTANCE_OTHER");
                    assert_eq!(changed.len(), text.len());
                    assert_ne!(changed, text);
                    std::fs::write(modules.join("leaf.vmf"), changed).unwrap();
                }
                "leaf_inode" => {
                    let bytes = std::fs::read(modules.join("leaf.vmf")).unwrap();
                    std::fs::write(modules.join("replacement.vmf"), bytes).unwrap();
                    std::fs::rename(modules.join("replacement.vmf"), modules.join("leaf.vmf"))
                        .unwrap();
                }
                "middle_missing" => std::fs::remove_file(modules.join("middle.vmf")).unwrap(),
                "directory_rebind" => {
                    let old = directory.path().join("old-modules");
                    std::fs::rename(&modules, &old).unwrap();
                    std::fs::create_dir(&modules).unwrap();
                    for name in ["leaf.vmf", "middle.vmf"] {
                        std::fs::copy(old.join(name), modules.join(name)).unwrap();
                    }
                }
                _ => unreachable!(),
            }
            assert!(
                guard.verify_metadata().is_err(),
                "{mutation}, leak={leaking}"
            );
            assert!(
                guard.verify(&Control::default()).is_err(),
                "{mutation}, leak={leaking}"
            );
            assert!(
                staged.commit_checked(&lease, &control).is_err(),
                "{mutation}, leak={leaking}"
            );
            assert_eq!(
                std::fs::read(&destination).unwrap(),
                b"previous complete BSP"
            );
            assert_eq!(std::fs::read(&lin).unwrap(), b"previous complete LIN");
            assert_eq!(
                std::fs::read(directory.path().join("parent.vmf")).unwrap(),
                source_bytes
            );
            assert_eq!(parent, before);
        }
    }
}

#[test]
fn accepted_cancellation_after_a_real_nested_branch_rolls_back_the_detached_transaction() {
    let (directory, mut parent, _, game) = fixture();
    write_child(
        directory.path(),
        "middle.vmf",
        &helper_file(vec![nested_helper(100, "child.vmf", "0 0 0", "0 0 0")]),
    );
    let first = parent.entities.last_mut().unwrap();
    first.properties.set("file", "middle.vmf");
    first.properties.set("origin", "0 0 0");
    let second = nested_helper(first.id + 1, "middle.vmf", "64 0 0", "0 0 0");
    parent.entities.push(second);
    let before = parent.clone();
    let old_solids = parent.solids().count();
    let source_bytes = std::fs::read(directory.path().join("parent.vmf")).unwrap();
    let child_bytes = std::fs::read(directory.path().join("child.vmf")).unwrap();
    let destination = directory.path().join("previous.bsp");
    std::fs::write(&destination, b"previous complete BSP").unwrap();
    let context = source(directory.path());
    let control = Control::default();
    let output = compiled(&parent, &game, &context);
    assert!(output.bsp.is_some(), "{:?}", output.report);
    let lease = crate::OutputLease::acquire(&destination, &control).unwrap();
    let staged = output
        .prepare_write_controlled(&destination, &control)
        .unwrap()
        .unwrap();
    let mut report = Report::default();
    let mut expansion = expand::Expansion::new(&parent, &context, &control, &mut report);
    let mut document = Document::new(parent.clone());
    let helpers = &parent.entities[parent.entities.len() - 2..];
    let result = document.edit("cancel real nested expansion", |tx| {
        expansion.helper(
            tx,
            &helpers[0],
            &helpers[0],
            &mut Vec::with_capacity(MAX_EXPANSION_DEPTH),
            &mut vec![context.root.clone()],
        )?;
        assert_eq!(tx.vmf().solids().count(), old_solids + 2);
        control.cancel().unwrap();
        expansion.helper(
            tx,
            &helpers[1],
            &helpers[1],
            &mut Vec::with_capacity(MAX_EXPANSION_DEPTH),
            &mut vec![context.root.clone()],
        )
    });
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    let progress = expansion.finish();
    assert_eq!(
        (
            progress.stats.instances,
            progress.stats.files,
            progress.stats.max_expansion_depth
        ),
        (2, 2, 2)
    );
    assert_eq!(
        (progress.stats.world_brushes, progress.stats.detail_brushes),
        (1, 1)
    );
    assert_eq!(document.vmf(), &before);
    assert_eq!(parent, before);
    assert_eq!(
        std::fs::read(directory.path().join("parent.vmf")).unwrap(),
        source_bytes
    );
    assert_eq!(
        std::fs::read(directory.path().join("child.vmf")).unwrap(),
        child_bytes
    );
    assert!(staged.commit_checked(&lease, &control).is_err());
    assert_eq!(
        std::fs::read(destination).unwrap(),
        b"previous complete BSP"
    );
}

fn fixture() -> (tempfile::TempDir, Vmf, Vmf, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    let mut child = Document::empty();
    child
        .edit("child geometry", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-32.0, -24.0, 16.0),
                DVec3::new(16.0, 24.0, 48.0),
                "dev/instance_world",
            )?;
            let ObjectId::Solid(detail) = edit::create_block(
                tx,
                DVec3::new(24.0, 16.0, 16.0),
                DVec3::new(56.0, 40.0, 32.0),
                "dev/instance_detail",
            )?
            else {
                unreachable!()
            };
            edit::tie_to_entity(tx, &[detail], "func_detail", &[])?;
            Ok(())
        })
        .unwrap();
    let mut child = child.into_vmf();
    for solid in child.world.solids.iter_mut().chain(
        child
            .entities
            .iter_mut()
            .flat_map(|entity| &mut entity.solids),
    ) {
        for side in &mut solid.sides {
            side.u_axis.scale = 0.375;
            side.v_axis.scale = -0.625;
            side.u_axis.offset = 3.125;
            side.v_axis.offset = -9.75;
        }
    }
    std::fs::write(
        directory.path().join("child.vmf"),
        child.write(&Default::default()),
    )
    .unwrap();
    let mut parent = test_common::room();
    let id = parent.object_ids().max().unwrap() + 1;
    let mut helper = Entity::new(id, "FuNc_InStAnCe");
    helper.properties.set("file", "child.vmf");
    helper.properties.set("origin", "80 16 16");
    parent.entities.push(helper);
    std::fs::write(
        directory.path().join("parent.vmf"),
        parent.write(&Default::default()),
    )
    .unwrap();
    std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsblack.vmt"),
        "UnlitGeneric {} ",
    )
    .unwrap();
    let mut game = test_common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, parent, child, game)
}

fn source(directory: &Path) -> SourceContext {
    SourceContext::capture(&directory.join("parent.vmf"), &Control::default()).unwrap()
}

fn compiled(parent: &Vmf, game: &GameFs, source: &SourceContext) -> crate::Output {
    compile_with_source_controlled(
        parent,
        game,
        Options::default(),
        None,
        Some(source),
        &Control::default(),
    )
}

fn source_matrix(angles: DVec3) -> DMat3 {
    let angles = angles.map(f64::to_radians);
    let (sp, cp) = angles.x.sin_cos();
    let (sy, cy) = angles.y.sin_cos();
    let (sr, cr) = angles.z.sin_cos();
    DMat3::from_cols(
        DVec3::new(cp * cy, cp * sy, -sp),
        DVec3::new(sp * sr * cy - cr * sy, sp * sr * sy + cr * cy, sr * cp),
        DVec3::new(sp * cr * cy + sr * sy, sp * cr * sy - sr * cy, cr * cp),
    )
}

fn manual_solid(original: &Solid, origin: DVec3, angles: DVec3) -> Solid {
    let matrix = source_matrix(angles);
    let mut solid = original.clone();
    for side in &mut solid.sides {
        for point in &mut side.plane.0 {
            *point = matrix * *point + origin;
        }
        for axis in [&mut side.u_axis, &mut side.v_axis] {
            axis.axis = matrix * axis.axis;
            axis.offset -= origin.dot(axis.axis) / axis.scale;
        }
    }
    solid
}

fn manual_parent(parent: &Vmf, child: &Vmf) -> Vmf {
    let mut expected = Document::new(parent.clone());
    expected
        .edit("manual flatten", |tx| {
            for helper in parent.entities.iter().filter(|entity| {
                entity
                    .classname()
                    .is_some_and(|name| name.eq_ignore_ascii_case("func_instance"))
            }) {
                let origin = vector(helper, "origin").unwrap();
                let angles = vector(helper, "angles").unwrap();
                for (original, detail) in
                    child.world.solids.iter().map(|solid| (solid, false)).chain(
                        child
                            .entities
                            .iter()
                            .flat_map(|entity| entity.solids.iter().map(|solid| (solid, true))),
                    )
                {
                    let mut solid = manual_solid(original, origin, angles);
                    solid.id = tx.new_object_id()?;
                    for side in &mut solid.sides {
                        side.id = tx.new_side_id()?;
                    }
                    if detail {
                        let mut entity = Entity::new(tx.new_object_id()?, "func_detail");
                        entity.solids.push(solid);
                        tx.add_entity(entity)?;
                    } else {
                        tx.add_solid(solid)?;
                    }
                }
            }
            for helper in parent.entities.iter().filter(|entity| {
                entity
                    .classname()
                    .is_some_and(|name| name.eq_ignore_ascii_case("func_instance"))
            }) {
                tx.remove(ObjectId::Entity(helper.id))?;
            }
            Ok(())
        })
        .unwrap();
    expected.into_vmf()
}

#[test]
fn source_matrix_uvs_detail_and_repeated_ids_match_manual_flattening() {
    let (directory, mut parent, child, game) = fixture();
    let first = parent.entities.last_mut().unwrap();
    first.properties.set("angles", "17 39 -23");
    let mut second = first.clone();
    second.id += 1;
    second.properties.set("origin", "-96 32 8");
    second.properties.set("angles", "-11 71 13");
    parent.entities.push(second);
    let context = source(directory.path());
    let before = parent.clone();
    let mut report = Report::default();
    let expanded = prepare(&parent, Some(&context), &Control::default(), &mut report).unwrap();
    let manual = manual_parent(&parent, &child);
    expanded.vmf.validate_ids().unwrap();
    let stats = expanded.stats.as_ref().unwrap();
    assert_eq!(
        (
            stats.instances,
            stats.files,
            stats.world_brushes,
            stats.detail_brushes
        ),
        (2, 1, 2, 2)
    );
    for (actual, expected) in expanded.vmf.solids().zip(manual.solids()) {
        let actual_geometry = BrushGeometry::from_solid(actual).unwrap();
        let expected_geometry = BrushGeometry::from_solid(expected).unwrap();
        assert!(
            actual_geometry
                .mins
                .abs_diff_eq(expected_geometry.mins, 1e-8)
        );
        assert!(
            actual_geometry
                .maxs
                .abs_diff_eq(expected_geometry.maxs, 1e-8)
        );
        for (side, expected_side) in actual.sides.iter().zip(&expected.sides) {
            for (point, expected_point) in side.plane.0.iter().zip(expected_side.plane.0) {
                assert!(point.abs_diff_eq(expected_point, 1e-8));
            }
            for corner in expected_geometry
                .faces
                .iter()
                .flat_map(|face| &face.vertices)
            {
                assert!(
                    map_core::texel_uv(side, *corner)
                        .abs_diff_eq(map_core::texel_uv(expected_side, *corner), 1e-8)
                );
            }
        }
    }
    let actual = compiled(&parent, &game, &context);
    let expected = crate::compile(&manual, &game);
    assert!(actual.bsp.is_some(), "{:?}", actual.report);
    let origins = actual.face_origins.as_ref().unwrap();
    let child_sources: std::collections::BTreeSet<_> = child
        .solids()
        .flat_map(|solid| solid.sides.iter().map(move |side| (solid.id, side.id)))
        .collect();
    for helper in parent.entities.iter().filter(|entity| {
        entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
    }) {
        let retained: Vec<_> = (0..origins.len())
            .flat_map(|face| origins.contributors(face).unwrap())
            .filter_map(|origin| match origin {
                crate::face_origins::Origin::Instance {
                    instance,
                    file,
                    solid,
                    side,
                } if *instance == helper.id => Some((file, *solid, *side)),
                _ => None,
            })
            .collect();
        assert!(!retained.is_empty());
        assert!(
            retained
                .iter()
                .all(|(file, solid, side)| file.as_str() == "child.vmf"
                    && child_sources.contains(&(*solid, *side)))
        );
    }
    assert_eq!(
        (
            actual.stats.brushes,
            actual.stats.faces,
            actual.stats.models
        ),
        (
            expected.stats.brushes,
            expected.stats.faces,
            expected.stats.models
        )
    );
    let mut map_report = Report::default();
    let map = crate::map::Map::load(expanded.vmf.as_ref(), &game, &mut map_report).unwrap();
    assert_eq!(
        map.brushes
            .iter()
            .filter(|brush| brush.contents & source_formats::bsp::contents::DETAIL != 0)
            .count(),
        2
    );
    assert_eq!(parent, before);
    assert_eq!(
        std::fs::read_to_string(directory.path().join("child.vmf")).unwrap(),
        child.write(&Default::default())
    );
}

#[test]
fn translation_bsp_matches_manual_and_region_uses_geometry_outside_marker() {
    let (directory, mut parent, mut child, game) = fixture();
    let context = source(directory.path());
    let actual = compiled(&parent, &game, &context);
    let expected = crate::compile(&manual_parent(&parent, &child), &game);
    assert_eq!(
        actual.bsp.unwrap().to_bytes(),
        expected.bsp.unwrap().to_bytes()
    );
    for solid in &mut child.world.solids {
        for side in &mut solid.sides {
            for point in &mut side.plane.0 {
                point.x -= 512.0;
            }
        }
    }
    for entity in &mut child.entities {
        for solid in &mut entity.solids {
            for side in &mut solid.sides {
                for point in &mut side.plane.0 {
                    point.x -= 512.0;
                }
            }
        }
    }
    std::fs::write(
        directory.path().join("child.vmf"),
        child.write(&Default::default()),
    )
    .unwrap();
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("origin", "512 0 0");
    parent.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-128 -128 0"),
            Node::string("maxs", "128 128 256"),
        ],
    ));
    let actual = compiled(&parent, &game, &context);
    let expected = crate::compile(&manual_parent(&parent, &child), &game);
    assert!(actual.bsp.is_some(), "{:?}", actual.report);
    assert_eq!(
        actual.bsp.unwrap().to_bytes(),
        expected.bsp.unwrap().to_bytes()
    );
    assert!(actual.stats.compile_region.is_some());
}

#[test]
fn staged_writes_reject_dependency_edits_and_output_replacement() {
    for mutation in ["same_size", "rename", "missing", "parent", "output"] {
        let (directory, parent, _, game) = fixture();
        let output = compiled(&parent, &game, &source(directory.path()));
        assert!(output.bsp.is_some(), "{:?}", output.report);
        let path = directory.path().join("previous.bsp");
        let lin = path.with_extension("lin");
        std::fs::write(&path, b"previous BSP").unwrap();
        std::fs::write(&lin, b"previous LIN").unwrap();
        let staged = output
            .prepare_write_controlled(&path, &Control::default())
            .unwrap()
            .unwrap();
        let child = directory.path().join("child.vmf");
        match mutation {
            "same_size" => {
                let text = std::fs::read_to_string(&child)
                    .unwrap()
                    .replace("instance_world", "instance_other");
                std::fs::write(&child, text).unwrap();
            }
            "rename" => {
                let replacement = directory.path().join("replacement.vmf");
                std::fs::write(&replacement, std::fs::read(&child).unwrap()).unwrap();
                std::fs::rename(replacement, &child).unwrap();
            }
            "missing" => std::fs::remove_file(child).unwrap(),
            "parent" => {
                std::fs::write(directory.path().join("parent.vmf"), b"changed saved parent")
                    .unwrap()
            }
            "output" => std::fs::write(&path, b"external replacement").unwrap(),
            _ => unreachable!(),
        }
        assert!(staged.commit(&Control::default()).is_err(), "{mutation}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            if mutation == "output" {
                &b"external replacement"[..]
            } else {
                &b"previous BSP"[..]
            }
        );
        assert_eq!(std::fs::read(&lin).unwrap(), b"previous LIN");
    }
}

#[cfg(unix)]
#[test]
fn staged_writes_reject_output_alias_retarget_and_cancel_for_bsp_and_leak() {
    use std::os::unix::fs::symlink;
    for leaking in [false, true] {
        let (directory, mut parent, _, game) = fixture();
        if leaking {
            parent.world.solids.remove(0);
        }
        let output = compiled(&parent, &game, &source(directory.path()));
        assert_eq!(output.leak.is_some(), leaking, "{:?}", output.report);
        assert_eq!(output.bsp.is_some(), !leaking, "{:?}", output.report);
        let original = directory.path().join("original");
        let replacement = directory.path().join("replacement");
        let alias = directory.path().join("output");
        std::fs::create_dir(&original).unwrap();
        std::fs::create_dir(&replacement).unwrap();
        for path in [&original, &replacement] {
            std::fs::write(path.join("map.bsp"), b"previous BSP").unwrap();
            std::fs::write(path.join("map.lin"), b"previous LIN").unwrap();
        }
        symlink(&original, &alias).unwrap();
        let control = Control::default();
        let cancelled = output
            .prepare_write_controlled(&alias.join("map.bsp"), &control)
            .unwrap()
            .unwrap();
        control.cancel().unwrap();
        assert!(cancelled.commit(&control).is_err());
        let staged = output
            .prepare_write_controlled(&alias.join("map.bsp"), &Control::default())
            .unwrap()
            .unwrap();
        std::fs::remove_file(&alias).unwrap();
        symlink(&replacement, &alias).unwrap();
        assert!(staged.commit(&Control::default()).is_err());
        for path in [&original, &replacement] {
            assert_eq!(
                std::fs::read(path.join("map.bsp")).unwrap(),
                b"previous BSP"
            );
            assert_eq!(
                std::fs::read(path.join("map.lin")).unwrap(),
                b"previous LIN"
            );
        }
    }
}

#[test]
fn leak_publication_retains_guard_and_context_free_apis_stay_rejected() {
    let (directory, mut parent, _, game) = fixture();
    let context = source(directory.path());
    assert!(crate::compile(&parent, &game).bsp.is_none());
    parent.world.solids.remove(0);
    let output = compiled(&parent, &game, &context);
    assert!(output.bsp.is_none());
    assert!(output.leak.is_some(), "{:?}", output.report);
    assert!(output.instance_dependencies.is_some());
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
    let lease =
        crate::OutputLease::acquire(&path.with_extension("lin"), &Control::default()).unwrap();
    let staged = output
        .prepare_write_controlled(&path, &Control::default())
        .unwrap()
        .unwrap();
    std::fs::remove_file(directory.path().join("child.vmf")).unwrap();
    assert!(staged.commit_checked(&lease, &Control::default()).is_err());
    assert_eq!(
        std::fs::read(path.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
    assert_eq!(std::fs::read(path).unwrap(), b"previous BSP");
}

#[test]
fn unsupported_child_semantics_and_path_escapes_are_explicit() {
    let (directory, mut parent, child, game) = fixture();
    let context = source(directory.path());
    for class in ["info_target", "func_instance", "func_instance_parms"] {
        let mut unsupported = child.clone();
        unsupported.entities[0].properties.set("classname", class);
        std::fs::write(
            directory.path().join("child.vmf"),
            unsupported.write(&Default::default()),
        )
        .unwrap();
        let output = compiled(&parent, &game, &context);
        assert!(output.bsp.is_none());
        assert!(output.report.has_errors());
        assert_eq!(
            output.report.diagnostics.last().unwrap().object,
            Some(format!("entity:{}", parent.entities.last().unwrap().id))
        );
    }
    std::fs::write(
        directory.path().join("child.vmf"),
        child.write(&Default::default()),
    )
    .unwrap();
    for path in [
        "../child.vmf",
        "/child.vmf",
        "C:\\child.vmf",
        "child",
        "child.vmt",
    ] {
        parent
            .entities
            .last_mut()
            .unwrap()
            .properties
            .set("file", path);
        let output = compiled(&parent, &game, &context);
        assert!(output.bsp.is_none(), "{path}");
        assert!(output.report.has_errors());
    }
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("file", "child.vmf");
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("replace01", "$name changed");
    assert!(compiled(&parent, &game, &context).report.has_errors());
}

#[cfg(unix)]
#[test]
fn root_symlink_uses_referring_directory_and_rejects_retarget_or_child_alias() {
    use std::os::unix::fs::symlink;
    let (directory, parent, _, game) = fixture();
    let physical = directory.path().join("physical");
    std::fs::create_dir(&physical).unwrap();
    std::fs::write(physical.join("root.vmf"), parent.write(&Default::default())).unwrap();
    let alias = directory.path().join("alias.vmf");
    symlink(physical.join("root.vmf"), &alias).unwrap();
    let context = SourceContext::capture(&alias, &Control::default()).unwrap();
    assert_eq!(context.path(), alias);
    let output = compiled(&parent, &game, &context);
    assert!(output.bsp.is_some(), "{:?}", output.report);
    let path = directory.path().join("result.bsp");
    let staged = output
        .prepare_write_controlled(&path, &Control::default())
        .unwrap()
        .unwrap();
    assert!(staged.commit(&Control::default()).unwrap());
    std::fs::remove_file(&alias).unwrap();
    symlink(directory.path().join("parent.vmf"), &alias).unwrap();
    assert!(
        output
            .instance_dependencies
            .as_ref()
            .unwrap()
            .verify_metadata()
            .is_err()
    );
    let child = directory.path().join("child.vmf");
    std::fs::rename(&child, directory.path().join("child-real.vmf")).unwrap();
    symlink(directory.path().join("child-real.vmf"), child).unwrap();
    assert!(
        compiled(&parent, &game, &source(directory.path()))
            .report
            .has_errors()
    );
}

#[test]
fn no_instance_bytes_stay_exact_and_saved_parent_is_not_the_memory_snapshot() {
    let (directory, mut parent, _, game) = fixture();
    let context = source(directory.path());
    parent.entities.pop();
    let baseline = crate::compile(&parent, &game);
    let output = compiled(&parent, &game, &context);
    assert_eq!(
        output.bsp.unwrap().to_bytes(),
        baseline.bsp.unwrap().to_bytes()
    );
    assert!(output.instance_dependencies.is_none());
    assert!(output.stats.instances.is_none());
    let mut parent = test_common::room();
    let mut instance = Entity::new(parent.object_ids().max().unwrap() + 1, "func_instance");
    instance.properties.set("file", "child.vmf");
    instance
        .properties
        .set("origin", format_vec3(DVec3::new(80.0, 16.0, 16.0)));
    parent.entities.push(instance);
    parent
        .world
        .properties
        .set("message", "unsaved in-memory value");
    let output = compiled(&parent, &game, &context);
    assert!(output.bsp.is_some(), "{:?}", output.report);
    output
        .instance_dependencies
        .as_ref()
        .unwrap()
        .verify(&Control::default())
        .unwrap();
}

#[test]
fn bounded_sources_counts_and_cancellation_preserve_existing_output() {
    let (directory, mut parent, child, game) = fixture();
    let context = source(directory.path());
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    let child_path = directory.path().join("child.vmf");
    let source_text = child.write(&Default::default());
    let mut exact = source_text.clone();
    exact.push_str("\n//");
    exact.extend(std::iter::repeat_n('x', MAX_FILE_BYTES - exact.len()));
    std::fs::write(&child_path, &exact).unwrap();
    let output = compiled(&parent, &game, &context);
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert_eq!(
        output.stats.instances.unwrap().dependencies[0].bytes,
        MAX_FILE_BYTES
    );
    exact.push('x');
    std::fs::write(&child_path, &exact).unwrap();
    let output = compiled(&parent, &game, &context);
    assert!(output.report.has_errors());
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    std::fs::write(&child_path, &source_text).unwrap();
    let helper = parent.entities.last().unwrap().clone();
    for i in 1..=MAX_INSTANCES {
        let mut copy = helper.clone();
        copy.id += i as u32;
        parent.entities.push(copy);
    }
    let output = compiled(&parent, &game, &context);
    assert!(output.report.has_errors());
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .last()
            .unwrap()
            .message
            .contains("occurrence")
    );
    parent.entities.truncate(3);
    let control = Control::default();
    control.cancel().unwrap();
    let output = compile_with_source_controlled(
        &parent,
        &game,
        Options::default(),
        None,
        Some(&context),
        &control,
    );
    assert!(output.bsp.is_none());
    assert!(output.leak.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
}

fn helpers_for_files(parent: &Vmf, names: &[String]) -> Vmf {
    let mut parent = parent.clone();
    let template = parent.entities.pop().unwrap();
    for (index, name) in names.iter().enumerate() {
        let mut helper = template.clone();
        helper.id += index as u32;
        helper.properties.set("file", name);
        parent.entities.push(helper);
    }
    parent
}

#[test]
fn unique_file_and_aggregate_byte_limits_count_before_loading() {
    let (directory, parent, _, _) = fixture();
    let context = source(directory.path());
    let names = (0..=MAX_FILES)
        .map(|index| format!("module{index}.vmf"))
        .collect::<Vec<_>>();
    let minimal = "world { id 1 classname worldspawn }";
    for name in &names {
        std::fs::write(directory.path().join(name), minimal).unwrap();
    }
    let at_limit = helpers_for_files(&parent, &names[..MAX_FILES]);
    let prepared = prepare(
        &at_limit,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    assert_eq!(prepared.stats.unwrap().files, MAX_FILES);
    let over_limit = helpers_for_files(&parent, &names);
    let error = prepare(
        &over_limit,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("unique files"));

    let mut padded = format!("{minimal}\n//");
    padded.extend(std::iter::repeat_n('x', MAX_FILE_BYTES - padded.len()));
    let full_files = MAX_TOTAL_BYTES / MAX_FILE_BYTES;
    for name in &names[..full_files] {
        std::fs::write(directory.path().join(name), &padded).unwrap();
    }
    let at_limit = helpers_for_files(&parent, &names[..full_files]);
    let prepared = prepare(
        &at_limit,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    assert_eq!(
        prepared
            .stats
            .unwrap()
            .dependencies
            .iter()
            .map(|dependency| dependency.bytes)
            .sum::<usize>(),
        MAX_TOTAL_BYTES
    );
    let over_limit = helpers_for_files(&parent, &names[..=full_files]);
    let error = prepare(
        &over_limit,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("0-byte limit"));
}

#[test]
fn bounded_parser_checks_per_file_and_aggregate_nodes_before_admission() {
    let (directory, parent, _, _) = fixture();
    let context = source(directory.path());
    let mut text = String::from("world { id 1 classname worldspawn ");
    for _ in 3..MAX_FILE_NODES {
        text.push_str("x 0 ");
    }
    text.push('}');
    std::fs::write(directory.path().join("child.vmf"), &text).unwrap();
    assert!(
        prepare(
            &parent,
            Some(&context),
            &Control::default(),
            &mut Report::default()
        )
        .is_ok()
    );
    let over_limit = text.replacen("x 0", "x 0 x 0", 1);
    std::fs::write(directory.path().join("child.vmf"), over_limit).unwrap();
    let error = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("node count exceeds"));
    let full_files = MAX_TOTAL_NODES / MAX_FILE_NODES;
    let names = (0..=full_files)
        .map(|index| format!("nodes{index}.vmf"))
        .collect::<Vec<_>>();
    for name in &names {
        std::fs::write(directory.path().join(name), &text).unwrap();
    }
    let at_limit = helpers_for_files(&parent, &names[..full_files]);
    assert!(
        prepare(
            &at_limit,
            Some(&context),
            &Control::default(),
            &mut Report::default()
        )
        .is_ok()
    );
    let over_limit = helpers_for_files(&parent, &names);
    let error = prepare(
        &over_limit,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("node count exceeds"));
    let deeply_nested = format!(
        "{}{}",
        "a { ".repeat(MAX_DEPTH + 1),
        "} ".repeat(MAX_DEPTH + 1)
    );
    std::fs::write(directory.path().join("child.vmf"), deeply_nested).unwrap();
    let error = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("block nesting exceeds"));
}

#[test]
fn unsupported_geometry_self_cycles_and_native_capture_stay_explicit() {
    let (directory, mut parent, child, game) = fixture();
    let context = source(directory.path());
    for issue in ["uv", "metadata", "bounds", "region", "ids"] {
        let mut invalid = child.clone();
        match issue {
            "uv" => invalid.world.solids[0].sides[0].u_axis.scale = 0.0,
            "metadata" => invalid.world.solids[0]
                .extra
                .push(Node::block("opaque", vec![])),
            "bounds" => {
                for side in &mut invalid.world.solids[0].sides {
                    for point in &mut side.plane.0 {
                        point.x += 20_000.0;
                    }
                }
            }
            "region" => {
                invalid.cordon = Some(Node::block(
                    "cordon",
                    vec![
                        Node::string("active", "1"),
                        Node::string("mins", "-128 -128 0"),
                        Node::string("maxs", "128 128 256"),
                    ],
                ))
            }
            "ids" => invalid.world.solids[0].id = invalid.entities[0].id,
            _ => unreachable!(),
        }
        std::fs::write(
            directory.path().join("child.vmf"),
            invalid.write(&Default::default()),
        )
        .unwrap();
        let output = compiled(&parent, &game, &context);
        assert!(output.report.has_errors(), "{issue}");
        assert!(output.bsp.is_none());
    }
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("file", "parent.vmf");
    let output = compiled(&parent, &game, &context);
    assert!(
        output
            .report
            .diagnostics
            .last()
            .unwrap()
            .message
            .contains("cycle")
    );
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("file", "child.vmf");
    let output = compile_with_source_controlled(
        &parent,
        &game,
        Options {
            generate_reflections: true,
            lighting: crate::LightingMode::DirectLdr,
            ..Default::default()
        },
        None,
        Some(&context),
        &Control::default(),
    );
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .last()
            .unwrap()
            .message
            .contains("Generate reflections")
    );
}

#[test]
fn cheap_source_location_freezes_parent_identity_before_worker_hashing() {
    let (directory, parent, _, _) = fixture();
    let path = directory.path().join("parent.vmf");
    let location = SourceLocation::capture(&path).unwrap();
    std::fs::write(&path, b"externally changed parent").unwrap();
    assert!(location.hydrate(&Control::default()).is_err());
    std::fs::write(&path, parent.write(&Default::default())).unwrap();
    let context = SourceLocation::capture(&path)
        .unwrap()
        .hydrate(&Control::default())
        .unwrap();
    let mut report = Report::default();
    let prepared = resolve(&parent, Some(&context), &Control::default(), &mut report).unwrap();
    let brushes = prepared.generated_brushes().collect::<Vec<_>>();
    assert_eq!(brushes.len(), 2);
    assert_eq!(brushes.iter().filter(|brush| brush.detail).count(), 1);
    assert!(brushes.iter().all(
        |brush| brush.owner == parent.entities.last().unwrap().id && brush.file == "child.vmf"
    ));
    assert!(prepared.dependencies().is_some());
    for brush in brushes {
        for side in &brush.solid.sides {
            let origin = prepared.face_origin(side.id).unwrap();
            assert_eq!(
                (origin.0, origin.1, origin.2),
                (brush.owner, brush.file, brush.original)
            );
        }
    }
}

#[test]
fn reference_presence_is_metadata_only_and_keeps_directory_changes_independent() {
    let (directory, _, _, _) = fixture();
    let location = SourceLocation::capture(&directory.path().join("parent.vmf")).unwrap();
    let modules = directory.path().join("modules");
    std::fs::create_dir(&modules).unwrap();
    let path = modules.join("child.VMF");
    std::fs::write(&path, [0xff, 0x00, 0xfe]).unwrap();
    let reference = location.reference(".\\modules\\child.VMF").unwrap();
    assert_eq!(reference.relative(), "modules/child.VMF");
    assert_eq!(reference.path(), path);
    assert_eq!(reference.bytes(), 3);
    assert_eq!(
        location.reference_from_path(&path).unwrap().relative(),
        reference.relative()
    );
    std::fs::write(directory.path().join("unrelated.txt"), "unrelated").unwrap();
    std::fs::create_dir(modules.join("unrelated")).unwrap();
    reference.verify_metadata().unwrap();
    let replacement = modules.join("replacement.VMF");
    std::fs::write(&replacement, [0xff, 0x00, 0xfe]).unwrap();
    std::fs::rename(replacement, &path).unwrap();
    assert!(reference.verify_metadata().is_err());
    location.verify_metadata().unwrap();
    assert!(location.reference("modules/child.VMF").is_ok());
}

#[test]
fn reference_presence_rejects_unsupported_paths_and_self_aliases() {
    let (directory, _, _, _) = fixture();
    #[cfg(unix)]
    std::fs::hard_link(
        directory.path().join("parent.vmf"),
        directory.path().join("self.vmf"),
    )
    .unwrap();
    let location = SourceLocation::capture(&directory.path().join("parent.vmf")).unwrap();
    let oversized = directory.path().join("oversized.vmf");
    std::fs::File::create(&oversized)
        .unwrap()
        .set_len(MAX_FILE_BYTES as u64 + 1)
        .unwrap();
    std::fs::create_dir(directory.path().join("directory.vmf")).unwrap();
    #[cfg(unix)]
    assert!(
        location
            .reference("self.vmf")
            .unwrap_err()
            .to_string()
            .contains("self-reference cycle")
    );
    for value in [
        "",
        "../child.vmf",
        "modules/../child.vmf",
        "/child.vmf",
        "C:\\child.vmf",
        "child",
        "child.vmx",
        "missing.vmf",
        "directory.vmf",
        "oversized.vmf",
        "parent.vmf",
        "child.vmf\n",
    ] {
        assert!(location.reference(value).is_err(), "{value:?}");
    }
    assert!(
        location
            .reference(&"x".repeat(MAX_FIELD_BYTES + 1))
            .is_err()
    );
    assert!(
        location
            .reference_from_path(Path::new("child.vmf"))
            .is_err()
    );
    let outside = tempfile::tempdir().unwrap();
    std::fs::copy(
        directory.path().join("child.vmf"),
        outside.path().join("child.vmf"),
    )
    .unwrap();
    assert!(
        location
            .reference_from_path(&outside.path().join("child.vmf"))
            .is_err()
    );
    assert!(location.reference("child.vmf").is_ok());
}

#[cfg(unix)]
#[test]
fn reference_picker_preserves_referring_aliases_and_rejects_child_symlinks() {
    use std::os::unix::fs::symlink;
    let (directory, _, _, _) = fixture();
    let physical = directory.path().join("physical");
    std::fs::create_dir(&physical).unwrap();
    std::fs::copy(
        directory.path().join("parent.vmf"),
        physical.join("root.vmf"),
    )
    .unwrap();
    let referring = directory.path().join("referring.vmf");
    symlink(physical.join("root.vmf"), &referring).unwrap();
    let alias = directory.path().join("alias");
    symlink(directory.path(), &alias).unwrap();
    let location = SourceLocation::capture(&alias.join("referring.vmf")).unwrap();
    let reference = location
        .reference_from_path(&alias.join("child.vmf"))
        .unwrap();
    assert_eq!(reference.relative(), "child.vmf");
    assert_eq!(reference.path(), directory.path().join("child.vmf"));
    assert!(
        location
            .reference_from_path(&directory.path().join("child.vmf"))
            .is_ok()
    );
    assert!(location.reference("referring.vmf").is_err());
    let child_alias = directory.path().join("child-alias.vmf");
    symlink(directory.path().join("child.vmf"), &child_alias).unwrap();
    assert!(location.reference_from_path(&child_alias).is_err());
    let modules = directory.path().join("modules");
    std::fs::create_dir(&modules).unwrap();
    std::fs::copy(
        directory.path().join("child.vmf"),
        modules.join("child.vmf"),
    )
    .unwrap();
    symlink(&modules, directory.path().join("linked-modules")).unwrap();
    assert!(location.reference("linked-modules/child.vmf").is_err());
    let ambiguous = directory.path().join("modules\\child.vmf");
    std::fs::write(&ambiguous, "separate filename").unwrap();
    assert!(location.reference_from_path(&ambiguous).is_err());
    reference.verify_metadata().unwrap();
    std::fs::remove_file(&referring).unwrap();
    symlink(directory.path().join("parent.vmf"), &referring).unwrap();
    assert!(reference.verify_metadata().is_err());
}

#[cfg(unix)]
#[test]
fn referring_directory_alias_cannot_hide_a_replaced_physical_directory() {
    use std::os::unix::fs::symlink;
    let (directory, _, _, _) = fixture();
    let target = directory.path().join("target");
    let replacement = directory.path().join("replacement");
    std::fs::create_dir(&target).unwrap();
    std::fs::create_dir(&replacement).unwrap();
    for name in ["parent.vmf", "child.vmf"] {
        std::fs::copy(directory.path().join(name), target.join(name)).unwrap();
        std::fs::hard_link(target.join(name), replacement.join(name)).unwrap();
    }
    let alias = directory.path().join("referring");
    symlink(&target, &alias).unwrap();
    let location = SourceLocation::capture(&alias.join("parent.vmf")).unwrap();
    let reference = location.reference("child.vmf").unwrap();
    let context = location.clone().hydrate(&Control::default()).unwrap();
    std::fs::write(target.join("unrelated.txt"), "unrelated").unwrap();
    location.verify_metadata().unwrap();
    reference.verify_metadata().unwrap();
    context.verify_metadata().unwrap();
    std::fs::rename(&target, directory.path().join("previous-target")).unwrap();
    std::fs::rename(&replacement, &target).unwrap();
    location.alias.verify_metadata().unwrap();
    location.root.verify_metadata().unwrap();
    location
        .directory_alias
        .as_ref()
        .unwrap()
        .verify_container_identity()
        .unwrap();
    assert!(location.verify_metadata().is_err());
    assert!(reference.verify_metadata().is_err());
    assert!(context.verify_metadata().is_err());
}

#[test]
fn new_reference_guards_absence_without_reserving_or_reading_child_contents() {
    let (directory, _, _, _) = fixture();
    let modules = directory.path().join("modules");
    std::fs::create_dir(&modules).unwrap();
    let parent = directory.path().join("parent.vmf");
    let before = std::fs::read(&parent).unwrap();
    let location = SourceLocation::capture(&parent).unwrap();
    let reference = location.new_reference(".\\modules\\new.VMF").unwrap();
    assert_eq!(reference.relative(), "modules/new.VMF");
    assert_eq!(reference.path(), modules.join("new.VMF"));
    assert!(!reference.path().exists());
    std::fs::write(modules.join("private-stage.vmf"), [0xff, 0x00, 0xfe]).unwrap();
    std::fs::create_dir(modules.join("unrelated")).unwrap();
    std::fs::write(directory.path().join("unrelated.txt"), "unrelated").unwrap();
    reference.clone().verify_metadata().unwrap();
    assert!(location.reference("modules/new.VMF").is_err());
    std::fs::write(reference.path(), "competing creator").unwrap();
    assert_eq!(
        reference.verify_metadata().unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(
        std::fs::read(reference.path()).unwrap(),
        b"competing creator"
    );
    assert_eq!(std::fs::read(parent).unwrap(), before);
}

#[test]
fn new_reference_rejects_missing_parents_existing_targets_and_unsafe_names() {
    let (directory, _, _, _) = fixture();
    let location = SourceLocation::capture(&directory.path().join("parent.vmf")).unwrap();
    std::fs::create_dir(directory.path().join("directory.vmf")).unwrap();
    for value in [
        "",
        "../new.vmf",
        "modules/../new.vmf",
        "/new.vmf",
        "C:\\new.vmf",
        "new",
        "new.vmx",
        "new.vmf\n",
        "missing/new.vmf",
        "child.vmf/new.vmf",
        "parent.vmf",
        "child.vmf",
        "directory.vmf",
    ] {
        assert!(location.new_reference(value).is_err(), "{value:?}");
    }
    assert!(
        location
            .new_reference(&format!("{}.vmf", "x".repeat(MAX_FIELD_BYTES)))
            .is_err()
    );
    assert_eq!(
        location.new_reference("child.vmf").unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    assert!(location.new_reference("new.vmf").is_ok());
}

#[cfg(unix)]
#[test]
fn new_reference_rejects_symlinks_and_retains_every_target_directory_identity() {
    use std::os::unix::fs::symlink;
    let (directory, _, _, _) = fixture();
    let modules = directory.path().join("modules");
    let nested = modules.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let location = SourceLocation::capture(&directory.path().join("parent.vmf")).unwrap();
    symlink(&modules, directory.path().join("linked-modules")).unwrap();
    symlink(
        directory.path().join("missing.vmf"),
        nested.join("dangling.vmf"),
    )
    .unwrap();
    symlink(
        directory.path().join("child.vmf"),
        nested.join("linked.vmf"),
    )
    .unwrap();
    assert!(location.new_reference("linked-modules/new.vmf").is_err());
    for value in ["modules/nested/dangling.vmf", "modules/nested/linked.vmf"] {
        assert_eq!(
            location.new_reference(value).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
    }
    let reference = location.new_reference("modules/nested/new.vmf").unwrap();
    std::fs::rename(&nested, modules.join("previous-nested")).unwrap();
    std::fs::create_dir(&nested).unwrap();
    assert!(reference.verify_metadata().is_err());
    let reference = location.new_reference("modules/nested/new.vmf").unwrap();
    std::fs::rename(&modules, directory.path().join("previous-modules")).unwrap();
    std::fs::create_dir_all(&nested).unwrap();
    assert!(reference.verify_metadata().is_err());
    let reference = location.new_reference("modules/nested/new.vmf").unwrap();
    std::fs::remove_dir(&nested).unwrap();
    symlink(directory.path(), &nested).unwrap();
    assert!(reference.verify_metadata().is_err());
    location.verify_metadata().unwrap();
}

#[cfg(unix)]
#[test]
fn new_reference_uses_referring_directory_and_expires_on_source_alias_retarget() {
    use std::os::unix::fs::symlink;
    let (directory, _, _, _) = fixture();
    let physical = directory.path().join("physical");
    std::fs::create_dir(&physical).unwrap();
    std::fs::copy(
        directory.path().join("parent.vmf"),
        physical.join("root.vmf"),
    )
    .unwrap();
    let referring = directory.path().join("referring.vmf");
    symlink(physical.join("root.vmf"), &referring).unwrap();
    let alias = directory.path().join("alias");
    symlink(directory.path(), &alias).unwrap();
    let location = SourceLocation::capture(&alias.join("referring.vmf")).unwrap();
    let reference = location.new_reference("new.vmf").unwrap();
    assert_eq!(reference.path(), directory.path().join("new.vmf"));
    std::fs::write(directory.path().join("unrelated.txt"), "unrelated").unwrap();
    reference.verify_metadata().unwrap();
    std::fs::remove_file(&referring).unwrap();
    symlink(directory.path().join("parent.vmf"), &referring).unwrap();
    assert!(reference.verify_metadata().is_err());
    assert!(!reference.path().exists());
}
