mod common;
#[path = "common/overlays.rs"]
mod overlay_fixture;

use glam::DVec3;
use map_core::{Document, ObjectId, Plane, compile_region, edit, surface::FaceId};
use source_formats::bsp::{self, Bsp, Face, WaterOverlayData};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::keyvalues::{Node, Value};
use source_formats::vmf::{Entity, Vmf};

fn transition(mut source: Vmf, sides: &[u32]) -> Vmf {
    let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
    let mut entity = Entity::new(id, "info_overlay_transition");
    entity.properties.set("origin", "1000 0 0");
    entity.properties.set("targetname", "transition_controller");
    let sides = sides
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    let fields = [
        ("material", "decals/decalgraffiti001a"),
        ("sides", sides.as_str()),
        ("BasisOrigin", "0 0 32"),
        ("BasisNormal", "0 0 1"),
        ("BasisU", "1 0 0"),
        ("BasisV", "0 -1 0"),
        ("StartU", "1"),
        ("EndU", "0"),
        ("StartV", "0"),
        ("EndV", "2"),
        ("uv0", "-224 -224 17"),
        ("uv1", "-224 224 9"),
        ("uv2", "224 224 7"),
        ("uv3", "224 -224 13"),
    ]
    .map(|(key, value)| Node::string(key, value))
    .to_vec();
    entity.extra.push(Node::block(
        "overlaytransition",
        vec![Node::block("overlaydata", fields)],
    ));
    source.entities.push(entity);
    Vmf::parse(&source.write(&Default::default())).unwrap()
}

fn fields(source: &mut Vmf) -> &mut Vec<Node> {
    let Value::Block(transition) = &mut source.entities.last_mut().unwrap().extra[0].value else {
        unreachable!()
    };
    let Value::Block(fields) = &mut transition[0].value else {
        unreachable!()
    };
    fields
}

fn set_field(source: &mut Vmf, key: &str, value: &str) {
    let fields = fields(source);
    if let Some(field) = fields
        .iter_mut()
        .find(|field| field.key.eq_ignore_ascii_case(key))
    {
        field.value = Value::String(value.into());
    } else {
        fields.push(Node::string(key, value));
    }
}

