use map_core::displacement::DisplacementMesh;
use source_formats::bsp::lump;

use super::*;

fn terrain_fixture(power: u32) -> (tempfile::TempDir, Vmf, Vmf, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    let mut document = Document::empty();
    for (detail, mins, maxs) in [
        (false, [-48.0, -32.0, 8.0], [0.0, 16.0, 16.0]),
        (true, [24.0, -24.0, 8.0], [72.0, 24.0, 16.0]),
    ] {
        document
            .edit("authored child terrain", |tx| {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "dev/terrain",
                )?
                else {
                    unreachable!()
                };
                let solid = tx.document().solid(id).unwrap();
                let geometry = BrushGeometry::from_solid(solid).unwrap();
                let face = geometry
                    .faces
                    .iter()
                    .find(|face| face.plane.normal.z > 0.99)
                    .unwrap();
                let side_id = solid.sides[face.side].id;
                map_core::displacement::edit::create(
                    tx,
                    &[map_core::surface::FaceId {
                        solid: id,
                        side: side_id,
                    }],
                    power,
                    0,
                )?;
                let solid = tx.solid_mut(id)?;
                for side in &mut solid.sides {
                    side.u_axis.scale = 0.375;
                    side.v_axis.scale = -0.625;
                    side.u_axis.offset = 3.125;
                    side.v_axis.offset = -9.75;
                }
                let side = solid
                    .sides
                    .iter_mut()
                    .find(|side| side.id == side_id)
                    .unwrap();
                side.smoothing_groups = Some(7);
                let disp = side.displacement.as_mut().unwrap();
                let size = (1 << power) + 1;
                disp.elevation = 0.5;
                disp.subdivided = true;
                disp.distances = Some(
                    (0..size)
                        .map(|row| {
                            (0..size)
                                .map(|col| (row * 2 + col) as f64 * 0.125)
                                .collect()
                        })
                        .collect(),
                );
                disp.offsets = Some(
                    (0..size)
                        .map(|row| {
                            (0..size)
                                .map(|col| {
                                    DVec3::new(
                                        row as f64 * 0.0625,
                                        col as f64 * 0.03125,
                                        (row + col) as f64 * 0.015625,
                                    )
                                })
                                .collect()
                        })
                        .collect(),
                );
                disp.offset_normals = Some(vec![vec![DVec3::Z; size]; size]);
                disp.alphas = Some(
                    (0..size)
                        .map(|row| (0..size).map(|col| (row * 5 + col * 7) as f64).collect())
                        .collect(),
                );
                disp.triangle_tags = Some(vec![vec![6; (size - 1) * 2]; size - 1]);
                disp.allowed_verts = Some(vec![-1; 10]);
                disp.extra_properties.set("smooth", "32");
                if detail {
                    edit::tie_to_entity(tx, &[id], "func_detail", &[])?;
                }
                Ok(())
            })
            .unwrap();
    }
    let child = document.into_vmf();
    write_child(directory.path(), "child.vmf", &child);
    let mut parent = test_common::room();
    let helper_id = parent.object_ids().max().unwrap() + 1;
    parent.entities.push(nested_helper(
        helper_id,
        "child.vmf",
        "16.25 -8.5 4.125",
        "0 0 0",
    ));
    let mut light = Entity::new(parent.object_ids().max().unwrap() + 1, "light");
    light.properties.set("origin", "-80 -64 160");
    light.properties.set("_light", "255 192 96 250");
    light.properties.set("_constant_attn", "1");
    light.properties.set("_quadratic_attn", "0");
    parent.entities.push(light);
    write_child(directory.path(), "parent.vmf", &parent);
    std::fs::create_dir_all(directory.path().join("materials/dev")).unwrap();
    for name in ["terrain", "dev_measuregeneric01"] {
        std::fs::write(
            directory.path().join(format!("materials/dev/{name}.vmt")),
            "LightmappedGeneric { $surfaceprop default }",
        )
        .unwrap();
    }
    let mut game = test_common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, parent, child, game)
}

