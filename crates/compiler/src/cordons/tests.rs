use super::*;
use source_formats::{
    gamefs::{Mount, MountedPath},
    keyvalues::Node,
};

use crate::test_common as common;

#[test]
fn region_preparation_rejects_visible_instance_helpers_before_origin_selection() {
    let (_directory, game, mut source) = fixture();
    let baseline = prepare(&source, &game, &mut Report::default(), &Control::default())
        .unwrap()
        .vmf
        .into_owned();
    for classname in ["FuNc_InStAnCe", "FUNC_INSTANCE_PARMS"] {
        let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
        let mut entity = Entity::new(id, classname);
        entity.properties.set("origin", "4096 0 64");
        source.entities.push(entity);
        let before = source.clone();
        let error = prepare(&source, &game, &mut Report::default(), &Control::default())
            .err()
            .unwrap();
        assert!(error.contains(classname));
        assert!(error.contains("instance expansion"));
        assert!(!error.contains("disable the region"));
        assert_eq!(source, before);
        source.entities.last_mut().unwrap().hidden = true;
    }
    let prepared = prepare(&source, &game, &mut Report::default(), &Control::default()).unwrap();
    assert_eq!(prepared.vmf.entities, baseline.entities);
    assert_eq!(prepared.vmf.world.solids.len(), baseline.world.solids.len());
}

#[test]
fn detail_prop_origins_use_inclusive_regions_and_preserve_authored_records() {
    let (_directory, game, mut source) = fixture();
    let mut retained = Vec::new();
    let mut omitted = Vec::new();
    for (class, position, hidden, included) in [
        ("prop_detail", Some("1.25 2.5 64"), false, true),
        ("detail_prop", Some("-128 -128 0"), false, true),
        ("PROP_DETAIL", Some("128 128 256"), false, true),
        ("detail_prop", Some("128.001 0 64"), false, false),
        ("prop_detail", None, false, true),
        ("prop_detail", Some("NaN 0 0"), true, false),
        ("prop_detail_sprite", Some("-128 -128 0"), false, true),
        ("PROP_DETAIL_SPRITE", Some("128 128 256"), false, true),
        ("prop_detail_sprite", Some("128.001 0 64"), false, false),
        ("prop_detail_sprite", None, false, true),
        ("prop_detail_sprite", Some("NaN 0 0"), true, false),
    ] {
        let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
        let mut entity = Entity::new(id, class);
        if let Some(position) = position {
            entity.properties.set("origin", position);
        }
        entity.properties.set("model", "models/retained-exact.mdl");
        entity.extra.push(Node::block(
            "custom",
            vec![Node::string("keep", "original")],
        ));
        entity.hidden = hidden;
        source.entities.push(entity.clone());
        if included {
            retained.push(entity);
        } else {
            omitted.push(id);
        }
    }
    let before = source.clone();
    let prepared = prepare(&source, &game, &mut Report::default(), &Control::default()).unwrap();
    let summary = prepared.region.as_ref().unwrap();
    for entity in retained {
        assert_eq!(
            prepared
                .vmf
                .entities
                .iter()
                .find(|copy| copy.id == entity.id),
            Some(&entity)
        );
        assert!(summary.included.contains(&format!("entity:{}", entity.id)));
    }
    for id in omitted {
        assert!(!prepared.vmf.entities.iter().any(|entity| entity.id == id));
        assert!(summary.omitted.contains(&format!("entity:{id}")));
    }
    assert_eq!(source, before);
    let mut document = Document::new(source);
    document
        .edit("exclude implicit zero", |tx| {
            compile_region::apply(
                tx,
                Some(compile_region::Definition {
                    enabled: true,
                    bounds: compile_region::Bounds {
                        mins: DVec3::splat(16.0),
                        maxs: DVec3::splat(128.0),
                    },
                }),
            )
        })
        .unwrap();
    let prepared = prepare(
        document.vmf(),
        &game,
        &mut Report::default(),
        &Control::default(),
    )
    .unwrap();
    assert!(
        !prepared
            .vmf
            .entities
            .iter()
            .any(
                |entity| entity.properties.get("model") == Some("models/retained-exact.mdl")
                    && entity.properties.get("origin").is_none()
            )
    );
}

