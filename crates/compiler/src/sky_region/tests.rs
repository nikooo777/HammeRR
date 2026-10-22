use super::*;
use map_core::{Document, Plane, edit};
use source_formats::gamefs::{Mount, MountedPath};
use source_formats::vmf::DispInfo;

fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("materials/dev")).unwrap();
    for (name, body) in [
        (
            "dev/dev_measuregeneric01",
            "LightmappedGeneric { $basetexture malformed }",
        ),
        ("water", "Water { $abovewater 1 }"),
        ("portal", "UnlitGeneric { %compileareaportal 1 }"),
        ("sky", "UnlitGeneric { %compilesky 1 }"),
        ("sky2d", "UnlitGeneric { %compilesky 1 %compile2dsky 1 }"),
    ] {
        std::fs::write(directory.path().join(format!("materials/{name}.vmt")), body).unwrap();
    }
    std::fs::write(
        directory.path().join("materials/malformed.vtf"),
        b"malformed VTF",
    )
    .unwrap();
    let mut game = crate::test_common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn add_camera(document: &mut Document, origin: DVec3) -> u32 {
    let ObjectId::Entity(id) = document
        .edit("camera", |tx| {
            edit::create_entity(
                tx,
                "sky_camera",
                Some(origin),
                &[("scale".into(), "16".into())],
                Vec::new(),
            )
        })
        .unwrap()
    else {
        unreachable!()
    };
    id
}

fn fixture() -> (Document, u32) {
    let mut document = Document::new(crate::test_common::room());
    let id = add_camera(&mut document, DVec3::new(0.0, 0.0, 128.0));
    (document, id)
}

fn add_room(document: &mut Document, offset: DVec3, ceiling: bool, left_wall: bool) {
    document
        .edit("other room", |tx| {
            for (index, (mins, maxs)) in [
                ([-272.0, -272.0, -16.0], [272.0, 272.0, 0.0]),
                ([-272.0, -272.0, 256.0], [272.0, 272.0, 272.0]),
                ([-272.0, -272.0, 0.0], [-256.0, 272.0, 256.0]),
                ([256.0, -272.0, 0.0], [272.0, 272.0, 256.0]),
                ([-256.0, -272.0, 0.0], [256.0, -256.0, 256.0]),
                ([-256.0, 256.0, 0.0], [256.0, 272.0, 256.0]),
            ]
            .into_iter()
            .enumerate()
            {
                if (index == 1 && !ceiling) || (index == 2 && !left_wall) {
                    continue;
                }
                edit::create_block(
                    tx,
                    DVec3::from_array(mins) + offset,
                    DVec3::from_array(maxs) + offset,
                    "dev/dev_measuregeneric01",
                )?;
            }
            edit::create_entity(
                tx,
                "info_player_start",
                Some(offset + DVec3::new(0.0, 0.0, 8.0)),
                &[],
                Vec::new(),
            )?;
            Ok(())
        })
        .unwrap();
}

fn add_brush(
    document: &mut Document,
    mins: DVec3,
    maxs: DVec3,
    classname: Option<&str>,
    material: &str,
) -> ObjectId {
    document
        .edit("scenery", |tx| {
            let object = edit::create_block(tx, mins, maxs, material)?;
            if let (ObjectId::Solid(id), Some(classname)) = (object, classname) {
                return edit::tie_to_entity(tx, &[id], classname, &[]);
            }
            Ok(object)
        })
        .unwrap()
}

fn resolve(vmf: &Vmf, game: &GameFs, camera: u32) -> Result<Region, String> {
    resolve_controlled(vmf, game, camera, |_| true, &Control::default())
}

