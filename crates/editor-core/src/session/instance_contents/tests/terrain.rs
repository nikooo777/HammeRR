use super::*;
use map_core::surface::FaceId;

fn patch(document: &mut Document, mins: DVec3, maxs: DVec3, power: u32) -> FaceId {
    let ObjectId::Solid(solid) = document
        .edit("terrain cage", |tx| {
            edit::create_block(tx, mins, maxs, "test/terrain")
        })
        .unwrap()
    else {
        unreachable!()
    };
    let side = document
        .solid(solid)
        .unwrap()
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
        .unwrap()
        .id;
    let face = FaceId { solid, side };
    document
        .edit("terrain grid", |tx| {
            map_core::displacement::edit::create(tx, &[face], power, 0)
        })
        .unwrap();
    face
}

fn hill_child() -> (Vmf, [FaceId; 2]) {
    let mut document = Document::empty();
    let world = patch(
        &mut document,
        DVec3::ZERO,
        DVec3::new(128.0, 128.0, 16.0),
        2,
    );
    let detail = patch(
        &mut document,
        DVec3::new(192.0, 0.0, 0.0),
        DVec3::new(320.0, 128.0, 16.0),
        3,
    );
    let mut vmf = document.into_vmf();
    for (index, solid) in vmf.world.solids.iter_mut().enumerate() {
        let info = solid
            .sides
            .iter_mut()
            .find_map(|side| side.displacement.as_mut())
            .unwrap();
        let size = (1 << info.power) + 1;
        info.normals = Some(vec![vec![DVec3::Z; size]; size]);
        let mut distances = vec![vec![0.0; size]; size];
        distances[size / 2][size / 2] = 32.0;
        info.distances = Some(distances);
        info.start_position = DVec3::new(if index == 0 { 0.0 } else { 192.0 }, 0.0, 16.0);
    }
    let entity_id = vmf.object_ids().chain(vmf.side_ids()).max().unwrap() + 1;
    let mut entity = source_formats::vmf::Entity::new(entity_id, "func_detail");
    entity.solids.push(vmf.world.solids.pop().unwrap());
    vmf.entities.push(entity);
    (vmf, [world, detail])
}