#[test]
fn detail_region_selection_rejects_invalid_model_and_sprite_origins() {
    let (_directory, game, source) = fixture();
    for (class, origin) in [
        ("prop_detail", "NaN 0 0"),
        ("detail_prop", "invalid"),
        ("prop_detail", "70000 0 0"),
        ("prop_detail_sprite", "NaN 0 0"),
    ] {
        let mut source = source.clone();
        let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
        let mut entity = Entity::new(id, class);
        entity.properties.set("origin", origin);
        source.entities.push(entity);
        let before = source.clone();
        let error = prepare(&source, &game, &mut Report::default(), &Control::default())
            .err()
            .unwrap();
        assert!(error.contains("valid finite origin"), "{error}");
        assert_eq!(source, before);
    }
}

#[test]
fn disjoint_fixed_helpers_preserve_invalid_origin_errors() {
    let (_directory, game, source) = fixture();
    for classname in ["func_occluder", "func_areaportal", "func_areaportalwindow"] {
        let mut document = Document::new(source.clone());
        document
            .edit("invalid outside helper", |tx| {
                let map_core::ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(400.0, 400.0, 32.0),
                    DVec3::new(416.0, 416.0, 64.0),
                    "tools/toolsblack",
                )?
                else {
                    unreachable!()
                };
                edit::tie_to_entity(tx, &[id], classname, &[("origin".into(), "invalid".into())])
            })
            .unwrap();
        let before = document.vmf().clone();
        let output = crate::compile(&before, &game);
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains("valid finite origin"))
        );
        assert_eq!(document.vmf(), &before);
    }
}

fn fixture() -> (tempfile::TempDir, GameFs, Vmf) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsnodraw.vmt"),
        "LightmappedGeneric { %compilenodraw 1 }",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsblack.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.game_dir = directory.path().to_owned();
    game.base_dir = directory.path().to_owned();
    game.mounts = vec![MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    }];
    let mut source = common::room();
    for entity in &mut source.entities {
        let mut point = entity.origin().unwrap();
        point.y = -64.0;
        entity
            .properties
            .set("origin", source_formats::vmf::format_vec3(point));
    }
    let mut document = Document::new(source);
    document
        .edit("region fixture", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-600.0, -16.0, 0.0),
                DVec3::new(600.0, 16.0, 32.0),
                "brick/test",
            )?;
            for (classname, origin, name) in [
                (
                    "info_target",
                    Some(DVec3::new(400.0, 0.0, 64.0)),
                    "outside_region",
                ),
                ("logic_auto", None, "global_logic"),
                ("light", Some(DVec3::new(400.0, 0.0, 64.0)), "outside_light"),
            ] {
                edit::create_entity(
                    tx,
                    classname,
                    origin,
                    &[("targetname".into(), name.into())],
                    Vec::new(),
                )?;
            }
            compile_region::apply(
                tx,
                Some(compile_region::Definition {
                    enabled: true,
                    bounds: compile_region::Bounds {
                        mins: DVec3::new(-128.0, -128.0, 0.0),
                        maxs: DVec3::new(128.0, 128.0, 256.0),
                    },
                }),
            )
        })
        .unwrap();
    (directory, game, document.into_vmf())
}

#[test]
fn preprocessing_seals_retains_crossing_brushes_and_never_mutates_the_source() {
    let (_directory, game, source) = fixture();
    let before = source.clone();
    let mut report = Report::default();
    let prepared = prepare(&source, &game, &mut report, &Control::default()).unwrap();
    let summary = prepared.region.as_ref().unwrap();
    assert!(
        summary
            .generated_brushes
            .iter()
            .all(|id| *id > source.object_ids().max().unwrap())
    );
    assert!(
        prepared
            .vmf
            .world
            .solids
            .iter()
            .flat_map(|solid| &solid.sides)
            .filter(|side| !source.side_ids().any(|id| id == side.id))
            .all(|side| side.id > source.side_ids().max().unwrap())
    );
    assert!(
        prepared
            .vmf
            .world
            .solids
            .iter()
            .any(|solid| solid == source.world.solids.last().unwrap())
    );
    assert!(
        prepared
            .vmf
            .entities
            .iter()
            .any(|entity| entity.classname() == Some("logic_auto"))
    );
    assert!(
        !prepared
            .vmf
            .entities
            .iter()
            .any(|entity| entity.properties.get("targetname") == Some("outside_region"))
    );
    assert!(
        !prepared
            .vmf
            .entities
            .iter()
            .any(|entity| entity.classname() == Some("light"))
    );
    let output = crate::compile(&source, &game);
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert_eq!(output.stats.brushes, 9);
    assert_eq!(source, before);
    let mut broken = prepared.vmf.into_owned();
    broken.cordon = None;
    broken
        .world
        .solids
        .retain(|solid| solid.id != summary.generated_brushes[1]);
    let output = crate::compile(&broken, &game);
    assert!(
        output.leak.is_some() && output.bsp.is_none(),
        "{:?}",
        output.report
    );
}