#[test]
fn selected_camera_isolates_sealed_room_from_unrelated_leak_and_compile_only_errors() {
    let (_directory, game) = game();
    let (mut document, camera) = fixture();
    add_room(&mut document, DVec3::new(1024.0, 0.0, 0.0), false, true);
    let detail = add_brush(
        &mut document,
        DVec3::new(-64.0, -64.0, 16.0),
        DVec3::new(64.0, 64.0, 32.0),
        Some("func_detail"),
        "dev/dev_measuregeneric01",
    );
    document
        .edit("unrelated compile metadata", |tx| {
            edit::create_entity(
                tx,
                "env_cubemap",
                Some(DVec3::new(1024.0, 0.0, 64.0)),
                &[("sides".into(), "invalid side list".into())],
                Vec::new(),
            )
        })
        .unwrap();
    let before = document.vmf().clone();
    let output = resolve(&before, &game, camera).unwrap();
    assert_eq!(output.camera, camera);
    assert_eq!(output.origin, DVec3::new(0.0, 0.0, 128.0));
    assert_eq!(output.scale, 16);
    assert_eq!(
        output.stats.component_bounds,
        [[-256.0, -256.0, 0.0], [256.0, 256.0, 256.0]]
    );
    assert!(output.stats.component_leaves > 0);
    assert!(output.stats.tree_nodes > output.stats.component_leaves);
    assert!(output.stats.portals > 0);
    assert!(
        output
            .surfaces
            .iter()
            .flat_map(|surface| &surface.source_sides)
            .any(|source| source.owner == detail)
    );
    assert!(
        output
            .surfaces
            .iter()
            .flat_map(|surface| &surface.vertices)
            .all(|point| point.x.abs() <= 256.0
                && point.y.abs() <= 256.0
                && (0.0..=256.0).contains(&point.z))
    );
    assert!(
        output
            .surfaces
            .iter()
            .all(|surface| surface.source_sides.len() == 1)
    );
    assert_eq!(document.vmf(), &before);
    let normal = crate::compile(&before, &game);
    assert!(normal.report.has_errors());
}

#[test]
fn shared_wall_emits_only_miniature_facing_original_side() {
    let (_directory, game) = game();
    let (mut document, camera) = fixture();
    let wall = document.vmf().world.solids[3].clone();
    let inward = wall
        .sides
        .iter()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.x < -0.99)
        .unwrap()
        .id;
    add_room(&mut document, DVec3::new(528.0, 0.0, 0.0), true, false);
    let output = resolve(document.vmf(), &game, camera).unwrap();
    let sides = output
        .surfaces
        .iter()
        .filter(|surface| {
            surface
                .source_sides
                .iter()
                .any(|source| source.solid == wall.id)
        })
        .collect::<Vec<_>>();
    assert!(!sides.is_empty());
    assert!(sides.iter().all(|surface| surface.normal.x < -0.99
        && surface.vertices.iter().all(|point| point.x == 256.0)
        && surface.source_sides[0].side == inward));
}

#[test]
fn surfaces_preserve_sky_kind_and_native_texture_projection() {
    let (_directory, game) = game();
    let (document, camera) = fixture();
    for (material, two_dimensional) in [("sky", false), ("sky2d", true)] {
        let mut vmf = document.vmf().clone();
        let side = vmf.world.solids[1]
            .sides
            .iter_mut()
            .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z < -0.99)
            .unwrap();
        side.material = material.into();
        side.u_axis.axis = DVec3::new(0.5, 0.25, 0.0);
        side.u_axis.scale = 0.5;
        side.u_axis.offset = 8.0;
        let source = side.id;
        let output = resolve(&vmf, &game, camera).unwrap();
        let surface = output
            .surfaces
            .iter()
            .find(|surface| surface.source_sides[0].side == source)
            .unwrap();
        assert_ne!(surface.flags & source_formats::bsp::surf::SKY, 0);
        assert_eq!(
            surface.flags & source_formats::bsp::surf::SKY2D != 0,
            two_dimensional
        );
        assert_eq!(surface.texture_vectors[0], [1.0, 0.5, 0.0, 8.0]);
    }
}

