//! Geometry checks against real Hammer maps. See source-formats' corpus test
//! for how the corpus is located.

mod common;

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

#[test]
fn corpus_map_checks_find_no_invalid_closed_geometry() {
    let mut failures = Vec::new();
    let mut counts = std::collections::BTreeMap::new();
    for (path, vmf) in corpus() {
        let report = map_core::checks::check(&vmf);
        for issue in report.issues {
            *counts.entry(issue.code).or_insert(0) += 1;
            if matches!(
                issue.code,
                "open_brush" | "invalid_brush" | "invalid_displacement"
            ) {
                failures.push(format!(
                    "{} {}: {}",
                    path.display(),
                    issue.location.path,
                    issue.message
                ));
            }
        }
    }
    eprintln!("map-check issue counts: {counts:?}");
    assert!(
        failures.is_empty(),
        "{}",
        failures.into_iter().take(30).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn corpus_component_translation_preserves_geometry_and_corner_textures() {
    use map_core::{Document, subobject, texel_uv};

    let delta = DVec3::new(0.125, -0.25, 0.375);
    let mut moved = 0;
    let mut skipped = std::collections::BTreeMap::new();
    let mut failures = Vec::new();
    for (path, vmf) in corpus() {
        let sources = vmf.solids().cloned().collect::<Vec<_>>();
        let mut document = Document::new(vmf);
        for source in sources {
            if source.sides.iter().any(|side| side.displacement.is_some()) {
                *skipped.entry("displacements").or_insert(0) += 1;
                continue;
            }
            if subobject::solid(&document, source.id).is_err() {
                *skipped.entry("ambiguous identity").or_insert(0) += 1;
                continue;
            }
            let before = match subobject::topology(&source) {
                Ok(shape) => shape,
                Err(_) => {
                    *skipped.entry("unsupported topology").or_insert(0) += 1;
                    continue;
                }
            };
            if before
                .vertices
                .iter()
                .any(|vertex| (vertex.position + delta).abs().max_element() > 65_536.0)
            {
                *skipped.entry("coordinate bounds").or_insert(0) += 1;
                continue;
            }
            if source.sides.iter().any(|side| {
                let plane = map_core::Plane::from_points(side.plane.0).unwrap();
                side.u_axis
                    .axis
                    .cross(side.v_axis.axis)
                    .dot(plane.normal)
                    .abs()
                    < 1e-10
            }) {
                *skipped.entry("degenerate texture projection").or_insert(0) += 1;
                continue;
            }
            let components = before
                .faces
                .iter()
                .map(|face| subobject::Component {
                    solid: source.id,
                    sides: vec![face.side],
                })
                .collect::<Vec<_>>();
            let plan =
                match subobject::plan_move_with_texture_lock(&document, &components, delta, true) {
                    Ok(plan) => plan,
                    Err(error) => {
                        failures.push(format!("{} brush {}: {error}", path.display(), source.id));
                        continue;
                    }
                };
            document
                .edit("attached translation", |tx| subobject::apply(tx, &plan))
                .unwrap();
            let (_, changed) = subobject::solid(&document, source.id).unwrap();
            let after = subobject::topology(changed).unwrap();
            for vertex in &before.vertices {
                let point = after.vertices[after.resolve(&vertex.sides).unwrap()[0]].position;
                assert!(
                    (vertex.position + delta).abs_diff_eq(point, 1e-5),
                    "{} brush {}",
                    path.display(),
                    source.id
                );
                for side_id in &vertex.sides {
                    let old = source
                        .sides
                        .iter()
                        .find(|side| side.id == *side_id)
                        .unwrap();
                    let new = changed
                        .sides
                        .iter()
                        .find(|side| side.id == *side_id)
                        .unwrap();
                    assert!(
                        texel_uv(old, vertex.position).abs_diff_eq(texel_uv(new, point), 0.0001),
                        "{} brush {} face {side_id}",
                        path.display(),
                        source.id
                    );
                }
            }
            for old in &source.sides {
                let new = changed.sides.iter().find(|side| side.id == old.id).unwrap();
                let mut expected = old.clone();
                expected.plane = new.plane;
                expected.u_axis = new.u_axis;
                expected.v_axis = new.v_axis;
                assert_eq!(*new, expected);
            }
            document.undo().unwrap();
            assert_eq!(subobject::solid(&document, source.id).unwrap().1, &source);
            moved += 1;
        }
    }
    eprintln!(
        "attached component translations: {moved}; skipped: {skipped:?}; rejected: {}",
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{}",
        failures.into_iter().take(30).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn corpus_repeated_component_edit_save_cycles_preserve_precise_geometry() {
    repeated_corpus_edits(false);
}

#[test]
fn corpus_repeated_affine_edit_save_cycles_preserve_precise_geometry() {
    repeated_corpus_edits(true);
}

fn repeated_corpus_edits(affine: bool) {
    use map_core::{Document, ObjectId, subobject};

    let mut sampled = 0;
    let mut qualified = 0;
    let mut skipped = std::collections::BTreeMap::new();
    let mut worst_geometry = 0f64;
    let mut worst_texture = 0f64;
    let maps = corpus();
    for (path, vmf) in &maps {
        let document = Document::new(vmf.clone());
        let mut world = Vec::new();
        let mut entities = Vec::new();
        for source in vmf.solids() {
            let skip = if source.sides.iter().any(|side| side.displacement.is_some()) {
                Some("displacements")
            } else if subobject::solid(&document, source.id).is_err() {
                Some("ambiguous identity")
            } else if let Ok(shape) = subobject::topology(source) {
                if shape
                    .vertices
                    .iter()
                    .any(|vertex| vertex.position.abs().max_element() > 65_535.0)
                {
                    Some("coordinate bounds")
                } else if source.sides.iter().any(|side| {
                    side.u_axis
                        .axis
                        .cross(side.v_axis.axis)
                        .dot(map_core::Plane::from_points(side.plane.0).unwrap().normal)
                        .abs()
                        < 1e-10
                }) {
                    Some("degenerate texture projection")
                } else {
                    None
                }
            } else {
                Some("unsupported topology")
            };
            if let Some(reason) = skip {
                *skipped.entry(reason).or_insert(0) += 1;
                continue;
            }
            qualified += 1;
            if matches!(
                document.owner_of_solid(source.id),
                Some(ObjectId::Entity(_))
            ) {
                entities.push(source);
            } else {
                world.push(source);
            }
        }
        for candidates in [&world, &entities] {
            if candidates.is_empty() {
                continue;
            }
            let indices = [
                0,
                candidates.len() / 3,
                2 * candidates.len() / 3,
                candidates.len() - 1,
            ]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
            for index in indices {
                let source = candidates[index];
                let mut isolated = Document::empty().vmf().clone();
                isolated.world.id = vmf.world.id;
                match document.owner_of_solid(source.id).unwrap() {
                    ObjectId::Solid(_) => isolated.world.solids.push(source.clone()),
                    ObjectId::Entity(owner) => {
                        let mut entity = document.entity(owner).unwrap().clone();
                        entity.solids = vec![source.clone()];
                        isolated.entities.push(entity);
                    }
                }
                let isolated = Document::try_new(isolated).unwrap();
                let (geometry, texture) = if affine {
                    common::repeated_affine(isolated, source.id, 8)
                } else {
                    common::repeated_translation(isolated, source.id, 8)
                };
                assert!(
                    geometry < 1e-5 && texture < 0.0001,
                    "{} brush {}",
                    path.display(),
                    source.id
                );
                worst_geometry = worst_geometry.max(geometry);
                worst_texture = worst_texture.max(texture);
                sampled += 1;
            }
        }
    }
    eprintln!(
        "repeated edit/save, affine {affine}: {sampled} sampled brushes from {qualified} qualified in {} maps; skipped {skipped:?}; worst drift {worst_geometry} units / {worst_texture} texels",
        maps.len()
    );
    if !maps.is_empty() {
        assert!(sampled > 0);
    }
}