#[test]
fn contained_world_water_compiles_but_crossing_and_entity_liquids_preserve_prior_output() {
    let (_directory, game, source) = fixture();
    std::fs::write(
        game.game_dir.join("materials/water.vmt"),
        "Water { $abovewater 1 }",
    )
    .unwrap();
    let mut document = Document::new(source);
    let id = document
        .edit("pool", |tx| {
            let map_core::ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-128.0, 16.0, 0.0),
                DVec3::new(128.0, 128.0, 64.0),
                "tools/toolsnodraw",
            )?
            else {
                unreachable!()
            };
            for side in &mut tx.solid_mut(id)?.sides {
                if map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99 {
                    side.material = "water".into();
                }
            }
            Ok(id)
        })
        .unwrap();
    let source = document.into_vmf();
    let output = crate::compile(&source, &game);
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert_eq!(output.stats.water_regions, 1);
    assert!(output.stats.water_leaves > 0);
    let path = game.game_dir.join("previous.bsp");
    assert!(output.write(&path).unwrap());
    let previous = std::fs::read(&path).unwrap();
    for (classname, offset, message) in [
        (None, DVec3::X, "crosses a compile-region boundary"),
        (Some("func_detail"), DVec3::ZERO, "structural world"),
        (Some("func_brush"), DVec3::ZERO, "structural world"),
    ] {
        let mut document = Document::new(source.clone());
        document
            .edit("unsupported water", |tx| {
                edit::translate(tx, &[map_core::ObjectId::Solid(id)], offset, true)?;
                if let Some(classname) = classname {
                    edit::tie_to_entity(tx, &[id], classname, &[])?;
                }
                Ok(())
            })
            .unwrap();
        let output = crate::compile(document.vmf(), &game);
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(message)),
            "{:?}",
            output.report
        );
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), previous);
    }
    let mut outside = Document::new(source);
    outside
        .edit("outside pool", |tx| {
            edit::translate(
                tx,
                &[map_core::ObjectId::Solid(id)],
                DVec3::X * 1024.0,
                true,
            )
        })
        .unwrap();
    let output = crate::compile(outside.vmf(), &game);
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert_eq!(output.stats.water_regions, 0);
    assert!(
        output
            .stats
            .compile_region
            .unwrap()
            .omitted
            .contains(&format!("solid:{id}"))
    );
}