fn fixture() -> (Vmf, [FaceId; 3]) {
    let mut document = Document::new(common::room());
    let faces = document
        .edit("transition targets", |tx| {
            let mut faces = Vec::new();
            for (index, x) in [-144.0, -48.0, 48.0].into_iter().enumerate() {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(x, 32.0, 0.0),
                    DVec3::new(x + 80.0, 112.0, 32.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let side = tx
                    .solid_mut(id)?
                    .sides
                    .iter()
                    .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                    .unwrap()
                    .id;
                let face = FaceId { solid: id, side };
                if index == 1 {
                    edit::tie_to_entity(tx, &[id], "func_detail", &[])?;
                }
                if index == 2 {
                    map_core::displacement::edit::create(tx, &[face], 2, 0)?;
                }
                faces.push(face);
            }
            Ok(faces.try_into().unwrap())
        })
        .unwrap();
    (document.into_vmf(), faces)
}

fn compile(source: &Vmf, game: &GameFs) -> (Bsp, WaterOverlayData) {
    let before = source.clone();
    let output = compiler::compile(source, game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    assert_eq!(source, &before);
    let bsp = Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap();
    let data = bsp.water_overlay_data().unwrap().unwrap();
    let normal = bsp.overlay_data().unwrap().unwrap_or_default();
    assert_eq!(
        output.stats.overlays,
        data.overlays.len() + normal.overlays.len()
    );
    assert_eq!(
        output.stats.overlay_face_refs,
        data.overlays
            .iter()
            .map(|record| record.face_count())
            .sum::<usize>()
            + normal
                .overlays
                .iter()
                .map(|record| record.face_count())
                .sum::<usize>()
    );
    assert!(!bsp.entities().unwrap().contains("info_overlay_transition"));
    assert!(!bsp.entities().unwrap().contains("transition_controller"));
    (bsp, data)
}

fn reject(source: &Vmf, game: &GameFs, destination: &std::path::Path) {
    let before = source.clone();
    let previous = std::fs::read(destination).unwrap();
    for controlled in [false, true] {
        let output = if controlled {
            compiler::compile_controlled(source, game, &compiler::Control::default())
        } else {
            compiler::compile(source, game)
        };
        assert!(
            output.bsp.is_none() && output.report.has_errors(),
            "{:?}",
            output.report
        );
        assert!(!output.write(destination).unwrap());
        assert_eq!(std::fs::read(destination).unwrap(), previous);
        assert_eq!(source, &before);
    }
}

#[test]
fn imported_transition_binds_world_detail_and_terrain_and_keeps_regular_overlay_ids_fades_separate()
{
    let (source, faces) = fixture();
    for chosen in [&faces[..1], &faces[1..2], &faces[2..], &faces[..]] {
        let source = overlay_fixture::add_overlay(source.clone(), &[faces[0].side]);
        let source = transition(
            source,
            &chosen.iter().map(|face| face.side).collect::<Vec<_>>(),
        );
        let (bsp, data) = compile(&source, &common::empty_game());
        let record = data.overlays[0];
        assert_eq!(record.face_count(), chosen.len());
        assert_eq!(record.u, [1.0, 0.0]);
        assert_eq!(record.v, [0.0, 2.0]);
        assert_eq!(record.render_order(), 0);
        assert_eq!(record.uv_points.map(|point| point[2]), [1.0, 0.0, 0.0, 1.0]);
        let native = bsp.records::<Face>().unwrap();
        let planes = bsp.records::<bsp::Plane>().unwrap();
        for &face in &record.faces[..record.face_count()] {
            assert_eq!(
                planes[native[face as usize].plane as usize].normal,
                [0.0, 0.0, 1.0]
            );
            assert_eq!(planes[native[face as usize].plane as usize].dist, 32.0);
        }
        if chosen.contains(&faces[2]) {
            assert!(record.faces[..record.face_count()].contains(&i32::from(
                bsp.displacements().unwrap().unwrap().infos[0].face
            )));
        }
        let normal = bsp.overlay_data().unwrap().unwrap();
        assert_eq!(normal.overlays.len(), 1);
        assert_eq!(normal.fades.len(), 1);
        assert!(bsp.entities().unwrap().contains("\"OverlayID\" \"0\""));
        let reopened = Vmf::parse(&source.write(&Default::default())).unwrap();
        assert_eq!(
            compile(&reopened, &common::empty_game()).0.to_bytes(),
            bsp.to_bytes()
        );
    }
}

#[test]
fn transition_faces_above_regular_64_face_budget_use_their_own_256_slots() {
    let mut source = common::room();
    let top = source.world.solids[0]
        .sides
        .iter_mut()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap();
    top.lightmap_scale = 1;
    let side = top.id;
    let source = transition(source, &[side]);
    let (_, data) = compile(&source, &common::empty_game());
    assert!(data.overlays[0].face_count() > bsp::MAX_OVERLAY_FACES);
    assert!(data.overlays[0].face_count() <= bsp::MAX_WATER_OVERLAY_FACES);
}

#[test]
fn water_membership_prevents_projection_bindings_spilling_across_adjacent_world_faces() {
    let mut document = Document::new(common::room());
    let side = document
        .edit("adjacent receivers", |tx| {
            let mut selected = None;
            for x in [-96.0, 0.0] {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(x, 32.0, 0.0),
                    DVec3::new(x + 96.0, 128.0, 16.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                if selected.is_none() {
                    selected = tx
                        .solid_mut(id)?
                        .sides
                        .iter()
                        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                        .map(|side| side.id);
                }
            }
            Ok(selected.unwrap())
        })
        .unwrap();
    let (bsp, data) = compile(
        &transition(document.into_vmf(), &[side]),
        &common::empty_game(),
    );
    let faces = bsp.records::<Face>().unwrap();
    let vertices = bsp.values::<[f32; 3]>(bsp::lump::VERTEXES).unwrap();
    let edges = bsp.records::<bsp::Edge>().unwrap();
    let surfedges = bsp.values::<i32>(bsp::lump::SURFEDGES).unwrap();
    for &index in &data.overlays[0].faces[..data.overlays[0].face_count()] {
        let face = faces[index as usize];
        for &edge in &surfedges
            [face.first_edge as usize..face.first_edge as usize + face.edge_count as usize]
        {
            let vertex = edges[edge.unsigned_abs() as usize].0[usize::from(edge < 0)];
            assert!(vertices[vertex as usize][0] <= 0.0);
            assert_eq!(vertices[vertex as usize][2], 16.0);
        }
    }
}

#[test]
fn water_records_do_not_add_or_shift_distinct_regular_fade_entries() {
    let (source, faces) = fixture();
    let mut source = overlay_fixture::add_overlay(source, &[faces[0].side]);
    source
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("fademindist", "16");
    source
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("fademaxdist", "32");
    source = overlay_fixture::add_overlay(source, &[faces[1].side]);
    source
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("fademindist", "64");
    source
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("fademaxdist", "128");
    let without_water = compiler::compile(&source, &common::empty_game())
        .bsp
        .unwrap();
    let (with_water, _) = compile(&transition(source, &[faces[2].side]), &common::empty_game());
    let normal = with_water.overlay_data().unwrap().unwrap();
    assert_eq!(normal.overlays.len(), 2);
    assert_eq!(
        normal
            .fades
            .iter()
            .map(|fade| [fade.min_distance_squared, fade.max_distance_squared])
            .collect::<Vec<_>>(),
        vec![[256.0, 1024.0], [4096.0, 16384.0]]
    );
    assert_eq!(
        normal
            .overlays
            .iter()
            .map(|record| record.id)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        with_water.lumps[bsp::lump::OVERLAY_FADES],
        without_water.lumps[bsp::lump::OVERLAY_FADES]
    );
    assert_eq!(
        with_water.lumps[bsp::lump::OVERLAY_FADES].data.len(),
        2 * size_of::<bsp::OverlayFade>()
    );
}

#[test]
fn transition_record_and_compiled_face_budgets_reject_before_publication() {
    let (source, faces) = fixture();
    let valid = transition(source, &[faces[0].side]);
    let previous = compile(&valid, &common::empty_game()).0.to_bytes();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, previous).unwrap();
    let mut excessive = valid.clone();
    let helper = excessive.entities.last_mut().unwrap();
    let Value::Block(children) = &mut helper.extra[0].value else {
        unreachable!()
    };
    children.resize(bsp::MAX_WATER_OVERLAYS + 1, children[0].clone());
    reject(&excessive, &common::empty_game(), &path);
    let mut repeated = common::room();
    let floor = repeated.world.solids[0]
        .sides
        .iter_mut()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap();
    floor.lightmap_scale = 1;
    let side = floor.id;
    let mut repeated = transition(repeated, &[side]);
    let Value::Block(children) = &mut repeated.entities.last_mut().unwrap().extra[0].value else {
        unreachable!()
    };
    children.resize(257, children[0].clone());
    reject(&repeated, &common::empty_game(), &path);
    let mut large = common::room();
    let mut sides = Vec::new();
    for index in [0, 1] {
        for side in &mut large.world.solids[index].sides {
            let normal = Plane::from_points(side.plane.0).unwrap().normal;
            if (index == 0 && normal.z > 0.99) || (index == 1 && normal.z < -0.99) {
                side.lightmap_scale = 1;
                sides.push(side.id);
            }
        }
    }
    assert_eq!(sides.len(), 2);
    reject(&transition(large, &sides), &common::empty_game(), &path);
}

#[test]
fn malformed_hidden_model_buried_and_missing_final_targets_preserve_published_output() {
    let (base, faces) = fixture();
    let valid = transition(base.clone(), &faces.map(|face| face.side));
    let (bsp, _) = compile(&valid, &common::empty_game());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, bsp.to_bytes()).unwrap();
    for (key, value) in [
        ("material", "../invalid"),
        ("BasisOrigin", "nan 0 0"),
        ("BasisU", "0 0 0"),
        ("BasisV", "1 0 0"),
        ("uv1", "-224 -224 0"),
        ("sides", ""),
        ("sides", "bad"),
        ("sides", "4294967295"),
        ("StartU", "NaN"),
        ("RenderOrder", "1"),
        ("fademindist", "16"),
    ] {
        let mut bad = valid.clone();
        set_field(&mut bad, key, value);
        reject(&bad, &common::empty_game(), &path);
    }
    for case in 0..9 {
        let mut bad = valid.clone();
        match case {
            0 => bad.entities.last_mut().unwrap().extra.clear(),
            1 => {
                bad.entities.last_mut().unwrap().extra[0] = Node::string("overlaytransition", "bad")
            }
            2 => fields(&mut bad).push(Node::string("bAsIsU", "1 0 0")),
            3 => fields(&mut bad).push(Node::block("unknown", vec![])),
            4 => {
                bad.entities
                    .iter_mut()
                    .find(|entity| entity.classname() == Some("func_detail"))
                    .unwrap()
                    .hidden = true
            }
            5 => bad
                .entities
                .iter_mut()
                .find(|entity| entity.classname() == Some("func_detail"))
                .unwrap()
                .properties
                .set("classname", "func_brush"),
            6 => {
                for index in 0..33 {
                    fields(&mut bad).push(Node::string(format!("extra{index}"), "ignored"));
                }
            }
            7 => fields(&mut bad).push(Node::string("extra", "x".repeat(4097))),
            8 => {
                let solid = bad.world.solids.remove(0);
                bad.entities.last_mut().unwrap().solids.push(solid);
            }
            _ => unreachable!(),
        }
        reject(&bad, &common::empty_game(), &path);
    }
    let mut buried = Document::new(valid.clone());
    buried
        .edit("bury world receiver", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-152.0, 24.0, 16.0),
                DVec3::new(-56.0, 120.0, 64.0),
                "dev/dev_measuregeneric01",
            )?;
            Ok(())
        })
        .unwrap();
    reject(buried.vmf(), &common::empty_game(), &path);
    let mut hidden = valid;
    hidden.entities.last_mut().unwrap().hidden = true;
    assert_eq!(
        compiler::compile(&hidden, &common::empty_game())
            .bsp
            .unwrap()
            .to_bytes(),
        compiler::compile(&base, &common::empty_game())
            .bsp
            .unwrap()
            .to_bytes()
    );
}