fn literal_terrain_child(child: &Vmf, origin: DVec3, angles: DVec3) -> Vmf {
    let matrix = source_matrix(angles);
    let mut result = child.clone();
    for (original, solid) in child.solids().zip(
        result.world.solids.iter_mut().chain(
            result
                .entities
                .iter_mut()
                .flat_map(|entity| &mut entity.solids),
        ),
    ) {
        *solid = manual_solid(original, origin, angles);
        for (original_side, side) in original.sides.iter().zip(&mut solid.sides) {
            let Some(before) = &original_side.displacement else {
                continue;
            };
            let disp = side.displacement.as_mut().unwrap();
            disp.start_position = matrix * before.start_position + origin;
            if angles == DVec3::ZERO {
                continue;
            }
            let base_normal = map_core::Plane::from_points(original_side.plane.0)
                .unwrap()
                .normal;
            let size = (1 << disp.power) + 1;
            disp.normals = None;
            disp.distances = None;
            disp.elevation = 0.0;
            disp.offsets = Some(
                (0..size)
                    .map(|row| {
                        (0..size)
                            .map(|col| {
                                let normal = before
                                    .normals
                                    .as_ref()
                                    .map_or(DVec3::ZERO, |rows| rows[row][col]);
                                let distance =
                                    before.distances.as_ref().map_or(0.0, |rows| rows[row][col]);
                                let offset = before
                                    .offsets
                                    .as_ref()
                                    .map_or(DVec3::ZERO, |rows| rows[row][col]);
                                matrix
                                    * (normal * distance + offset + base_normal * before.elevation)
                            })
                            .collect()
                    })
                    .collect(),
            );
            disp.offset_normals = before.offset_normals.as_ref().map(|rows| {
                rows.iter()
                    .map(|row| {
                        row.iter()
                            .map(|normal| (matrix * *normal).normalize_or_zero())
                            .collect()
                    })
                    .collect()
            });
        }
    }
    result
}

fn literal_flat(parent: &Vmf, child: &Vmf, origin: DVec3, angles: DVec3) -> Vmf {
    let transformed = literal_terrain_child(child, origin, angles);
    let mut root = parent.clone();
    for helper in root.entities.iter_mut().filter(|entity| {
        entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
    }) {
        helper.properties.set("origin", "0 0 0");
        helper.properties.set("angles", "0 0 0");
    }
    manual_parent(&root, &transformed)
}

fn checked_output(
    parent: &Vmf,
    game: &GameFs,
    context: Option<&SourceContext>,
    options: Options,
) -> crate::Output {
    let output =
        compile_with_source_controlled(parent, game, options, None, context, &Control::default());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    output
}

#[test]
fn translated_power_two_through_four_terrain_matches_flat_fullbright_and_dual_lighting() {
    for power in 2..=4 {
        let (directory, parent, child, game) = terrain_fixture(power);
        let context = source(directory.path());
        let source_bytes = std::fs::read(directory.path().join("child.vmf")).unwrap();
        let flat = literal_flat(&parent, &child, DVec3::new(16.25, -8.5, 4.125), DVec3::ZERO);
        for lighting in [
            crate::LightingMode::Fullbright,
            crate::LightingMode::DirectLdrHdr,
        ] {
            let options = Options {
                lighting,
                ..Options::default()
            };
            let actual = checked_output(&parent, &game, Some(&context), options);
            let expected = checked_output(&flat, &game, None, options);
            let stats = actual.stats.instances.as_ref().unwrap();
            assert_eq!(stats.terrain_patches, 2);
            assert_eq!(stats.terrain_vertices, 2 * ((1_usize << power) + 1).pow(2));
            assert_eq!(
                actual.bsp.as_ref().unwrap().to_bytes(),
                expected.bsp.unwrap().to_bytes()
            );
            let origins = actual.face_origins.as_ref().unwrap();
            let displacement = actual
                .bsp
                .as_ref()
                .unwrap()
                .displacements()
                .unwrap()
                .unwrap();
            for info in displacement.infos {
                assert!(origins.contributors(usize::from(info.face)).unwrap().any(|origin| {
                    matches!(origin, crate::face_origins::Origin::Instance { file, .. } if file == "child.vmf")
                }));
            }
        }
        assert_eq!(
            std::fs::read(directory.path().join("child.vmf")).unwrap(),
            source_bytes
        );
    }
}