#[test]
fn temporary_visibility_filters_source_fragments_without_opening_enclosure() {
    let (_directory, game) = game();
    let (mut document, camera) = fixture();
    let first = add_brush(
        &mut document,
        DVec3::new(-128.0, -64.0, 16.0),
        DVec3::new(0.0, 64.0, 32.0),
        Some("func_detail"),
        "dev/dev_measuregeneric01",
    );
    let second = add_brush(
        &mut document,
        DVec3::new(0.0, -64.0, 16.0),
        DVec3::new(128.0, 64.0, 32.0),
        Some("func_detail"),
        "dev/dev_measuregeneric01",
    );
    let wall = ObjectId::Solid(document.vmf().world.solids[3].id);
    let before = document.vmf().clone();
    let all = resolve(&before, &game, camera).unwrap();
    assert!(
        all.surfaces
            .iter()
            .any(|surface| surface.source_sides[0].owner == first)
    );
    assert!(
        all.surfaces
            .iter()
            .any(|surface| surface.source_sides[0].owner == second)
    );
    let filtered = resolve_controlled(
        &before,
        &game,
        camera,
        |owner| owner != first && owner != wall,
        &Control::default(),
    )
    .unwrap();
    assert_eq!(all.stats.component_bounds, filtered.stats.component_bounds);
    assert_eq!(all.stats.component_leaves, filtered.stats.component_leaves);
    assert!(filtered.surfaces.iter().all(|surface| {
        surface
            .source_sides
            .iter()
            .all(|source| source.owner != first && source.owner != wall)
    }));
    let top = filtered
        .surfaces
        .iter()
        .filter(|surface| surface.source_sides[0].owner == second && surface.normal.z > 0.99)
        .collect::<Vec<_>>();
    assert!(!top.is_empty());
    assert!(
        top.iter()
            .flat_map(|surface| &surface.vertices)
            .all(|point| point.x >= 0.0 && point.x <= 128.0 && point.z == 32.0)
    );
    let mut hidden = before.clone();
    hidden.world.solids[3].hidden = true;
    assert!(
        resolve(&hidden, &game, camera)
            .unwrap_err()
            .contains("leaks")
    );
    assert_eq!(document.vmf(), &before);
}

#[test]
fn selected_seed_rejects_solid_outside_boundary_and_leaking_space() {
    let (_directory, game) = game();
    let (document, camera) = fixture();
    for (origin, expected) in [
        ("260 0 128", "solid"),
        ("10000 0 128", "root volume"),
        ("256 0 128", "boundary"),
        ("0 0 0", "boundary"),
    ] {
        let mut vmf = document.vmf().clone();
        vmf.entities
            .iter_mut()
            .find(|entity| entity.id == camera)
            .unwrap()
            .properties
            .set("origin", origin);
        let error = resolve(&vmf, &game, camera).unwrap_err();
        assert!(error.contains(expected), "{origin}: {error}");
    }
    let mut document = document;
    add_room(&mut document, DVec3::new(1024.0, 0.0, 0.0), true, true);
    add_camera(&mut document, DVec3::new(1024.0, 0.0, 128.0));
    let mut vmf = document.vmf().clone();
    vmf.world.solids[1].hidden = true;
    assert!(resolve(&vmf, &game, camera).unwrap_err().contains("leaks"));
}

#[test]
fn camera_on_extended_open_partition_is_valid() {
    let (_directory, game) = game();
    let (mut document, camera) = fixture();
    add_brush(
        &mut document,
        DVec3::new(0.0, 96.0, 96.0),
        DVec3::new(16.0, 112.0, 112.0),
        None,
        "dev/dev_measuregeneric01",
    );
    let mut report = Report::default();
    let mut prepared = Map::prepare_world_geometry_controlled(
        document.vmf(),
        &game,
        &mut report,
        &Control::default(),
    )
    .unwrap();
    let indices = prepared.map.entities[0]
        .brushes
        .iter()
        .copied()
        .filter(|&index| prepared.map.brushes[index].structural())
        .collect::<Vec<_>>();
    let tree = Tree::build(&prepared.map.brushes, &indices, &mut prepared.map.planes).unwrap();
    let links = portals::build(&tree, &prepared.map.planes).unwrap();
    let portal = links
        .iter()
        .filter(|portal| {
            let center = portal.winding.center();
            portal
                .leaves
                .iter()
                .all(|&leaf| tree.nodes.get(leaf).is_some_and(|node| !node.solid()))
                && center.x.abs() < 255.0
                && center.y.abs() < 255.0
                && (1.0..255.0).contains(&center.z)
        })
        .max_by(|a, b| a.winding.area().total_cmp(&b.winding.area()))
        .expect("room has an open portal across an extended scenery partition");
    assert!(portal.winding.area() > 1.0);
    let origin = portal.winding.center();
    let leaf = tree.leaf_at(origin, &prepared.map.planes);
    assert!(
        tree.nodes[leaf].volume.sides.iter().any(|side| prepared
            .map
            .planes
            .get(side.plane)
            .distance(origin)
            .abs()
            <= ON_EPSILON)
    );
    let mut vmf = document.vmf().clone();
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == camera)
        .unwrap()
        .properties
        .set("origin", source_formats::vmf::format_vec3(origin));
    assert!(resolve(&vmf, &game, camera).is_ok());
}