#[test]
fn regions_retain_transition_blocks_with_outside_markers_and_reject_omitted_bindings() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsblack.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    let (source, faces) = fixture();
    let mut document = Document::new(transition(source, &faces.map(|face| face.side)));
    document
        .edit("region", |tx| {
            compile_region::apply(
                tx,
                Some(compile_region::Definition {
                    enabled: true,
                    bounds: compile_region::Bounds {
                        mins: DVec3::new(-160.0, -160.0, 0.0),
                        maxs: DVec3::new(160.0, 160.0, 256.0),
                    },
                }),
            )
        })
        .unwrap();
    let source = document.into_vmf();
    let (bsp, data) = compile(&source, &game);
    assert_eq!(data.overlays[0].face_count(), 3);
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, bsp.to_bytes()).unwrap();
    let mut missing = source;
    set_field(&mut missing, "sides", "4294967295");
    reject(&missing, &game, &path);
}

#[test]
fn authored_water_top_bindings_exclude_generated_underside_faces() {
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials/transition");
    std::fs::create_dir_all(&materials).unwrap();
    for (name, value) in [
        (
            "top",
            "Water { $abovewater 1 $bottommaterial transition/bottom }",
        ),
        ("bottom", "Water { $abovewater 0 }"),
        ("nodraw", "UnlitGeneric { %compilenodraw 1 }"),
    ] {
        std::fs::write(materials.join(format!("{name}.vmt")), value).unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    let mut document = Document::new(common::room());
    let side = document
        .edit("water receiver", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-128.0, -128.0, 0.0),
                DVec3::new(128.0, 128.0, 64.0),
                "transition/nodraw",
            )?
            else {
                unreachable!()
            };
            let side = tx
                .solid_mut(id)?
                .sides
                .iter_mut()
                .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap();
            side.material = "transition/top".into();
            Ok(side.id)
        })
        .unwrap();
    let mut source = transition(document.into_vmf(), &[side]);
    set_field(&mut source, "BasisOrigin", "0 0 64");
    let (bsp, data) = compile(&source, &game);
    let faces = bsp.records::<Face>().unwrap();
    let infos = bsp.records::<bsp::TexInfo>().unwrap();
    let textures = bsp.records::<bsp::TexData>().unwrap();
    let names = bsp.texdata_strings().unwrap();
    let material = |face: &Face| {
        names[textures[infos[face.texinfo as usize].texdata as usize].name_index as usize].as_str()
    };
    assert!(
        faces
            .iter()
            .any(|face| material(face).eq_ignore_ascii_case("transition/bottom"))
    );
    for &face in &data.overlays[0].faces[..data.overlays[0].face_count()] {
        assert!(material(&faces[face as usize]).eq_ignore_ascii_case("transition/top"));
    }
}
