mod common;
#[path = "common/overlays.rs"]
mod overlay_fixture;

use overlay_fixture::add_overlay;

use compiler::{Report, faces, map::Map, portals, tree::Tree};
use glam::DVec3;
use map_core::{BrushGeometry, Document, ObjectId, edit};
use source_formats::bsp::{self, Bsp, Edge, Face, OverlayData, TexInfo};
use source_formats::vmf::{DispInfo, Vmf};

fn plates() -> (Vmf, [u32; 2]) {
    let mut document = Document::new(common::room());
    let mut tops = Vec::new();
    document
        .edit("plates", |tx| {
            for x in [-128.0, 0.0] {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(x, -64.0, 16.0),
                    DVec3::new(x + 128.0, 64.0, 32.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                for side in &mut tx.solid_mut(id)?.sides {
                    side.lightmap_scale = 1;
                    if map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99 {
                        tops.push(side.id);
                    }
                }
                edit::tie_to_entity(tx, &[id], "func_detail", &[])?;
            }
            Ok(())
        })
        .unwrap();
    (document.vmf().clone(), tops.try_into().unwrap())
}

fn compile(vmf: &Vmf) -> (Bsp, OverlayData) {
    let before = vmf.clone();
    let output = compiler::compile(vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(vmf, &before);
    let bsp = Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap();
    let data = bsp.overlay_data().unwrap().unwrap();
    assert_eq!(output.stats.overlays, data.overlays.len());
    assert_eq!(
        output.stats.overlay_face_refs,
        data.overlays
            .iter()
            .map(|overlay| overlay.face_count())
            .sum::<usize>()
    );
    (bsp, data)
}

fn face_points(bsp: &Bsp, face: Face) -> Vec<[f32; 3]> {
    let vertices = bsp.values::<[f32; 3]>(bsp::lump::VERTEXES).unwrap();
    let edges = bsp.records::<Edge>().unwrap();
    let surfedges = bsp.values::<i32>(bsp::lump::SURFEDGES).unwrap();
    surfedges[face.first_edge as usize..face.first_edge as usize + face.edge_count as usize]
        .iter()
        .map(|&edge| {
            vertices[usize::from(edges[edge.unsigned_abs() as usize].0[usize::from(edge < 0)])]
        })
        .collect()
}

#[test]
fn source_membership_prevents_spill_and_final_indices_follow_native_emission() {
    let (vmf, tops) = plates();
    for sides in [&tops[..1], &tops[..]] {
        let vmf = add_overlay(vmf.clone(), sides);
        let (bsp, data) = compile(&vmf);
        let record = data.overlays[0];
        assert!(record.face_count() >= 8);
        let faces = bsp.records::<Face>().unwrap();
        for &index in &record.faces[..record.face_count()] {
            let points = face_points(&bsp, faces[index as usize]);
            assert!(points.iter().all(|point| point[2] == 32.0
                && point[0] >= -128.0
                && point[0] <= if sides.len() == 1 { 0.0 } else { 128.0 }));
        }
        let game = common::empty_game();
        let mut report = Report::default();
        let mut map = Map::load(&vmf, &game, &mut report).unwrap();
        let mut tree =
            Tree::build(&map.brushes, &map.entities[0].brushes, &mut map.planes).unwrap();
        let links = portals::build(&tree, &map.planes).unwrap();
        assert!(
            portals::flood(&mut tree, &map.planes, &links, &map.entities, &mut report).is_none()
        );
        tree.prune();
        let planned = faces::build(&map, &tree, 0).unwrap();
        let selected = planned
            .iter()
            .filter(|face| face.source_sides.iter().any(|side| sides.contains(side)))
            .collect::<Vec<_>>();
        assert_eq!(selected.len(), record.face_count());
        if sides.len() == 2 {
            assert!(selected.iter().all(|face| face.source_sides.len() == 2));
        } else {
            assert!(selected.iter().all(|face| &*face.source_sides == sides));
        }
    }
}

#[test]
fn packed_basis_defaults_fades_named_accessor_and_hidden_helpers_preserve_source() {
    let (base, tops) = plates();
    let mut vmf = add_overlay(base.clone(), &tops);
    let (bsp, data) = compile(&vmf);
    let record = data.overlays[0];
    assert_eq!(record.u, [0.0, 1.0]);
    assert_eq!(record.v, [0.0, 1.0]);
    assert_eq!(record.uv_points.map(|point| point[2]), [1.0, 0.0, 0.0, 0.0]);
    assert_eq!(data.fades[0].min_distance_squared, -1.0);
    assert_eq!(data.fades[0].max_distance_squared, 0.0);
    let infos = bsp.records::<TexInfo>().unwrap();
    assert_eq!(
        infos[record.texinfo as usize].texture_vecs,
        [[0.0, 0.0, 0.0, -99999.0]; 2]
    );
    assert!(
        bsp.texdata_strings()
            .unwrap()
            .iter()
            .any(|name| name == "DECALS/DECALGRAFFITI001A")
    );
    let entities = bsp.entities().unwrap();
    assert!(entities.contains("\"classname\" \"info_overlay_accessor\""));
    assert!(entities.contains("\"OverlayID\" \"0\""));
    assert!(entities.contains("\"origin\" \"0 0 32\""));
    assert!(!entities.contains("\"classname\" \"info_overlay\""));
    let helper = vmf.entities.last_mut().unwrap();
    helper.properties.set("BasisV", "0 -1 0");
    helper.properties.set("RenderOrder", "3");
    helper.properties.set("StartU", "1");
    helper.properties.set("EndU", "0");
    helper.properties.set("fademindist", "32");
    helper.properties.set("fademaxdist", "64");
    let (_, data) = compile(&vmf);
    assert_eq!(data.overlays[0].uv_points[3][2], 1.0);
    assert_eq!(data.overlays[0].render_order(), 3);
    assert_eq!(data.overlays[0].u, [1.0, 0.0]);
    assert_eq!(data.fades[0].min_distance_squared, 1024.0);
    assert_eq!(data.fades[0].max_distance_squared, 4096.0);
    vmf.entities.last_mut().unwrap().hidden = true;
    assert_eq!(
        compiler::compile(&vmf, &common::empty_game())
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
fn displacement_sources_bind_the_native_terrain_face() {
    for power in [2, 3, 4] {
        let mut document = Document::new(common::room());
        let side = document
            .edit("terrain", |tx| {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(-64.0, -64.0, 0.0),
                    DVec3::new(64.0, 64.0, 16.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let solid = tx.solid_mut(id)?;
                let geometry = BrushGeometry::from_solid(solid).unwrap();
                let face = geometry
                    .faces
                    .iter()
                    .find(|face| face.plane.normal.z > 0.99)
                    .unwrap();
                let size = (1 << power) + 1;
                solid.sides[face.side].displacement = Some(DispInfo {
                    power,
                    start_position: face.vertices[0],
                    flags: Some(0),
                    elevation: 0.0,
                    subdivided: false,
                    normals: Some(vec![vec![DVec3::Z; size]; size]),
                    distances: Some(vec![vec![4.0; size]; size]),
                    offsets: None,
                    offset_normals: None,
                    alphas: None,
                    triangle_tags: None,
                    allowed_verts: None,
                    extra_properties: Default::default(),
                    extra: Vec::new(),
                });
                Ok(solid.sides[face.side].id)
            })
            .unwrap();
        let vmf = add_overlay(document.vmf().clone(), &[side]);
        let (bsp, data) = compile(&vmf);
        let terrain = bsp.displacements().unwrap().unwrap();
        assert_eq!(data.overlays[0].face_count(), 1);
        assert_eq!(data.overlays[0].faces[0], i32::from(terrain.infos[0].face));
    }
}

#[test]
fn invalid_overlay_definitions_and_face_budgets_preserve_previous_output() {
    let (base, tops) = plates();
    let valid = add_overlay(base, &tops);
    let previous = compile(&valid).0.to_bytes();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, &previous).unwrap();
    for (key, value) in [
        ("material", "../secret"),
        ("material", "/absolute"),
        ("material", ""),
        ("BasisOrigin", "nan 0 0"),
        ("BasisOrigin", "32768 0 0"),
        ("BasisU", "0 0 0"),
        ("BasisV", "1 0 0"),
        ("BasisNormal", "0 0 2"),
        ("uv1", "-120 -60 0"),
        ("uv0", "1e40 0 0"),
        ("uv2", "0 -100 0"),
        ("StartU", "NaN"),
        ("EndU", "0"),
        ("RenderOrder", "4"),
        ("RenderOrder", "1.5"),
        ("fademindist", "1e30"),
        ("fademaxdist", "inf"),
        ("sides", ""),
        ("sides", "4294967295"),
        ("sides", "oops"),
    ] {
        let mut vmf = valid.clone();
        vmf.entities.last_mut().unwrap().properties.set(key, value);
        let before = vmf.clone();
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(
            output.bsp.is_none() && output.report.has_errors(),
            "{key}={value}: {:?}",
            output.report
        );
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), previous);
        assert_eq!(vmf, before);
    }
    let mut hidden = valid.clone();
    hidden
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("func_detail"))
        .unwrap()
        .hidden = true;
    assert!(
        compiler::compile(&hidden, &common::empty_game())
            .bsp
            .is_none()
    );
    let mut model = valid.clone();
    model
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("func_detail"))
        .unwrap()
        .properties
        .set("classname", "func_brush");
    assert!(
        compiler::compile(&model, &common::empty_game())
            .bsp
            .is_none()
    );
    let mut room = common::room();
    let floor = &mut room.world.solids[0];
    let top = floor
        .sides
        .iter_mut()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap();
    top.lightmap_scale = 1;
    let side = top.id;
    let output = compiler::compile(&add_overlay(room, &[side]), &common::empty_game());
    assert!(output.bsp.is_none() && output.report.has_errors());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("compiled-face budget"))
    );
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(path).unwrap(), previous);
}
