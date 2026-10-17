use super::*;
use crate::{Options, compile_with_source_controlled, test_common};
use glam::DMat3;
use source_formats::{
    gamefs::{GameFs, Mount, MountedPath},
    keyvalues::Node,
    vmf::format_vec3,
};

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
    let location = SourceLocation::capture(&directory.path().join("parent.vmf")).unwrap();
    let oversized = directory.path().join("oversized.vmf");
    std::fs::File::create(&oversized)
        .unwrap()
        .set_len(MAX_FILE_BYTES as u64 + 1)
        .unwrap();
    std::fs::create_dir(directory.path().join("directory.vmf")).unwrap();
    #[cfg(unix)]
    {
        std::fs::hard_link(
            directory.path().join("parent.vmf"),
            directory.path().join("self.vmf"),
        )
        .unwrap();
        assert!(location.reference("self.vmf").is_err());
    }
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