#[test]
fn whole_terrain_retains_world_and_detail_meshes_but_rejects_crossing_cages_and_sheets() {
    let (_directory, game, source) = fixture();
    let mut document = Document::new(source);
    let (id, face) = document
        .edit("terrain", |tx| {
            let map_core::ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(32.0, 32.0, 0.0),
                DVec3::new(112.0, 112.0, 32.0),
                "brick/test",
            )?
            else {
                unreachable!()
            };
            let side = tx
                .solid_mut(id)?
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id;
            let face = map_core::surface::FaceId { solid: id, side };
            map_core::displacement::edit::create(tx, &[face], 2, 0)?;
            Ok((id, face))
        })
        .unwrap();
    let source = document.into_vmf();
    let output = crate::compile(&source, &game);
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert_eq!(output.stats.displacements, 1);
    assert_eq!(output.stats.displacement_triangles, 32);
    let path = game.game_dir.join("terrain.bsp");
    assert!(output.write(&path).unwrap());
    let bytes = std::fs::read(&path).unwrap();
    let mut detail = Document::new(source.clone());
    detail
        .edit("detail terrain", |tx| {
            edit::tie_to_entity(tx, &[id], "func_detail", &[])
        })
        .unwrap();
    let output = crate::compile(detail.vmf(), &game);
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert_eq!(output.stats.displacement_triangles, 32);
    for sheet in [false, true] {
        let mut crossing = Document::new(source.clone());
        crossing
            .edit("crossing terrain", |tx| {
                if sheet {
                    let info = tx
                        .solid_mut(id)?
                        .sides
                        .iter_mut()
                        .find(|side| side.id == face.side)
                        .unwrap()
                        .displacement
                        .as_mut()
                        .unwrap();
                    info.distances.as_mut().unwrap()[2][2] = 300.0;
                } else {
                    edit::translate(tx, &[map_core::ObjectId::Solid(id)], DVec3::Z * 240.0, true)?;
                }
                Ok(())
            })
            .unwrap();
        let output = crate::compile(crossing.vmf(), &game);
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains("Terrain crosses")),
            "{:?}",
            output.report
        );
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    let mut outside = Document::new(source);
    outside
        .edit("outside terrain", |tx| {
            edit::translate(
                tx,
                &[map_core::ObjectId::Solid(id)],
                DVec3::X * 1024.0,
                true,
            )
        })
        .unwrap();
    let output = crate::compile(outside.vmf(), &game);
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert_eq!(output.stats.displacements, 0);
    assert!(
        output
            .stats
            .compile_region
            .unwrap()
            .omitted
            .contains(&format!("solid:{id}"))
    );
}