#[test]
fn displaced_world_and_detail_contents_use_real_triangles_bounds_uvs_and_leaf_provenance() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, source, child, owner) = opened(actor);
        let (vmf, faces) = hill_child();
        std::fs::write(&child, vmf.write(&Default::default())).unwrap();
        let ObjectId::Entity(id) = owner else {
            unreachable!()
        };
        session
            .edit(actor, "instance pose", |tx| {
                tx.entity_mut(id)?.properties.set("origin", "64 -32 48");
                tx.entity_mut(id)?.properties.set("angles", "0 90 0");
                Ok(())
            })
            .unwrap();
        session.select(&[owner], SelectMode::Replace).unwrap();
        session
            .begin_property_edit(crate::session::PropertyTarget::World)
            .unwrap();
        let token = session.property_draft().unwrap().token;
        session
            .append_property_change(
                token,
                crate::session::PropertyChange::Set {
                    key: "message".into(),
                    value: "retained world draft".into(),
                },
            )
            .unwrap();
        let memory = session.document.vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let settings = session.compile_settings();
        let draft = json!(session.property_draft());
        let disk = std::fs::read(&source).unwrap();
        let child_bytes = std::fs::read(&child).unwrap();
        let report = refresh(&mut session, actor);
        assert_eq!(report["state"], "ready", "{report}");
        assert_eq!(report["geometry"]["vertices"], 106);
        assert_eq!(report["geometry"]["triangles"], 160);
        assert_eq!(report["geometry"]["indices"], 480);
        assert_eq!(report["geometry"]["line_vertices"], 528);
        assert_eq!(report["face_provenance_total"], 2);
        let rows = report["face_provenance"].as_array().unwrap();
        for (index, face) in faces.iter().enumerate() {
            let row = rows
                .iter()
                .find(|row| row["child_side"] == face.side)
                .unwrap();
            assert_eq!(row["owner"], format!("entity:{id}"));
            assert_eq!(row["file"], "child.vmf");
            assert_eq!(row["child_brush"], face.solid);
            assert_eq!(row["terrain"], true);
            assert_eq!(row["power"], if index == 0 { 2 } else { 3 });
            assert_eq!(row["vertices"], if index == 0 { 25 } else { 81 });
            assert_eq!(row["triangles"], if index == 0 { 32 } else { 128 });
        }
        let ready = session.instance_contents.ready.as_ref().unwrap();
        let vertex = ready
            .mesh
            .vertices
            .iter()
            .find(|vertex| {
                DVec3::from_array(vertex.position.map(f64::from))
                    .distance(DVec3::new(0.0, 32.0, 96.0))
                    < 1e-5
            })
            .unwrap();
        let source_side = vmf.world.solids[0]
            .sides
            .iter()
            .find(|side| side.id == faces[0].side)
            .unwrap();
        assert_eq!(
            vertex.uv,
            map_core::texel_uv(source_side, DVec3::new(64.0, 64.0, 16.0))
                .as_vec2()
                .to_array()
        );
        let bounds = ready.owners[0].bounds.unwrap();
        assert!(bounds[0].distance(DVec3::new(-64.0, -32.0, 64.0)) < 1e-5);
        assert!(bounds[1].distance(DVec3::new(64.0, 288.0, 96.0)) < 1e-5);
        let hits = session.instance_contents_hits(
            &Ray {
                origin: DVec3::new(0.0, 32.0, 120.0),
                direction: DVec3::NEG_Z,
            },
            Some(true),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].object, owner);
        assert!((hits[0].point.z - 96.0).abs() < 1e-5);
        assert_eq!((hits[0].solid, hits[0].side), (None, None));
        assert!(
            session
                .instance_contents_hits(
                    &Ray {
                        origin: DVec3::new(0.0, 32.0, 60.0),
                        direction: DVec3::NEG_Z,
                    },
                    Some(true)
                )
                .is_empty()
        );
        assert_eq!(session.document.vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.selected_ids(), vec![owner]);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(json!(session.property_draft()), draft);
        assert_eq!(std::fs::read(&source).unwrap(), disk);
        assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
        assert_eq!(
            std::fs::read(source.with_extension("bsp")).unwrap(),
            b"previous BSP"
        );
        assert_eq!(
            std::fs::read(source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
    }
}

fn curved(document: Document) -> Vmf {
    let mut vmf = document.into_vmf();
    for solid in &mut vmf.world.solids {
        let geometry = BrushGeometry::from_solid(solid).unwrap();
        let face = geometry
            .faces
            .iter()
            .find(|face| solid.sides[face.side].displacement.is_some())
            .unwrap();
        let info = solid.sides[face.side].displacement.as_mut().unwrap();
        let mesh = map_core::DisplacementMesh::build(face, info).unwrap();
        let size = mesh.size;
        info.normals = Some(vec![vec![DVec3::Z; size]; size]);
        info.distances = Some(
            mesh.base_positions
                .chunks(size)
                .map(|row| row.iter().map(|point| point.y.powi(2) / 128.0).collect())
                .collect(),
        );
    }
    vmf
}