#[test]
fn camera_requires_unique_point_identity_and_explicit_supported_integer_scale() {
    let (document, id) = fixture();
    for scale in [
        None,
        Some(""),
        Some("0"),
        Some("-1"),
        Some("1.5"),
        Some("NaN"),
        Some("129"),
        Some("+16"),
        Some(" 16"),
    ] {
        let mut vmf = document.vmf().clone();
        let entity = vmf
            .entities
            .iter_mut()
            .find(|entity| entity.id == id)
            .unwrap();
        entity.properties.remove("scale");
        if let Some(scale) = scale {
            entity.properties.set("scale", scale);
        }
        assert!(camera(&vmf, id).unwrap_err().contains("scale"));
    }
    for scale in ["1", "128"] {
        let mut vmf = document.vmf().clone();
        vmf.entities
            .iter_mut()
            .find(|entity| entity.id == id)
            .unwrap()
            .properties
            .set("scale", scale);
        assert_eq!(
            camera(&vmf, id).unwrap().scale,
            scale.parse::<u32>().unwrap()
        );
    }
    let mut vmf = document.vmf().clone();
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .properties
        .0
        .push(("SCALE".into(), "16".into()));
    assert!(camera(&vmf, id).unwrap_err().contains("duplicate key"));
    let mut vmf = document.vmf().clone();
    let mut duplicate = vmf
        .entities
        .iter()
        .find(|entity| entity.id == id)
        .unwrap()
        .clone();
    duplicate.properties.set("origin", "NaN 0 0");
    vmf.entities.push(duplicate);
    assert!(camera(&vmf, id).unwrap_err().contains("duplicate"));
    let mut vmf = document.vmf().clone();
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .properties
        .set("origin", "NaN 0 0");
    assert!(camera(&vmf, id).unwrap_err().contains("finite"));
    assert!(
        camera(document.vmf(), u32::MAX)
            .unwrap_err()
            .contains("missing")
    );
    let mut vmf = document.vmf().clone();
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .hidden = true;
    assert!(camera(&vmf, id).unwrap_err().contains("visible root point"));
    let mut vmf = document.vmf().clone();
    vmf.world.solids[1].sides[0].id = vmf.world.solids[0].sides[0].id;
    assert!(camera(&vmf, id).unwrap_err().contains("duplicate side"));
    let mut entity = document
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.id == id)
        .unwrap()
        .clone();
    entity
        .properties
        .0
        .resize(MAX_CAMERA_PROPERTIES + 1, ("empty".into(), String::new()));
    assert!(
        parse_camera(&entity)
            .unwrap_err()
            .contains("property count budget")
    );
    let mut entity = document
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.id == id)
        .unwrap()
        .clone();
    entity
        .properties
        .set("message", "x".repeat(MAX_CAMERA_BYTES));
    assert!(
        parse_camera(&entity)
            .unwrap_err()
            .contains("property byte budget")
    );
}

