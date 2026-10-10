//! Reads every compiled map of a local Counter-Strike: Source install
//! (`CSS_DIR`, default the Steam library) and checks the lumps decode, the
//! cross-references are in range, and writing reproduces the same map.
//! Skipped when the game isn't installed.

use std::path::PathBuf;

use source_formats::bsp::{
    self, Brush, BrushSide, Bsp, Edge, Face, Leaf, Model, Node, Plane, TexData, TexInfo,
};

fn maps_dir() -> Option<PathBuf> {
    let dir = match std::env::var_os("CSS_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?)
            .join(".local/share/Steam/steamapps/common/Counter-Strike Source"),
    };
    let maps = dir.join("cstrike/maps");
    maps.is_dir().then_some(maps)
}

fn check(bsp: &Bsp) -> Result<(), String> {
    let e = |e: std::io::Error| e.to_string();
    let planes = bsp.records::<Plane>().map_err(e)?;
    let texdata = bsp.records::<TexData>().map_err(e)?;
    let texinfo = bsp.records::<TexInfo>().map_err(e)?;
    let faces = bsp.records::<Face>().map_err(e)?;
    let nodes = bsp.records::<Node>().map_err(e)?;
    let leafs = bsp.records::<Leaf>().map_err(e)?;
    let edges = bsp.records::<Edge>().map_err(e)?;
    let models = bsp.records::<Model>().map_err(e)?;
    let brushes = bsp.records::<Brush>().map_err(e)?;
    let sides = bsp.records::<BrushSide>().map_err(e)?;
    let vertices = bsp.vertices().map_err(e)?;
    let surfedges = bsp.surfedges().map_err(e)?;
    let leaf_faces = bsp.leaf_faces().map_err(e)?;
    let leaf_brushes = bsp.leaf_brushes().map_err(e)?;
    let names = bsp.texdata_strings().map_err(e)?;
    bsp.entities().map_err(e)?;

    let ensure = |ok: bool, what: &str| if ok { Ok(()) } else { Err(what.to_owned()) };
    for face in &faces {
        ensure((face.plane as usize) < planes.len(), "face plane")?;
        ensure(
            face.texinfo < 0 || (face.texinfo as usize) < texinfo.len(),
            "face texinfo",
        )?;
        let range = face.first_edge as usize..face.first_edge as usize + face.edge_count as usize;
        ensure(range.end <= surfedges.len(), "face surfedges")?;
    }
    for &surfedge in &surfedges {
        ensure((surfedge.unsigned_abs() as usize) < edges.len(), "surfedge")?;
    }
    for edge in &edges {
        ensure(
            edge.0.iter().all(|&v| (v as usize) < vertices.len()),
            "edge vertex",
        )?;
    }
    for info in &texinfo {
        ensure(
            info.texdata < 0 || (info.texdata as usize) < texdata.len(),
            "texinfo texdata",
        )?;
    }
    for data in &texdata {
        ensure((data.name_index as usize) < names.len(), "texdata name")?;
    }
    for node in &nodes {
        ensure((node.plane as usize) < planes.len(), "node plane")?;
        for &child in &node.children {
            let ok = if child >= 0 {
                (child as usize) < nodes.len()
            } else {
                ((-1 - child) as usize) < leafs.len()
            };
            ensure(ok, "node child")?;
        }
    }
    for leaf in &leafs {
        let faces_end = leaf.first_leaf_face as usize + leaf.leaf_face_count as usize;
        let brushes_end = leaf.first_leaf_brush as usize + leaf.leaf_brush_count as usize;
        ensure(faces_end <= leaf_faces.len(), "leaf faces")?;
        ensure(brushes_end <= leaf_brushes.len(), "leaf brushes")?;
    }
    for brush in &brushes {
        ensure(
            (brush.first_side + brush.side_count) as usize <= sides.len(),
            "brush sides",
        )?;
    }
    for model in &models {
        ensure((model.head_node as usize) < nodes.len(), "model head node")?;
        ensure(
            (model.first_face + model.face_count) as usize <= faces.len(),
            "model faces",
        )?;
    }
    ensure(!models.is_empty(), "no world model")
}

#[test]
fn stock_maps_decode_and_round_trip() {
    let Some(dir) = maps_dir() else {
        eprintln!("CS:S not installed; skipping");
        return;
    };
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "bsp"))
        .filter(|p| p.metadata().is_ok_and(|m| m.len() > 0))
        .collect();
    paths.sort();
    assert!(!paths.is_empty());

    let mut failures = Vec::new();
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let bsp = match Bsp::read(path) {
            Ok(bsp) => bsp,
            Err(error) => {
                failures.push(format!("{name}: {error}"));
                continue;
            }
        };
        if let Err(error) = check(&bsp) {
            failures.push(format!("{name}: {error}"));
        }
        let compressed: Vec<usize> = (0..bsp::LUMP_COUNT)
            .filter(|&i| bsp.is_compressed(i))
            .collect();
        if !compressed.is_empty() {
            eprintln!("{name}: compressed lumps {compressed:?}");
        }
        match Bsp::parse(&bsp.to_bytes()) {
            Ok(again) if again == bsp => {}
            Ok(_) => failures.push(format!("{name}: rewrite differs")),
            Err(error) => failures.push(format!("{name}: rewrite unreadable: {error}")),
        }
        let faces = bsp.records::<Face>().map(|f| f.len()).unwrap_or(0);
        let props = bsp.game_lump(*b"sprp").map(|g| g.version);
        eprintln!(
            "{name}: v{} leafs v{} faces {faces} static props v{props:?}",
            bsp.version,
            bsp.lump(bsp::lump::LEAFS).version
        );
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