#[test]
fn child_terrain_retopology_uses_the_expanded_parent_neighborhood() {
    let (_directory, mut session, source, child, _owner) = opened(Actor::Agent);
    let mut document = Document::empty();
    patch(
        &mut document,
        DVec3::new(128.0, 0.0, 0.0),
        DVec3::new(256.0, 128.0, 16.0),
        4,
    );
    let child_vmf = curved(document);
    std::fs::write(&child, child_vmf.write(&Default::default())).unwrap();
    let mut document = Document::empty();
    let parent = patch(
        &mut document,
        DVec3::ZERO,
        DVec3::new(128.0, 128.0, 16.0),
        2,
    );
    let mut document = Document::new(curved(document));
    let owner = document
        .edit("instance", |tx| {
            edit::create_entity(
                tx,
                "func_instance",
                Some(DVec3::ZERO),
                &[("file".into(), "child.vmf".into())],
                Vec::new(),
            )
        })
        .unwrap();
    std::fs::write(&source, document.vmf().write(&Default::default())).unwrap();
    session.open(Actor::Agent, &source, true).unwrap();
    let memory = session.document.vmf().clone();
    let report = refresh(&mut session, Actor::Agent);
    assert_eq!(report["state"], "ready", "{report}");
    assert_eq!(report["geometry"]["vertices"], 289);
    assert_eq!(report["geometry"]["triangles"], 452);
    assert_eq!(report["face_provenance_total"], 1);
    assert_eq!(report["face_provenance"][0]["power"], 4);
    assert_eq!(report["face_provenance"][0]["triangles"], 452);
    let mesh = session.instance_contents_preview(true).mesh.unwrap();
    assert!(
        mesh.vertices
            .iter()
            .any(|vertex| vertex.position == [128.0, 8.0, 18.0])
    );
    assert!(
        !mesh
            .vertices
            .iter()
            .any(|vertex| vertex.position == [128.0, 8.0, 16.5])
    );
    let hits = session.instance_contents_hits(
        &Ray {
            origin: DVec3::new(128.0, 8.0, 256.0),
            direction: DVec3::NEG_Z,
        },
        Some(true),
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].object, owner);
    assert!((hits[0].point.z - 18.0).abs() < 1e-6);
    assert_eq!(session.document.vmf(), &memory);
    session
        .edit(Actor::Agent, "invalid parent terrain", |tx| {
            let side = tx
                .solid_mut(parent.solid)?
                .sides
                .iter_mut()
                .find(|side| side.id == parent.side)
                .unwrap();
            side.displacement
                .as_mut()
                .unwrap()
                .extra_properties
                .set("smooth", "broken");
            Ok(())
        })
        .unwrap();
    let memory = session.document.vmf().clone();
    let report = refresh(&mut session, Actor::Agent);
    assert_eq!(report["state"], "failed", "{report}");
    assert!(report["error"].as_str().unwrap().contains("smooth"));
    assert!(session.instance_contents_preview(true).mesh.is_none());
    assert!(report["geometry"].is_null());
    assert_eq!(session.document.vmf(), &memory);
}

#[test]
fn child_coarse_terrain_requiring_new_parent_retopology_is_explicitly_unavailable() {
    let (_directory, mut session, source, child, _owner) = opened(Actor::Agent);
    let mut document = Document::empty();
    patch(
        &mut document,
        DVec3::new(128.0, 0.0, 0.0),
        DVec3::new(256.0, 128.0, 16.0),
        2,
    );
    std::fs::write(&child, curved(document).write(&Default::default())).unwrap();
    let mut document = Document::empty();
    patch(
        &mut document,
        DVec3::ZERO,
        DVec3::new(128.0, 128.0, 16.0),
        4,
    );
    let mut document = Document::new(curved(document));
    document
        .edit("instance", |tx| {
            edit::create_entity(
                tx,
                "func_instance",
                Some(DVec3::ZERO),
                &[("file".into(), "child.vmf".into())],
                Vec::new(),
            )
        })
        .unwrap();
    std::fs::write(&source, document.vmf().write(&Default::default())).unwrap();
    session.open(Actor::Agent, &source, true).unwrap();
    let memory = session.document.vmf().clone();
    let root_bytes = std::fs::read(&source).unwrap();
    let child_bytes = std::fs::read(&child).unwrap();
    let control = compiler::Control::default();
    let context = instances::SourceContext::capture(&source, &control).unwrap();
    assert!(
        instances::resolve(
            &memory,
            Some(&context),
            &control,
            &mut compiler::Report::default()
        )
        .is_ok()
    );
    let report = refresh(&mut session, Actor::Agent);
    assert_eq!(report["state"], "failed", "{report}");
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .contains("replacing ordinary parent terrain LOD")
    );
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .contains("compilation remains available")
    );
    assert!(report["geometry"].is_null());
    assert!(session.instance_contents_preview(true).mesh.is_none());
    assert_eq!(session.document.vmf(), &memory);
    assert_eq!(std::fs::read(&source).unwrap(), root_bytes);
    assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
}

