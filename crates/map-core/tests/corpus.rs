//! Geometry checks against real Hammer maps. See source-formats' corpus test
//! for how the corpus is located.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::DVec3;
use map_core::{BrushGeometry, DisplacementMesh};
use source_formats::vmf::Vmf;

fn corpus_dirs() -> Vec<PathBuf> {
    if let Some(dirs) = std::env::var_os("VMF_CORPUS") {
        return std::env::split_paths(&dirs).collect();
    }
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let common = Path::new(&home).join(".local/share/Steam/steamapps/common");
    vec![common.join("sourcesdk_content"), common.join("SourceSDK")]
}

fn collect_vmfs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_vmfs(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("vmf"))
        {
            out.push(path);
        }
    }
}

fn corpus() -> Vec<(PathBuf, Vmf)> {
    let mut files = Vec::new();
    for dir in corpus_dirs() {
        collect_vmfs(&dir, &mut files);
    }
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).unwrap();
            let vmf = Vmf::parse(&text).unwrap();
            (path, vmf)
        })
        .collect()
}

#[test]
fn corpus_geometry() {
    let maps = corpus();
    if maps.is_empty() {
        eprintln!("no VMF corpus found; skipping");
        return;
    }

    let mut brushes = 0;
    let mut failures = Vec::new();
    let mut not_convex = Vec::new();
    let (mut shared, mut border) = (0, 0);

    for (path, vmf) in &maps {
        let name = path.file_name().unwrap().to_string_lossy();
        let mut disp_meshes = Vec::new();
        for solid in vmf.solids() {
            brushes += 1;
            let brush = match BrushGeometry::from_solid(solid) {
                Ok(brush) => brush,
                Err(e) => {
                    failures.push(format!("{name} solid {}: {e}", solid.id));
                    continue;
                }
            };
            let worst = brush
                .faces
                .iter()
                .flat_map(|f| brush.faces.iter().map(move |g| (f, g)))
                .flat_map(|(f, g)| g.vertices.iter().map(move |&v| f.plane.distance_to(v)))
                .fold(f64::MIN, f64::max);
            if worst > 0.01 {
                not_convex.push(format!(
                    "{name} solid {}: vertex {worst} in front",
                    solid.id
                ));
            }
            for face in &brush.faces {
                let Some(disp) = &solid.sides[face.side].displacement else {
                    continue;
                };
                match DisplacementMesh::build(face, disp) {
                    Ok(mesh) => disp_meshes.push(mesh),
                    Err(e) => failures.push(format!("{name} solid {}: {e}", solid.id)),
                }
            }
        }
        let (matched, total) = shared_border_vertices(&disp_meshes);
        shared += matched;
        border += total;
    }

    // Sewn neighbours share border vertices. With rows and columns swapped
    // this drops to about 20%, so it guards the grid orientation.
    let shared_ratio = shared as f64 / border as f64;
    eprintln!(
        "displacements: {shared}/{border} border vertices shared ({:.1}%)",
        100.0 * shared_ratio
    );
    eprintln!(
        "{brushes} brushes in {} maps, {} failed, {} not convex",
        maps.len(),
        failures.len(),
        not_convex.len()
    );
    for line in failures.iter().chain(&not_convex).take(30) {
        eprintln!("  {line}");
    }
    assert!(failures.is_empty() && not_convex.is_empty());
    assert!(
        shared_ratio > 0.7,
        "displacement grid orientation regressed"
    );
}

/// Counts border vertices that coincide with a border vertex of a different
/// displacement in the same map.
fn shared_border_vertices(meshes: &[DisplacementMesh]) -> (usize, usize) {
    let key = |p: DVec3| (p * 10.0).round().as_i64vec3().to_array();
    let mut owners: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    for (i, mesh) in meshes.iter().enumerate() {
        for v in mesh.border() {
            let entry = owners.entry(key(mesh.positions[v])).or_default();
            if entry.last() != Some(&i) {
                entry.push(i);
            }
        }
    }
    let mut matched = 0;
    let mut total = 0;
    for mesh in meshes {
        for v in mesh.border() {
            total += 1;
            if owners[&key(mesh.positions[v])].len() > 1 {
                matched += 1;
            }
        }
    }
    (matched, total)
}