#[test]
fn known_omitted_geometry_must_be_provably_disjoint_even_with_outside_origin() {
    let (_directory, game) = game();
    for classname in ["func_brush", "func_areaportal", "func_areaportalwindow"] {
        let (mut document, camera) = fixture();
        let owner = add_brush(
            &mut document,
            DVec3::new(240.0, -16.0, 96.0),
            DVec3::new(300.0, 16.0, 160.0),
            Some(classname),
            "dev/dev_measuregeneric01",
        );
        let mut vmf = document.vmf().clone();
        let ObjectId::Entity(id) = owner else {
            unreachable!()
        };
        vmf.entities
            .iter_mut()
            .find(|entity| entity.id == id)
            .unwrap()
            .properties
            .set("origin", "1024 0 128");
        let error = resolve(&vmf, &game, camera).unwrap_err();
        assert!(error.contains("intersects"), "{classname}: {error}");
        let (mut document, camera) = fixture();
        add_brush(
            &mut document,
            DVec3::new(500.0, -16.0, 96.0),
            DVec3::new(600.0, 16.0, 160.0),
            Some(classname),
            "dev/dev_measuregeneric01",
        );
        assert!(
            resolve(document.vmf(), &game, camera).is_ok(),
            "{classname}"
        );
    }
    for (mins, maxs, rejected) in [
        (
            DVec3::new(-32.0, -32.0, 0.0),
            DVec3::new(32.0, 32.0, 32.0),
            true,
        ),
        (
            DVec3::new(500.0, -32.0, 0.0),
            DVec3::new(600.0, 32.0, 32.0),
            false,
        ),
    ] {
        let (mut document, camera) = fixture();
        add_brush(&mut document, mins, maxs, None, "water");
        let output = resolve(document.vmf(), &game, camera);
        if rejected {
            assert!(output.unwrap_err().contains("water"));
        } else {
            assert!(output.is_ok());
        }
    }
}

#[test]
fn material_generated_details_have_unknown_extents_even_with_disjoint_parent_brushes() {
    let (directory, game) = game();
    for detail in [None, Some(""), Some(" "), Some("grass")] {
        let declaration = detail
            .map(|value| format!(" %detailtype \"{value}\""))
            .unwrap_or_default();
        std::fs::write(
            directory.path().join("materials/generated.vmt"),
            format!("LightmappedGeneric {{ $basetexture malformed{declaration} }}"),
        )
        .unwrap();
        for (mins, maxs) in [
            (DVec3::new(-32.0, -32.0, 0.0), DVec3::new(32.0, 32.0, 32.0)),
            (DVec3::new(500.0, -32.0, 0.0), DVec3::new(600.0, 32.0, 32.0)),
        ] {
            let (mut document, camera) = fixture();
            add_brush(&mut document, mins, maxs, None, "generated");
            let result = resolve(document.vmf(), &game, camera);
            if detail.is_some_and(|value| !value.trim().is_empty()) {
                let error = result.unwrap_err();
                assert!(
                    error.contains("material-generated detail geometry"),
                    "{error}"
                );
                assert!(error.contains("bounds are unavailable"), "{error}");
            } else {
                assert!(result.is_ok(), "{detail:?} at {mins:?}: {result:?}");
            }
        }
    }
}

#[test]
fn unknown_model_instance_and_displacement_bounds_reject_instead_of_using_origins() {
    let (_directory, game) = game();
    for classname in ["prop_static", "func_instance"] {
        let (mut document, camera) = fixture();
        document
            .edit("unsupported", |tx| {
                edit::create_entity(
                    tx,
                    classname,
                    Some(DVec3::new(10000.0, 0.0, 0.0)),
                    &[("model".into(), "models/unknown.mdl".into())],
                    Vec::new(),
                )
            })
            .unwrap();
        assert!(
            resolve(document.vmf(), &game, camera)
                .unwrap_err()
                .contains("bounds are unavailable")
        );
    }
    let (mut document, camera) = fixture();
    let object = add_brush(
        &mut document,
        DVec3::new(500.0, -32.0, 0.0),
        DVec3::new(600.0, 32.0, 32.0),
        None,
        "dev/dev_measuregeneric01",
    );
    let ObjectId::Solid(id) = object else {
        unreachable!()
    };
    let mut vmf = document.vmf().clone();
    vmf.world
        .solids
        .iter_mut()
        .find(|solid| solid.id == id)
        .unwrap()
        .sides[0]
        .displacement = Some(DispInfo {
        power: 2,
        start_position: DVec3::new(500.0, -32.0, 32.0),
        flags: None,
        elevation: 0.0,
        subdivided: false,
        normals: None,
        distances: None,
        offsets: None,
        offset_normals: None,
        alphas: None,
        triangle_tags: None,
        allowed_verts: None,
        extra_properties: Default::default(),
        extra: Vec::new(),
    });
    assert!(
        resolve(&vmf, &game, camera)
            .unwrap_err()
            .contains("displacement")
    );
}