#[test]
fn global_markers_keep_authored_origins_without_seeding_exterior_region_leaks() {
    let (_directory, game, source) = fixture();
    let mut document = Document::new(source);
    let ids = document
        .edit("outside global markers", |tx| {
            ["light_environment", "water_lod_control"]
                .into_iter()
                .map(|classname| {
                    let properties = if classname == "water_lod_control" {
                        vec![
                            ("cheapwaterstartdistance".into(), "123".into()),
                            ("cheapwaterenddistance".into(), "456".into()),
                        ]
                    } else {
                        Vec::new()
                    };
                    edit::create_entity(
                        tx,
                        classname,
                        Some(DVec3::splat(192.0)),
                        &properties,
                        Vec::new(),
                    )
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap();
    let source = document.into_vmf();
    let output = crate::compile(&source, &game);
    assert!(
        output.bsp.is_some() && output.leak.is_none(),
        "{:?}",
        output.report
    );
    let text = output.bsp.as_ref().unwrap().entities().unwrap();
    for classname in ["light_environment", "water_lod_control"] {
        assert_eq!(
            text.matches(&format!("\"classname\" \"{classname}\""))
                .count(),
            1
        );
    }
    assert_eq!(text.matches("\"origin\" \"192 192 192\"").count(), 2);
    assert!(text.contains("\"cheapwaterstartdistance\" \"123\""));
    for id in ids {
        let map_core::ObjectId::Entity(id) = id else {
            unreachable!()
        };
        assert!(
            output
                .stats
                .compile_region
                .as_ref()
                .unwrap()
                .included
                .contains(&format!("entity:{id}"))
        );
    }
    let mut full = source.clone();
    let bounds = compile_region::inspect(&source).unwrap().unwrap().bounds;
    full.entities.retain(|entity| {
        is_global(entity.classname().unwrap_or_default())
            || entity.origin().is_none_or(|point| bounds.contains(point))
    });
    full.cordon = None;
    let baseline = crate::compile(&full, &game).bsp.unwrap().to_bytes();
    let mut document = Document::new(full);
    document
        .edit("disable", |tx| {
            compile_region::apply(
                tx,
                Some(compile_region::Definition {
                    enabled: false,
                    bounds,
                }),
            )
        })
        .unwrap();
    assert_eq!(
        crate::compile(document.vmf(), &game)
            .bsp
            .unwrap()
            .to_bytes(),
        baseline
    );
}

#[test]
fn missing_or_non_rendering_boundary_material_never_substitutes_nodraw() {
    let (_directory, game, source) = fixture();
    let path = game.game_dir.join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    let material = game.game_dir.join("materials/tools/toolsblack.vmt");
    for text in [
        None,
        Some("UnlitGeneric { %compilenodraw 1 }"),
        Some("Water { $abovewater 1 }"),
    ] {
        if let Some(text) = text {
            std::fs::write(&material, text).unwrap();
        } else {
            std::fs::remove_file(&material).unwrap();
        }
        let output = crate::compile(&source, &game);
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("tools/toolsblack")),
            "{:?}",
            output.report
        );
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    }
}

#[test]
fn brush_model_inclusion_uses_the_same_origin_brush_resolution_as_compilation() {
    let (_directory, game, source) = fixture();
    let mut document = Document::new(source);
    document
        .edit("model pivots", |tx| {
            for (point, marker, name) in [
                (
                    DVec3::new(400.0, 0.0, 64.0),
                    DVec3::new(32.0, 32.0, 32.0),
                    "origin_inside",
                ),
                (
                    DVec3::new(32.0, 0.0, 64.0),
                    DVec3::new(400.0, 0.0, 32.0),
                    "origin_outside",
                ),
            ] {
                let marker = edit::block_solid(
                    tx,
                    marker - DVec3::splat(8.0),
                    marker + DVec3::splat(8.0),
                    "tools/toolsorigin",
                )?;
                let shape = edit::block_solid(
                    tx,
                    DVec3::new(16.0, 32.0, 32.0),
                    DVec3::new(48.0, 48.0, 64.0),
                    "brick/test",
                )?;
                edit::create_entity(
                    tx,
                    "func_brush",
                    Some(point),
                    &[("targetname".into(), name.into())],
                    vec![marker, shape],
                )?;
            }
            Ok(())
        })
        .unwrap();
    std::fs::write(
        game.game_dir.join("materials/tools/toolsorigin.vmt"),
        "LightmappedGeneric { %compileorigin 1 }",
    )
    .unwrap();
    let source = document.into_vmf();
    let prepared = prepare(&source, &game, &mut Report::default(), &Control::default()).unwrap();
    assert!(
        prepared
            .vmf
            .entities
            .iter()
            .any(|entity| entity.properties.get("targetname") == Some("origin_inside"))
    );
    assert!(
        !prepared
            .vmf
            .entities
            .iter()
            .any(|entity| entity.properties.get("targetname") == Some("origin_outside"))
    );
    let output = crate::compile(&source, &game);
    assert!(output.bsp.is_some(), "{:?}", output.report);
}

#[test]
fn disabled_regions_borrow_input_and_unsupported_scopes_or_id_exhaustion_preserve_outputs() {
    let (_directory, game, mut source) = fixture();
    source.cordon = None;
    let prepared = prepare(&source, &game, &mut Report::default(), &Control::default()).unwrap();
    assert!(matches!(prepared.vmf, Cow::Borrowed(_)));
    let mut document = Document::new(source);
    document
        .edit("enable", |tx| {
            compile_region::apply(
                tx,
                Some(compile_region::Definition {
                    enabled: true,
                    bounds: compile_region::Bounds {
                        mins: DVec3::new(-128.0, -128.0, 0.0),
                        maxs: DVec3::new(128.0, 128.0, 256.0),
                    },
                }),
            )
        })
        .unwrap();
    let mut source = document.into_vmf();
    let mut outside = source.entities.last().unwrap().clone();
    outside.id = u32::MAX;
    source.entities.push(outside);
    let output = crate::compile(&source, &game);
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("exhausted")),
        "{:?}",
        output.report
    );
    source.entities.pop();
    for classname in [
        "info_overlay_transition",
        "func_instance",
        "func_viscluster",
    ] {
        let mut unsafe_source = source.clone();
        unsafe_source
            .entities
            .last_mut()
            .unwrap()
            .properties
            .set("classname", classname);
        let before = unsafe_source.clone();
        let output = crate::compile(&unsafe_source, &game);
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(classname))
        );
        assert_eq!(unsafe_source, before);
    }
    let mut malformed = source;
    malformed.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "NaN 0 0"),
            Node::string("maxs", "128 128 128"),
        ],
    ));
    assert!(compile_region::active(&malformed).is_err());
}