#[test]
fn rotated_terrain_matches_independent_flat_positions_and_both_lighting_channels() {
    let (directory, mut parent, child, game) = terrain_fixture(3);
    let root = parent
        .entities
        .iter_mut()
        .find(|entity| {
            entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
        })
        .unwrap();
    root.properties.set("angles", "0 90 0");
    let context = source(directory.path());
    let flat = literal_flat(
        &parent,
        &child,
        DVec3::new(16.25, -8.5, 4.125),
        DVec3::new(0.0, 90.0, 0.0),
    );
    let options = Options {
        lighting: crate::LightingMode::DirectLdrHdr,
        ..Options::default()
    };
    let actual = checked_output(&parent, &game, Some(&context), options)
        .bsp
        .unwrap();
    let expected = checked_output(&flat, &game, None, options).bsp.unwrap();
    let a = actual.displacements().unwrap().unwrap();
    let b = expected.displacements().unwrap().unwrap();
    assert_eq!(a.infos.len(), b.infos.len());
    assert_eq!(a.triangles, b.triangles);
    for index in 0..a.infos.len() {
        let actual_positions = a
            .positions(index, actual.displacement_quad(index).unwrap())
            .unwrap();
        let expected_positions = b
            .positions(index, expected.displacement_quad(index).unwrap())
            .unwrap();
        for (actual, expected) in actual_positions.into_iter().zip(expected_positions) {
            assert!(
                DVec3::from_array(actual.map(f64::from))
                    .abs_diff_eq(DVec3::from_array(expected.map(f64::from)), 0.00002,)
            );
        }
    }
    for channel in [lump::LIGHTING, lump::LIGHTING_HDR] {
        assert!(!actual.lump(channel).data.is_empty());
        assert_eq!(actual.lump(channel).data, expected.lump(channel).data);
    }
}

#[test]
fn nested_noncommuting_terrain_poses_preserve_displaced_points_base_uvs_tags_and_source_ids() {
    let (directory, mut parent, child, _) = terrain_fixture(3);
    let inner_origin = DVec3::new(8.0, -4.0, 12.0);
    let inner_angles = DVec3::new(20.0, 0.0, 0.0);
    write_child(
        directory.path(),
        "middle.vmf",
        &helper_file(vec![nested_helper(100, "child.vmf", "8 -4 12", "20 0 0")]),
    );
    let root = parent
        .entities
        .iter_mut()
        .find(|entity| {
            entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
        })
        .unwrap();
    root.properties.set("file", "middle.vmf");
    root.properties.set("origin", "-12 16 8");
    root.properties.set("angles", "0 90 0");
    let owner = root.id;
    let outer_origin = DVec3::new(-12.0, 16.0, 8.0);
    let outer = DMat3::from_cols(DVec3::Y, -DVec3::X, DVec3::Z);
    let inner = source_matrix(inner_angles);
    let context = source(directory.path());
    let prepared = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    assert_eq!(prepared.vmf().solids().count(), parent.solids().count() + 2);
    for (generated, original) in prepared.generated_brushes().zip(child.solids()) {
        assert_eq!(
            (generated.owner, generated.file, generated.original),
            (owner, "child.vmf", original.id)
        );
        let old_geometry = BrushGeometry::from_solid(original).unwrap();
        let geometry = BrushGeometry::from_solid(generated.solid).unwrap();
        for (index, side) in generated.solid.sides.iter().enumerate() {
            let Some(disp) = &side.displacement else {
                continue;
            };
            let before_side = &original.sides[index];
            let before = before_side.displacement.as_ref().unwrap();
            let old_face = old_geometry
                .faces
                .iter()
                .find(|face| face.side == index)
                .unwrap();
            let face = geometry
                .faces
                .iter()
                .find(|face| face.side == index)
                .unwrap();
            let old_mesh = DisplacementMesh::build(old_face, before).unwrap();
            let mesh = DisplacementMesh::build(face, disp).unwrap();
            for ((actual, old), (base, old_base)) in mesh
                .positions
                .iter()
                .zip(&old_mesh.positions)
                .zip(mesh.base_positions.iter().zip(&old_mesh.base_positions))
            {
                let expected = outer * (inner * *old + inner_origin) + outer_origin;
                assert!(actual.abs_diff_eq(expected, 1e-8));
                assert!(
                    map_core::texel_uv(side, *base)
                        .abs_diff_eq(map_core::texel_uv(before_side, *old_base), 1e-8)
                );
            }
            assert_eq!(disp.alphas, before.alphas);
            assert_eq!(disp.triangle_tags, before.triangle_tags);
            assert_eq!(disp.allowed_verts, before.allowed_verts);
            assert_eq!(disp.extra_properties, before.extra_properties);
            assert_eq!(side.smoothing_groups, before_side.smoothing_groups);
            assert_eq!(
                prepared.face_origin(side.id),
                Some((owner, "child.vmf", original.id, before_side.id))
            );
        }
    }
}