#[test]
fn unrelated_existing_parent_lod_and_flat_instance_contents_remain_available() {
    let (_directory, mut session, source, child, _owner) = opened(Actor::Agent);
    let mut document = Document::empty();
    patch(
        &mut document,
        DVec3::new(512.0, 0.0, 0.0),
        DVec3::new(640.0, 128.0, 16.0),
        2,
    );
    std::fs::write(&child, curved(document).write(&Default::default())).unwrap();
    let mut document = Document::empty();
    patch(
        &mut document,
        DVec3::ZERO,
        DVec3::new(128.0, 128.0, 16.0),
        2,
    );
    let fine = patch(
        &mut document,
        DVec3::new(128.0, 0.0, 0.0),
        DVec3::new(256.0, 128.0, 16.0),
        4,
    );
    let mut document = Document::new(curved(document));
    document
        .edit("instance", |tx| {
            edit::create_entity(
                tx,
                "func_instance",
                Some(DVec3::ZERO),
                &[("file".into(), "child.vmf".into())],
                Vec::new(),
            )
        })
        .unwrap();
    std::fs::write(&source, document.vmf().write(&Default::default())).unwrap();
    session.open(Actor::Agent, &source, true).unwrap();
    let memory = session.document.vmf().clone();
    let ordinary = map_core::displacement::surface::prepare(&memory);
    assert!(
        ordinary
            .get(&fine)
            .unwrap()
            .positions
            .contains(&DVec3::new(128.0, 8.0, 18.0))
    );
    let report = refresh(&mut session, Actor::Agent);
    assert_eq!(report["state"], "ready", "{report}");
    assert_eq!(report["geometry"]["vertices"], 25);
    assert_eq!(report["geometry"]["triangles"], 32);
    assert_eq!(report["face_provenance_total"], 1);
    let mut flat = Document::empty();
    flat.edit("flat child", |tx| {
        edit::create_block(
            tx,
            DVec3::new(512.0, 0.0, 0.0),
            DVec3::new(640.0, 128.0, 16.0),
            "test/terrain",
        )
    })
    .unwrap();
    std::fs::write(&child, flat.vmf().write(&Default::default())).unwrap();
    let report = refresh(&mut session, Actor::Agent);
    assert_eq!(report["state"], "ready", "{report}");
    assert_eq!(report["geometry"]["vertices"], 24);
    assert_eq!(report["geometry"]["triangles"], 12);
    assert_eq!(report["face_provenance_total"], 6);
    assert!(
        report["face_provenance"]
            .as_array()
            .unwrap()
            .iter()
            .all(|face| face.get("terrain").is_none())
    );
    assert_eq!(
        report["policy"],
        "flat_read_only_world_detail_root_owner_picking_no_material_loading"
    );
    assert_eq!(session.document.vmf(), &memory);
}

#[test]
fn nested_terrain_keeps_root_owner_and_distinct_leaf_names_for_repeated_source_ids() {
    let (_directory, mut session, source, middle, leaves, owner) = nested_opened(Actor::Agent);
    let (vmf, faces) = hill_child();
    for leaf in &leaves {
        std::fs::write(leaf, vmf.write(&Default::default())).unwrap();
    }
    let mut module = Vmf::parse(&std::fs::read_to_string(&middle).unwrap()).unwrap();
    module.entities[1].properties.set("origin", "512 0 0");
    std::fs::write(&middle, module.write(&Default::default())).unwrap();
    let memory = session.document.vmf().clone();
    let root_bytes = std::fs::read(&source).unwrap();
    let middle_bytes = std::fs::read(&middle).unwrap();
    let leaf_bytes = leaves.each_ref().map(|leaf| std::fs::read(leaf).unwrap());
    let history = session.history_snapshot();
    let report = refresh(&mut session, Actor::Agent);
    assert_eq!(report["state"], "ready", "{report}");
    assert_eq!(report["stats"]["instances"], 3);
    assert_eq!(report["stats"]["max_expansion_depth"], 2);
    assert_eq!(report["owners"][0]["file"], "modules/middle.vmf");
    assert_eq!(report["geometry"]["vertices"], 212);
    assert_eq!(report["geometry"]["triangles"], 320);
    assert_eq!(report["face_provenance_total"], 4);
    let ObjectId::Entity(owner) = owner else {
        unreachable!()
    };
    let rows = report["face_provenance"].as_array().unwrap();
    for file in ["leaf-a.vmf", "leaf-b.vmf"] {
        for face in faces {
            let row = rows
                .iter()
                .find(|row| row["file"] == file && row["child_side"] == face.side)
                .unwrap();
            assert_eq!(row["child_brush"], face.solid);
            assert_eq!(row["owner"], format!("entity:{owner}"));
            assert_eq!(row["terrain"], true);
        }
    }
    assert_eq!(session.document.vmf(), &memory);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(std::fs::read(&source).unwrap(), root_bytes);
    assert_eq!(std::fs::read(&middle).unwrap(), middle_bytes);
    assert_eq!(
        leaves.each_ref().map(|leaf| std::fs::read(leaf).unwrap()),
        leaf_bytes
    );
}