#[test]
fn cancellation_and_every_preview_stage_budget_return_errors() {
    let (_directory, game) = game();
    let (document, camera) = fixture();
    let control = Control::default();
    control.cancel().unwrap();
    assert!(
        resolve_controlled(document.vmf(), &game, camera, |_| true, &control)
            .unwrap_err()
            .contains("cancelled")
    );
    let baseline = resolve(document.vmf(), &game, camera).unwrap();
    for (limits, expected) in [
        (
            Limits {
                source_brushes: 0,
                ..Limits::default()
            },
            "source brush",
        ),
        (
            Limits {
                source_work: 0,
                ..Limits::default()
            },
            "source conversion work",
        ),
        (
            Limits {
                cpu_bytes: 0,
                ..Limits::default()
            },
            "aggregate CPU",
        ),
        (
            Limits {
                vertices: baseline.stats.vertices - 1,
                ..Limits::default()
            },
            "output vertex",
        ),
        (
            Limits {
                tree: tree::Limits {
                    nodes: 1,
                    ..Limits::default().tree
                },
                ..Limits::default()
            },
            "node allocation",
        ),
        (
            Limits {
                tree: tree::Limits {
                    pending: 0,
                    ..Limits::default().tree
                },
                ..Limits::default()
            },
            "pending fragment",
        ),
        (
            Limits {
                tree: tree::Limits {
                    points: 0,
                    ..Limits::default().tree
                },
                ..Limits::default()
            },
            "geometry allocation",
        ),
        (
            Limits {
                tree: tree::Limits {
                    work: 0,
                    ..Limits::default().tree
                },
                ..Limits::default()
            },
            "traversal work",
        ),
        (
            Limits {
                portals: portals::Limits {
                    portals: 0,
                    ..Limits::default().portals
                },
                ..Limits::default()
            },
            "output portal",
        ),
        (
            Limits {
                faces: faces::Limits {
                    faces: baseline.stats.surfaces - 1,
                    ..Limits::default().faces
                },
                ..Limits::default()
            },
            "output fragment",
        ),
        (
            Limits {
                faces: faces::Limits {
                    pending: 0,
                    ..Limits::default().faces
                },
                ..Limits::default()
            },
            "pending fragment",
        ),
        (
            Limits {
                faces: faces::Limits {
                    points: 0,
                    ..Limits::default().faces
                },
                ..Limits::default()
            },
            "winding allocation",
        ),
        (
            Limits {
                faces: faces::Limits {
                    work: 0,
                    ..Limits::default().faces
                },
                ..Limits::default()
            },
            "traversal work",
        ),
    ] {
        let error = resolve_limited(
            document.vmf(),
            &game,
            camera,
            |_| true,
            &Control::default(),
            limits,
        )
        .unwrap_err();
        assert!(error.contains(expected), "{expected}: {error}");
    }
    let exact = Limits {
        vertices: baseline.stats.vertices,
        tree: tree::Limits {
            nodes: baseline.stats.tree_nodes,
            ..Limits::default().tree
        },
        faces: faces::Limits {
            faces: baseline.stats.surfaces,
            ..Limits::default().faces
        },
        ..Limits::default()
    };
    let exact_output = resolve_limited(
        document.vmf(),
        &game,
        camera,
        |_| true,
        &Control::default(),
        exact,
    )
    .unwrap();
    assert_eq!(exact_output.stats.vertices, baseline.stats.vertices);
    assert_eq!(
        exact_output.stats.component_bounds,
        baseline.stats.component_bounds
    );
}