#[test]
fn displaced_intermediate_bounds_fail_even_when_source_cage_and_final_grid_fit() {
    let (directory, mut parent, mut child, _) = terrain_fixture(2);
    child.world.solids[0]
        .sides
        .iter_mut()
        .find_map(|side| side.displacement.as_mut())
        .unwrap()
        .offsets
        .as_mut()
        .unwrap()[0][0]
        .x = 4096.0;
    write_child(directory.path(), "child.vmf", &child);
    write_child(
        directory.path(),
        "middle.vmf",
        &helper_file(vec![nested_helper(100, "child.vmf", "14000 0 0", "0 0 0")]),
    );
    let root = parent
        .entities
        .iter_mut()
        .find(|entity| {
            entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
        })
        .unwrap();
    root.properties.set("file", "middle.vmf");
    root.properties.set("origin", "-14000 0 0");
    let owner = root.id;
    for solid in child.solids() {
        checked_solid(solid).unwrap();
    }
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
        error
            .message
            .contains("Displacement positions must fit Source compile bounds"),
        "{error:?}"
    );
    assert_eq!(error.object, Some(format!("entity:{owner}")));
}

#[test]
fn accepted_cancellation_rolls_back_a_real_terrain_branch() {
    let (directory, mut parent, _, _) = terrain_fixture(2);
    let first = parent
        .entities
        .iter()
        .find(|entity| {
            entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
        })
        .unwrap()
        .clone();
    let mut second = first.clone();
    second.id = parent.object_ids().max().unwrap() + 1;
    second.properties.set("origin", "96 0 0");
    parent.entities.push(second.clone());
    let context = source(directory.path());
    let before = parent.clone();
    let control = Control::default();
    let mut report = Report::default();
    let mut expansion = expand::Expansion::new(&parent, &context, &control, &mut report);
    let mut document = Document::new(parent.clone());
    let error = document
        .edit("cancel real terrain branch", |tx| {
            expansion.helper(
                tx,
                &first,
                &first,
                &mut Vec::new(),
                &mut vec![context.root.clone()],
            )?;
            assert_eq!(
                tx.vmf()
                    .solids()
                    .filter(|solid| solid.sides.iter().any(|side| side.displacement.is_some()))
                    .count(),
                2
            );
            control.cancel().unwrap();
            expansion.helper(
                tx,
                &second,
                &second,
                &mut Vec::new(),
                &mut vec![context.root.clone()],
            )
        })
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert_eq!(document.vmf(), &before);
}

