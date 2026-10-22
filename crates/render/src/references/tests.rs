use super::*;
use crate::{MaterialLibrary, SceneMesh, SurfaceView, ViewOptions};
use glam::{DVec3, Vec3};
use map_core::{Document, ObjectId};
use std::collections::{BTreeSet, HashSet};

fn vertex(position: [f32; 3]) -> Vertex {
    Vertex {
        position,
        normal: [0.0, 0.0, 1.0],
        uv: [0.0; 2],
        color: [255; 4],
        lightmap: [1.0; 3],
        smoothing: 1,
        opacity: 0.25,
        texture_alpha: 1,
        tangent: [1.0; 4],
        lighting_receiver: 1,
    }
}

fn layer() -> Mesh {
    let positions = [
        [-24.0, -24.0, 0.0],
        [-24.0, 24.0, 0.0],
        [24.0, 24.0, 0.0],
        [24.0, -24.0, 0.0],
    ];
    Mesh {
        vertices: positions.map(vertex).into(),
        indices: vec![0, 1, 2, 0, 2, 3],
        edges: [0, 1, 1, 2, 2, 3, 3, 0]
            .map(|index| LineVertex {
                position: positions[index],
                color: [255; 4],
            })
            .into(),
        brushes: vec![Brush {
            owner: 100,
            vertices: 0..4,
            indices: 0..6,
            edges: 0..8,
        }],
    }
}

fn scene(document: &Document, materials: &mut MaterialLibrary) -> SceneMesh {
    SceneMesh::build(
        document.vmf(),
        &source_formats::gamefs::GameFs {
            game_dir: Default::default(),
            base_dir: Default::default(),
            mounts: Vec::new(),
        },
        materials,
        &HashSet::new(),
    )
}

#[test]
fn reference_append_filters_owners_preserves_root_and_reuses_edges_for_outlines() {
    let mut document = Document::empty();
    document
        .edit("root", |tx| {
            map_core::edit::create_block(tx, DVec3::splat(64.0), DVec3::splat(96.0), "root")
        })
        .unwrap();
    let mut materials =
        MaterialLibrary::flat(["root".into(), "__editor/instance-reference".into()]);
    let mut mesh = scene(&document, &mut materials);
    let root_vertices = mesh.vertices.clone();
    let root_indices = mesh.indices.clone();
    let root_batches = mesh.batches.clone();
    let root_bounds = (mesh.mins, mesh.maxs);
    let references = layer();
    mesh.append_references(
        &references,
        &mut materials,
        &BTreeSet::new(),
        &HashSet::new(),
    )
    .unwrap();
    assert_eq!(mesh.vertices, root_vertices);
    mesh.append_references(
        &references,
        &mut materials,
        &BTreeSet::from([100]),
        &HashSet::from([ObjectId::Entity(100)]),
    )
    .unwrap();
    assert_eq!(&mesh.vertices[..root_vertices.len()], root_vertices);
    assert_eq!(&mesh.indices[..root_indices.len()], root_indices);
    assert_eq!(&mesh.batches[..root_batches.len()], root_batches);
    assert_eq!((mesh.mins, mesh.maxs), root_bounds);
    assert_eq!(mesh.reference_lines.len(), 8);
    assert_eq!(mesh.reference_selection_ranges, vec![0..8]);
    for (source, drawn) in references
        .vertices
        .iter()
        .zip(&mesh.vertices[root_vertices.len()..])
    {
        assert_eq!(source.position, drawn.position);
        assert_eq!(source.normal, drawn.normal);
        assert_eq!(drawn.lighting_receiver, 0);
        assert_eq!(drawn.lightmap, [0.0; 3]);
        assert_eq!(drawn.tangent, [0.0; 4]);
        assert_eq!(drawn.opacity, 1.0);
        assert_eq!(drawn.texture_alpha, 0);
    }
    let material = materials.get(mesh.batches.last().unwrap().material);
    assert!(material.reference && !material.tool && !material.sky && !material.translucent);
    assert!(material.base.is_none() && material.normal.is_none());
    let fs = source_formats::gamefs::GameFs {
        game_dir: Default::default(),
        base_dir: Default::default(),
        mounts: Vec::new(),
    };
    materials.reload(&fs);
    assert!(
        materials
            .get(mesh.batches.last().unwrap().material)
            .reference
    );
    assert!(!materials.get(mesh.batches.last().unwrap().material).missing);
}

