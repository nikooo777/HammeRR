//! Prints a BSP's lump sizes and checks face winding against face planes.

use source_formats::bsp::{self, Bsp, Edge, Face, Leaf, Plane};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: bsp_info <file.bsp>")?;
    let bsp = Bsp::read(&path)?;
    println!("version {} revision {}", bsp.version, bsp.map_revision);
    for (index, lump) in bsp.lumps.iter().enumerate() {
        if !lump.data.is_empty() {
            println!(
                "  lump {index:2} v{} {:>9} bytes",
                lump.version,
                lump.data.len()
            );
        }
    }
    for game in &bsp.game_lumps {
        let id = String::from_utf8_lossy(&game.id);
        println!(
            "  game lump {id} v{} {} bytes",
            game.version,
            game.data.len()
        );
    }

    let planes = bsp.records::<Plane>()?;
    let faces = bsp.records::<Face>()?;
    let edges = bsp.records::<Edge>()?;
    let vertices = bsp.vertices()?;
    let surfedges = bsp.surfedges()?;
    let (mut clockwise, mut counter) = (0, 0);
    for face in &faces {
        let points: Vec<glam::Vec3> = (0..face.edge_count as usize)
            .map(|i| {
                let surfedge = surfedges[face.first_edge as usize + i];
                let edge = edges[surfedge.unsigned_abs() as usize].0;
                let v = if surfedge >= 0 { edge[0] } else { edge[1] };
                glam::Vec3::from_array(vertices[v as usize])
            })
            .collect();
        let mut normal = glam::Vec3::ZERO;
        for i in 1..points.len() - 1 {
            normal += (points[i] - points[0]).cross(points[i + 1] - points[0]);
        }
        let plane = glam::Vec3::from_array(planes[face.plane as usize].normal);
        if normal.dot(plane) < 0.0 {
            clockwise += 1;
        } else {
            counter += 1;
        }
    }
    println!(
        "faces {}: clockwise from front {clockwise}, counter-clockwise {counter}",
        faces.len()
    );
    let odd_face_planes = faces.iter().filter(|f| f.plane % 2 == 1).count();
    let node_faces = faces.iter().filter(|f| f.on_node != 0).count();
    let side_mismatch = faces
        .iter()
        .filter(|f| f.side as u16 != f.plane % 2)
        .count();
    println!(
        "faces on odd planes {odd_face_planes}, side != plane parity {side_mismatch}, on nodes {node_faces}"
    );
    let leafs = bsp.records::<Leaf>()?;
    if let Some(first) = leafs.first() {
        println!(
            "leaf 0: contents {:#x} cluster {} area {}",
            first.contents,
            first.cluster,
            first.area()
        );
    }
    let flags = leafs
        .iter()
        .fold(std::collections::BTreeMap::new(), |mut m, l| {
            *m.entry((l.flags(), l.contents == 0)).or_insert(0) += 1;
            m
        });
    println!("leaf (flags, empty) counts {flags:?}");
    println!(
        "lighting bytes {}",
        bsp.lump(bsp::lump::LIGHTING).data.len()
    );
    let entities = bsp.entities()?;
    println!(
        "entity lump head:\n{}",
        &entities[..entities.len().min(600)]
    );
    Ok(())
}