#[test]
fn failed_terrain_refresh_has_no_cage_fallback_and_preserves_both_document_drafts() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, source, child, owner) = opened(actor);
        let (vmf, _) = hill_child();
        std::fs::write(&child, vmf.write(&Default::default())).unwrap();
        assert_eq!(refresh(&mut session, actor)["state"], "ready");
        let ObjectId::Entity(id) = owner else {
            unreachable!()
        };
        let draft = session.begin_instance_reference(actor, Some(id)).unwrap();
        let draft = session
            .update_instance_reference(
                actor,
                draft["token"].as_u64().unwrap(),
                crate::session::instance_reference::Changes {
                    origin: Some("21.5 -7.25 3.125".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let mut peer = Session::new(Err("preview needs no mounted materials".into()));
        peer.open(actor, &source, true).unwrap();
        peer.begin_property_edit(crate::session::PropertyTarget::World)
            .unwrap();
        let token = peer.property_draft().unwrap().token;
        peer.append_property_change(
            token,
            crate::session::PropertyChange::Set {
                key: "message".into(),
                value: "peer draft".into(),
            },
        )
        .unwrap();
        let peer_draft = json!(peer.property_draft());
        let memory = session.document.vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let settings = session.compile_settings();
        let source_bytes = std::fs::read(&source).unwrap();
        for invalid in ["unpaired", "smooth"] {
            let mut child_vmf = vmf.clone();
            let info = child_vmf.world.solids[0]
                .sides
                .iter_mut()
                .find_map(|side| side.displacement.as_mut())
                .unwrap();
            if invalid == "unpaired" {
                info.normals = None;
            } else {
                info.extra_properties.set("smooth", "broken");
            }
            std::fs::write(&child, child_vmf.write(&Default::default())).unwrap();
            let child_bytes = std::fs::read(&child).unwrap();
            let report = refresh(&mut session, actor);
            assert_eq!(report["state"], "failed", "{report}");
            assert!(report["error"].as_str().is_some());
            assert!(report["geometry"].is_null());
            assert_eq!(report["face_provenance_total"], 0);
            assert!(session.instance_contents_preview(true).mesh.is_none());
            assert!(
                session
                    .instance_contents_hits(
                        &Ray {
                            origin: DVec3::new(64.0, 64.0, 128.0),
                            direction: DVec3::NEG_Z,
                        },
                        Some(true)
                    )
                    .is_empty()
            );
            assert!(
                session
                    .frame_instance_contents(
                        actor,
                        report["token"].as_u64().unwrap(),
                        &[owner],
                        1.5
                    )
                    .is_err()
            );
            assert_eq!(session.get_instance_reference(), draft);
            assert_eq!(json!(peer.property_draft()), peer_draft);
            assert_eq!(session.document.vmf(), &memory);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.camera(), camera);
            assert_eq!(session.compile_settings(), settings);
            assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
            assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
            assert_eq!(
                std::fs::read(source.with_extension("bsp")).unwrap(),
                b"previous BSP"
            );
            assert_eq!(
                std::fs::read(source.with_extension("lin")).unwrap(),
                b"previous pointfile"
            );
        }
    }
}