#[test]
fn reference_append_rejects_invalid_ranges_indices_and_limits_atomically() {
    for case in ["range", "index", "vertices", "indices", "lines"] {
        let mut references = layer();
        match case {
            "range" => references.brushes[0].vertices.end = 5,
            "index" => references.indices[0] = 4,
            "vertices" => {
                references
                    .vertices
                    .resize(MAX_VERTICES + 1, vertex([0.0; 3]));
                references.brushes[0].vertices.end = references.vertices.len() as u32;
            }
            "indices" => {
                references.indices.resize(MAX_INDICES + 3, 0);
                references.brushes[0].indices.end = references.indices.len() as u32;
            }
            "lines" => {
                references.edges.resize(
                    MAX_LINES + 2,
                    LineVertex {
                        position: [0.0; 3],
                        color: [255; 4],
                    },
                );
                references.brushes[0].edges.end = references.edges.len() as u32;
            }
            _ => unreachable!(),
        }
        let mut materials = MaterialLibrary::default();
        let mut mesh = scene(&Document::empty(), &mut materials);
        let vertices = mesh.vertices.clone();
        let indices = mesh.indices.clone();
        let batches = mesh.batches.clone();
        assert!(
            mesh.append_references(
                &references,
                &mut materials,
                &BTreeSet::from([100]),
                &HashSet::new()
            )
            .is_err(),
            "{case}"
        );
        assert_eq!(mesh.vertices, vertices);
        assert_eq!(mesh.indices, indices);
        assert_eq!(mesh.batches, batches);
        assert!(mesh.reference_lines.is_empty());
        assert!(mesh.reference_selection_ranges.is_empty());
    }
}

#[test]
#[ignore = "requires offscreen GPU; root runs explicitly with --ignored"]
fn reference_gpu_preserves_disabled_pixels_wireframe_holes_and_ordinary_depth() {
    let context = crate::GpuContext::new().unwrap();
    let mut renderer = crate::Renderer::new(&context);
    let mut materials = MaterialLibrary::default();
    let document = Document::empty();
    let mut mesh = scene(&document, &mut materials);
    let camera = crate::Camera::planar(crate::Projection::Top, Vec3::ZERO, 65.0);
    let off = ViewOptions {
        grid_visible: false,
        selection_outlines: false,
        ..Default::default()
    };
    let uploaded = renderer.upload(&context, &mesh, &materials);
    let baseline = renderer.render_to_rgba(&context, &uploaded, &camera, &off, 65, 65);
    mesh.append_references(
        &layer(),
        &mut materials,
        &BTreeSet::from([100]),
        &HashSet::new(),
    )
    .unwrap();
    let uploaded = renderer.upload(&context, &mesh, &materials);
    assert_eq!(
        renderer.render_to_rgba(&context, &uploaded, &camera, &off, 65, 65),
        baseline
    );
    let on = ViewOptions {
        instance_contents: true,
        ..off
    };
    let filled = renderer.render_to_rgba(&context, &uploaded, &camera, &on, 65, 65);
    let center = (32 * 65 + 32) * 4;
    assert_ne!(filled[center..center + 4], baseline[center..center + 4]);
    let wire = renderer.render_to_rgba(
        &context,
        &uploaded,
        &camera,
        &ViewOptions {
            surface: SurfaceView::Wireframe,
            ..on
        },
        65,
        65,
    );
    assert_eq!(wire[center..center + 4], baseline[center..center + 4]);
    assert_ne!(wire, baseline);
    for surface in [
        SurfaceView::Flat,
        SurfaceView::LightmapDensity,
        SurfaceView::SmoothingGroups,
    ] {
        assert_eq!(
            renderer.render_to_rgba(
                &context,
                &uploaded,
                &camera,
                &ViewOptions { surface, ..on },
                65,
                65
            ),
            filled
        );
    }
    let mut root = Document::empty();
    root.edit("occluder", |tx| {
        map_core::edit::create_block(
            tx,
            DVec3::new(-8.0, -8.0, 8.0),
            DVec3::new(8.0, 8.0, 16.0),
            "root",
        )
    })
    .unwrap();
    let mut materials = MaterialLibrary::flat(["root".into()]);
    let mut mesh = scene(&root, &mut materials);
    let root_only = renderer.upload(&context, &mesh, &materials);
    let root_pixels = renderer.render_to_rgba(&context, &root_only, &camera, &off, 65, 65);
    mesh.append_references(
        &layer(),
        &mut materials,
        &BTreeSet::from([100]),
        &HashSet::new(),
    )
    .unwrap();
    let uploaded = renderer.upload(&context, &mesh, &materials);
    let both = renderer.render_to_rgba(&context, &uploaded, &camera, &on, 65, 65);
    assert_eq!(both[center..center + 4], root_pixels[center..center + 4]);
    let outside = (32 * 65 + 48) * 4;
    assert_ne!(
        both[outside..outside + 4],
        root_pixels[outside..outside + 4]
    );
}
