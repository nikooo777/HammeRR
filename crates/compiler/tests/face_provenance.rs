mod common;

use std::sync::Arc;

use compiler::{Report, faces, map::Map, portals, tree::Tree};
use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::vmf::Vmf;

fn world_faces(vmf: &Vmf) -> Vec<faces::Face> {
    let game = common::empty_game();
    let mut report = Report::default();
    let mut map = Map::load(vmf, &game, &mut report);
    let mut tree = Tree::build(&map.brushes, &map.entities[0].brushes, &mut map.planes).unwrap();
    let links = portals::build(&tree, &map.planes);
    assert!(portals::flood(&mut tree, &map.planes, &links, &map.entities, &mut report).is_none());
    tree.prune();
    assert!(!report.has_errors(), "{report:?}");
    faces::build(&map, &tree, 0)
}

#[test]
fn merged_subdivided_faces_retain_source_sides_and_hidden_sources_disappear() {
    let mut document = Document::new(common::room());
    let mut blocks = Vec::new();
    let mut tops = Vec::new();
    for x in [-128.0, 0.0] {
        document
            .edit("detail plate", |tx| {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(x, -64.0, 16.0),
                    DVec3::new(x + 128.0, 64.0, 32.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let brush = tx.solid_mut(id)?;
                for side in &mut brush.sides {
                    side.lightmap_scale = 1;
                    if map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99 {
                        tops.push(side.id);
                    }
                }
                edit::tie_to_entity(tx, &[id], "func_detail", &[])?;
                blocks.push(id);
                Ok(())
            })
            .unwrap();
    }
    tops.sort_unstable();
    let vmf = document.vmf().clone();
    let output = world_faces(&vmf);
    let shared = output
        .iter()
        .filter(|face| &*face.source_sides == tops.as_slice())
        .collect::<Vec<_>>();
    assert!(
        shared.len() >= 8,
        "merged/subdivided source scopes: {shared:?}"
    );
    assert!(
        shared
            .iter()
            .all(|face| Arc::ptr_eq(&shared[0].source_sides, &face.source_sides))
    );
    assert!(
        output
            .iter()
            .all(|face| face.source_sides.windows(2).all(|ids| ids[0] < ids[1]))
    );
    let mut hidden = vmf.clone();
    let brush = hidden
        .entities
        .iter_mut()
        .flat_map(|entity| &mut entity.solids)
        .find(|brush| brush.id == blocks[0])
        .unwrap();
    let hidden_sides = brush.sides.iter().map(|side| side.id).collect::<Vec<_>>();
    brush.hidden = true;
    let output = world_faces(&hidden);
    assert!(output.iter().all(|face| {
        face.source_sides
            .iter()
            .all(|id| !hidden_sides.contains(id))
    }));
    assert_eq!(document.vmf(), &vmf);
}