#[test]
fn finite_collapsed_child_triangles_reject_the_whole_ready_layer_and_preserve_drafts() {
    for actor in [Actor::User, Actor::Agent] {
        for whole_grid in [false, true] {
            let (_directory, mut session, source, child, owner) = opened(actor);
            let (valid, faces) = hill_child();
            std::fs::write(&child, valid.write(&Default::default())).unwrap();
            session.select(&[owner], SelectMode::Replace).unwrap();
            let ray = Ray {
                origin: DVec3::new(256.0, 64.0, 128.0),
                direction: DVec3::NEG_Z,
            };
            let ready = refresh(&mut session, actor);
            assert_eq!(ready["state"], "ready", "{ready}");
            assert_eq!(ready["available"], true);
            let hits = session.instance_contents_hits(&ray, Some(true));
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].object, owner);
            assert!((hits[0].point.z - 48.0).abs() < 1e-6);
            let ObjectId::Entity(id) = owner else {
                unreachable!()
            };
            let form = session.begin_instance_reference(actor, Some(id)).unwrap();
            let form = session
                .update_instance_reference(
                    actor,
                    form["token"].as_u64().unwrap(),
                    crate::session::instance_reference::Changes {
                        origin: Some("21.5 -7.25 3.125".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
            let mut peer = Session::new(Err("preview needs no mounted materials".into()));
            peer.open(actor, &source, true).unwrap();
            peer.begin_property_edit(crate::session::PropertyTarget::World)
                .unwrap();
            let token = peer.property_draft().unwrap().token;
            peer.append_property_change(
                token,
                crate::session::PropertyChange::Set {
                    key: "message".into(),
                    value: "retained peer draft".into(),
                },
            )
            .unwrap();
            let peer_draft = json!(peer.property_draft());
            let memory = session.document.vmf().clone();
            let history = session.history_snapshot();
            let camera = session.camera();
            let settings = session.compile_settings();
            let root_bytes = std::fs::read(&source).unwrap();
            let mut invalid = valid.clone();
            let solid = invalid
                .world
                .solids
                .iter_mut()
                .find(|solid| solid.id == faces[0].solid)
                .unwrap();
            let geometry = BrushGeometry::from_solid(solid).unwrap();
            let face = geometry
                .faces
                .iter()
                .find(|face| solid.sides[face.side].id == faces[0].side)
                .unwrap();
            let info = solid.sides[face.side].displacement.as_mut().unwrap();
            let mesh = map_core::DisplacementMesh::build(face, info).unwrap();
            let [a, b, _] = mesh.triangles().next().unwrap();
            let mut offsets = vec![vec![DVec3::ZERO; mesh.size]; mesh.size];
            for (index, point) in mesh.positions.iter().enumerate() {
                if whole_grid || index == b {
                    offsets[index / mesh.size][index % mesh.size] = mesh.positions[a] - *point;
                }
            }
            info.offsets = Some(offsets);
            let collapsed = map_core::DisplacementMesh::build(face, info).unwrap();
            assert!(collapsed.positions.iter().all(|point| point.is_finite()));
            assert_eq!(collapsed.positions[a], collapsed.positions[b]);
            std::fs::write(&child, invalid.write(&Default::default())).unwrap();
            let child_bytes = std::fs::read(&child).unwrap();
            let request = session.begin_instance_contents_refresh(actor).unwrap();
            let loading = session.inspect_instance_contents(0, 64).unwrap();
            assert_eq!(loading["state"], "loading");
            assert_eq!(loading["available"], false);
            assert!(session.instance_contents.ready.is_none());
            assert!(session.instance_contents_hits(&ray, Some(true)).is_empty());
            let report = session
                .finish_instance_contents_refresh(actor, request.run())
                .unwrap();
            assert_eq!(
                report["state"], "failed",
                "whole_grid={whole_grid}: {report}"
            );
            assert!(
                report["error"]
                    .as_str()
                    .unwrap()
                    .contains("collapsed triangles")
            );
            assert_eq!(report["available"], false);
            assert_eq!(report["loading"], false);
            assert!(!session.instance_workers_busy());
            assert!(report["geometry"].is_null());
            assert_eq!(report["face_provenance_total"], 0);
            assert!(session.instance_contents.ready.is_none());
            assert!(session.instance_contents_preview(true).mesh.is_none());
            assert!(session.instance_contents_hits(&ray, Some(true)).is_empty());
            assert!(
                session
                    .frame_instance_contents(
                        actor,
                        report["token"].as_u64().unwrap(),
                        &[owner],
                        1.5
                    )
                    .is_err()
            );
            assert_eq!(session.get_instance_reference(), form);
            assert_eq!(json!(peer.property_draft()), peer_draft);
            assert_eq!(session.document.vmf(), &memory);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.camera(), camera);
            assert_eq!(session.selected_ids(), vec![owner]);
            assert_eq!(session.compile_settings(), settings);
            assert_eq!(std::fs::read(&source).unwrap(), root_bytes);
            assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
            assert_eq!(
                std::fs::read(source.with_extension("bsp")).unwrap(),
                b"previous BSP"
            );
            assert_eq!(
                std::fs::read(source.with_extension("lin")).unwrap(),
                b"previous pointfile"
            );
            std::fs::write(&child, valid.write(&Default::default())).unwrap();
            let recovered = refresh(&mut session, actor);
            assert_eq!(recovered["state"], "ready", "{recovered}");
            assert_eq!(recovered["geometry"], ready["geometry"]);
            assert_eq!(recovered["available"], true);
            assert_eq!(session.get_instance_reference(), form);
            assert_eq!(json!(peer.property_draft()), peer_draft);
            let hits = session.instance_contents_hits(&ray, Some(true));
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].object, owner);
            assert!((hits[0].point.z - 48.0).abs() < 1e-6);
        }
    }
}