#[test]
fn unsupported_child_grid_semantics_fail_without_replacing_previous_outputs() {
    let (directory, parent, child, game) = terrain_fixture(2);
    let context = source(directory.path());
    let destination = directory.path().join("previous.bsp");
    std::fs::write(&destination, b"previous BSP").unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
    for issue in [
        "rows",
        "unpaired",
        "offset normals",
        "flags",
        "holes",
        "smooth",
        "opaque",
    ] {
        let mut invalid = child.clone();
        let disp = invalid.world.solids[0]
            .sides
            .iter_mut()
            .find_map(|side| side.displacement.as_mut())
            .unwrap();
        match issue {
            "rows" => {
                disp.alphas.as_mut().unwrap()[0].pop().unwrap();
            }
            "unpaired" => disp.distances = None,
            "offset normals" => disp.offset_normals.as_mut().unwrap()[0][0].x = f64::NAN,
            "flags" => disp.flags = Some(1),
            "holes" => disp.triangle_tags.as_mut().unwrap()[0][0] = 64,
            "smooth" => disp.extra_properties.set("smooth", "181"),
            "opaque" => disp.extra.push(Node::block("unsupported", Vec::new())),
            _ => unreachable!(),
        }
        write_child(directory.path(), "child.vmf", &invalid);
        let output = compiled(&parent, &game, &context);
        assert!(
            output.report.has_errors() && output.bsp.is_none(),
            "{issue}"
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
        assert_eq!(
            std::fs::read(destination.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
    }
}

#[test]
fn hidden_unsupported_terrain_keeps_visible_siblings_identical_to_a_flat_control() {
    for hidden_entity in [false, true] {
        for issue in ["power", "opaque", "opaque_block"] {
            let (directory, parent, child, game) = terrain_fixture(2);
            let mut document = Document::new(child);
            document
                .edit("visible ordinary sibling", |tx| {
                    edit::create_block(
                        tx,
                        DVec3::new(-96.0, -32.0, 8.0),
                        DVec3::new(-64.0, 16.0, 16.0),
                        "dev/terrain",
                    )?;
                    Ok(())
                })
                .unwrap();
            let mut child = document.into_vmf();
            let solid = if hidden_entity {
                child.entities[0].hidden = true;
                &mut child.entities[0].solids[0]
            } else {
                child.world.solids[0].hidden = true;
                &mut child.world.solids[0]
            };
            let info = solid
                .sides
                .iter_mut()
                .find_map(|side| side.displacement.as_mut())
                .unwrap();
            match issue {
                "power" => info.power = 5,
                "opaque" => info.extra_properties.set("unsupported_hidden", "ignored"),
                "opaque_block" => info.extra.push(source_formats::keyvalues::Node::block(
                    "startposition",
                    vec![source_formats::keyvalues::Node::string("custom", "ignored")],
                )),
                _ => unreachable!(),
            }
            write_child(directory.path(), "child.vmf", &child);
            let before = std::fs::read(directory.path().join("child.vmf")).unwrap();
            Vmf::parse(std::str::from_utf8(&before).unwrap()).unwrap();
            let mut visible = child.clone();
            visible.world.solids.retain(|solid| !solid.hidden);
            visible.entities.retain(|entity| !entity.hidden);
            let flat = literal_flat(
                &parent,
                &visible,
                DVec3::new(16.25, -8.5, 4.125),
                DVec3::ZERO,
            );
            let context = source(directory.path());
            let prepared = prepare(
                &parent,
                Some(&context),
                &Control::default(),
                &mut Report::default(),
            )
            .unwrap();
            let stats = prepared.stats().unwrap();
            assert_eq!((stats.terrain_patches, stats.terrain_vertices), (1, 25));
            assert_eq!(prepared.generated_brushes().count(), 2);
            let actual = checked_output(&parent, &game, Some(&context), Options::default());
            let expected = checked_output(&flat, &game, None, Options::default());
            assert_eq!(
                actual.bsp.unwrap().to_bytes(),
                expected.bsp.unwrap().to_bytes(),
                "entity={hidden_entity}, issue={issue}"
            );
            assert_eq!(
                std::fs::read(directory.path().join("child.vmf")).unwrap(),
                before
            );
            if hidden_entity {
                child.entities[0].hidden = false;
            } else {
                child.world.solids[0].hidden = false;
            }
            write_child(directory.path(), "child.vmf", &child);
            let error = prepare(
                &parent,
                Some(&context),
                &Control::default(),
                &mut Report::default(),
            )
            .err()
            .unwrap();
            let expected = match issue {
                "power" => "powers 2 through 4",
                "opaque" => "unsupported_hidden",
                "opaque_block" => "startposition",
                _ => unreachable!(),
            };
            assert!(error.message.contains(expected), "{error:?}");
        }
    }
}

#[test]
fn finite_partial_and_fully_collapsed_terrain_fail_shared_admission_and_compilation() {
    for whole in [false, true] {
        let (directory, parent, mut child, game) = terrain_fixture(2);
        let solid = &mut child.world.solids[0];
        let geometry = BrushGeometry::from_solid(solid).unwrap();
        let side = solid
            .sides
            .iter()
            .position(|side| side.displacement.is_some())
            .unwrap();
        let face = geometry
            .faces
            .iter()
            .find(|face| face.side == side)
            .unwrap();
        let info = solid.sides[side].displacement.as_mut().unwrap();
        let mesh = DisplacementMesh::build(face, info).unwrap();
        let anchor = mesh.positions[0];
        for (index, offset) in info
            .offsets
            .as_mut()
            .unwrap()
            .iter_mut()
            .flatten()
            .enumerate()
        {
            if whole || index == 1 {
                *offset += anchor - mesh.positions[index];
            }
        }
        let error = crate::map::terrain::checked_mesh(face, info).unwrap_err();
        assert_eq!(error, "Displacement contains collapsed triangles");
        let source_brush = solid.id;
        write_child(directory.path(), "child.vmf", &child);
        let context = source(directory.path());
        let error = prepare(
            &parent,
            Some(&context),
            &Control::default(),
            &mut Report::default(),
        )
        .err()
        .unwrap();
        assert!(error.message.contains("collapsed triangles"), "{error:?}");
        let owner = parent
            .entities
            .iter()
            .find(|entity| entity.classname() == Some("func_instance"))
            .unwrap()
            .id;
        assert_eq!(error.object, Some(format!("entity:{owner}")));
        let output = compiled(&parent, &game, &context);
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("collapsed triangles"))
        );
        let flat = literal_flat(&parent, &child, DVec3::new(16.25, -8.5, 4.125), DVec3::ZERO);
        let output = crate::compile(&flat, &game);
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("collapsed triangles")),
            "brush {source_brush}: {:?}",
            output.report
        );
    }
}

#[test]
#[ignore = "requires installed CS:S vphysics and the physics worker"]
fn translated_child_terrain_native_cooking_matches_the_independent_flat_control() {
    let (directory, parent, child, mut game) = terrain_fixture(3);
    let root = std::env::var_os("CSS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap())
                .join(".local/share/Steam/steamapps/common/Counter-Strike Source")
        });
    assert!(root.join("bin/linux64/vphysics.so").is_file());
    game.base_dir = root;
    let options = Options {
        physics: crate::PhysicsMode::Engine,
        ..Options::default()
    };
    let flat = literal_flat(&parent, &child, DVec3::new(16.25, -8.5, 4.125), DVec3::ZERO);
    let actual = checked_output(&parent, &game, Some(&source(directory.path())), options);
    let expected = checked_output(&flat, &game, None, options);
    assert!(actual.stats.physics_mesh_triangles > 0);
    assert_eq!(
        actual.stats.physics_mesh_triangles,
        expected.stats.physics_mesh_triangles
    );
    assert_eq!(
        actual.bsp.unwrap().lump(lump::PHYSCOLLIDE).data,
        expected.bsp.unwrap().lump(lump::PHYSCOLLIDE).data
    );
}