#[test]
fn hidden_unsupported_terrain_is_excluded_but_visible_terrain_rejects_refresh() {
    for actor in [Actor::User, Actor::Agent] {
        for hidden_entity in [false, true] {
            let (_directory, mut session, source, child, owner) = opened(actor);
            let mut document = Document::empty();
            document
                .edit("visible flat sibling", |tx| {
                    edit::create_block(tx, DVec3::ZERO, DVec3::splat(16.0), "test/terrain")
                })
                .unwrap();
            let visible = patch(
                &mut document,
                DVec3::new(64.0, 0.0, 0.0),
                DVec3::new(192.0, 128.0, 16.0),
                2,
            );
            let hidden = patch(
                &mut document,
                DVec3::new(256.0, 0.0, 0.0),
                DVec3::new(384.0, 128.0, 16.0),
                2,
            );
            let mut vmf = document.into_vmf();
            let mut unsupported = vmf.world.solids.pop().unwrap();
            assert_eq!(unsupported.id, hidden.solid);
            unsupported
                .sides
                .iter_mut()
                .find_map(|side| side.displacement.as_mut())
                .unwrap()
                .extra_properties
                .set("unsupported_hidden", "1");
            if hidden_entity {
                let id = vmf
                    .object_ids()
                    .chain(vmf.side_ids())
                    .chain([unsupported.id])
                    .chain(unsupported.sides.iter().map(|side| side.id))
                    .max()
                    .unwrap()
                    + 1;
                let mut entity = source_formats::vmf::Entity::new(id, "func_detail");
                entity.hidden = true;
                entity.solids.push(unsupported);
                vmf.entities.push(entity);
            } else {
                unsupported.hidden = true;
                vmf.world.solids.push(unsupported);
            }
            std::fs::write(&child, vmf.write(&Default::default())).unwrap();
            let hidden_bytes = std::fs::read(&child).unwrap();
            session.select(&[owner], SelectMode::Replace).unwrap();
            session
                .begin_property_edit(crate::session::PropertyTarget::World)
                .unwrap();
            let token = session.property_draft().unwrap().token;
            session
                .append_property_change(
                    token,
                    crate::session::PropertyChange::Set {
                        key: "message".into(),
                        value: "retained parent draft".into(),
                    },
                )
                .unwrap();
            let draft = json!(session.property_draft());
            let memory = session.document.vmf().clone();
            let history = session.history_snapshot();
            let camera = session.camera();
            let settings = session.compile_settings();
            let root_bytes = std::fs::read(&source).unwrap();
            let ray = Ray {
                origin: DVec3::new(128.0, 64.0, 128.0),
                direction: DVec3::NEG_Z,
            };
            let report = refresh(&mut session, actor);
            assert_eq!(
                report["state"], "ready",
                "hidden_entity={hidden_entity}: {report}"
            );
            assert_eq!(report["available"], true);
            assert_eq!(report["stats"]["world_brushes"], 2);
            assert_eq!(report["stats"]["detail_brushes"], 0);
            assert_eq!(report["stats"]["terrain_patches"], 1);
            assert_eq!(report["stats"]["terrain_vertices"], 25);
            assert_eq!(report["geometry"]["vertices"], 49);
            assert_eq!(report["geometry"]["triangles"], 44);
            assert_eq!(report["face_provenance_total"], 7);
            let rows = report["face_provenance"].as_array().unwrap();
            assert!(
                rows.iter()
                    .any(|row| row["child_side"] == visible.side && row["terrain"] == true)
            );
            assert!(rows.iter().all(|row| row["child_brush"] != hidden.solid));
            let hits = session.instance_contents_hits(&ray, Some(true));
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].object, owner);
            assert!((hits[0].point.z - 16.0).abs() < 1e-6);
            assert!(
                session
                    .instance_contents_hits(
                        &Ray {
                            origin: DVec3::new(320.0, 64.0, 128.0),
                            direction: DVec3::NEG_Z,
                        },
                        Some(true)
                    )
                    .is_empty()
            );
            assert_eq!(std::fs::read(&child).unwrap(), hidden_bytes);
            if hidden_entity {
                vmf.entities.last_mut().unwrap().hidden = false;
            } else {
                vmf.world.solids.last_mut().unwrap().hidden = false;
            }
            std::fs::write(&child, vmf.write(&Default::default())).unwrap();
            let visible_bytes = std::fs::read(&child).unwrap();
            let failed = refresh(&mut session, actor);
            assert_eq!(failed["state"], "failed", "{failed}");
            assert!(
                failed["error"]
                    .as_str()
                    .unwrap()
                    .contains("unsupported_hidden")
            );
            assert_eq!(failed["available"], false);
            assert_eq!(failed["loading"], false);
            assert!(!session.instance_workers_busy());
            assert!(failed["geometry"].is_null());
            assert_eq!(failed["face_provenance_total"], 0);
            assert!(session.instance_contents.ready.is_none());
            assert!(session.instance_contents_preview(true).mesh.is_none());
            assert!(session.instance_contents_hits(&ray, Some(true)).is_empty());
            assert!(
                session
                    .frame_instance_contents(
                        actor,
                        failed["token"].as_u64().unwrap(),
                        &[owner],
                        1.5
                    )
                    .is_err()
            );
            assert_eq!(json!(session.property_draft()), draft);
            assert_eq!(session.document.vmf(), &memory);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.camera(), camera);
            assert_eq!(session.selected_ids(), vec![owner]);
            assert_eq!(session.compile_settings(), settings);
            assert_eq!(std::fs::read(&source).unwrap(), root_bytes);
            assert_eq!(std::fs::read(&child).unwrap(), visible_bytes);
            assert_eq!(
                std::fs::read(source.with_extension("bsp")).unwrap(),
                b"previous BSP"
            );
            assert_eq!(
                std::fs::read(source.with_extension("lin")).unwrap(),
                b"previous pointfile"
            );
            std::fs::write(&child, &hidden_bytes).unwrap();
            let recovered = refresh(&mut session, actor);
            assert_eq!(recovered["state"], "ready", "{recovered}");
            assert_eq!(recovered["geometry"], report["geometry"]);
            assert_eq!(json!(session.property_draft()), draft);
            assert_eq!(session.instance_contents_hits(&ray, Some(true)).len(), 1);
        }
    }
}
