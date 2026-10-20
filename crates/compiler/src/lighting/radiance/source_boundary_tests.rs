use super::super::{HullSideSource, props::PropTriangle};
use super::*;
use map_core::Plane;

const SOURCE_PLANE: usize = 5;
const SOURCE_SIDE: u32 = 42;
const SOURCE_OWNER: usize = 7;
const COLOR: DVec3 = DVec3::new(0.5, 0.25, 0.125);

fn box_hull(mins: DVec3, maxs: DVec3) -> Occluder {
    let mut planes = Vec::new();
    for axis in 0..3 {
        let mut normal = DVec3::ZERO;
        normal[axis] = 1.0;
        planes.push(Plane {
            normal,
            dist: maxs[axis],
        });
        planes.push(Plane {
            normal: -normal,
            dist: -mins[axis],
        });
    }
    Occluder::new(planes, mins, maxs).unwrap()
}

fn ceiling_hull(z: f64, owner: usize, side: u32) -> Occluder {
    let sources = (0..6)
        .map(|index| HullSideSource {
            plane: if index == 5 {
                SOURCE_PLANE
            } else {
                100 + index
            },
            side: (index == 5).then_some(side),
        })
        .collect();
    box_hull(
        DVec3::new(-128.0, -128.0, z),
        DVec3::new(128.0, 128.0, z + 8.0),
    )
    .with_source(owner, sources)
    .unwrap()
}

fn boundary(z: f64, owners: Vec<usize>, sides: Vec<u32>) -> SourceBoundary {
    SourceBoundary {
        plane: SOURCE_PLANE,
        equation: Plane {
            normal: DVec3::NEG_Z,
            dist: -z,
        },
        sides: sides.into(),
        owners,
    }
}

fn surface(
    z: f64,
    boundary: Option<SourceBoundary>,
    sky: bool,
    budget: &mut Budget<'_>,
) -> SurfaceIndex {
    surface_points(
        [
            DVec3::new(-128.0, -128.0, z),
            DVec3::new(128.0, -128.0, z),
            DVec3::new(0.0, 128.0, z),
        ],
        boundary,
        sky,
        budget,
    )
}

fn surface_points(
    points: [DVec3; 3],
    boundary: Option<SourceBoundary>,
    sky: bool,
    budget: &mut Budget<'_>,
) -> SurfaceIndex {
    let allocation = budget
        .reserve_bytes(
            ShadowIndex::<Surface>::allocation_bytes(1).unwrap()
                + size_of::<Radiance>()
                + 2 * size_of::<usize>(),
        )
        .unwrap();
    let mut geometry = Vec::new();
    add_surface(&mut geometry, points, 0, None, boundary.as_ref(), budget).unwrap();
    SurfaceIndex {
        geometry: ShadowIndex::new(geometry, &mut |work| budget.consume(work)).unwrap(),
        radiance: vec![Radiance {
            average: COLOR,
            reflectivity: DVec3::ONE,
            samples: Vec::new(),
            dimensions: [0; 2],
            projection: [[0.0; 4]; 2],
            mins: [0; 2],
            sky,
            extras: Vec::new(),
            boundary,
        }],
        _allocation: allocation,
    }
}

fn trace(
    index: &SurfaceIndex,
    start: DVec3,
    direction: DVec3,
    hulls: Vec<Occluder>,
    budget: &mut Budget<'_>,
) -> Option<DVec3> {
    let shadows = ShadowIndex::new(hulls, &mut |work| budget.consume(work)).unwrap();
    let props =
        ShadowIndex::<PropTriangle>::new(Vec::new(), &mut |work| budget.consume(work)).unwrap();
    index
        .trace(start, direction, 0.0, &shadows, &props, budget)
        .unwrap()
        .map(|hit| hit.color)
}

#[test]
fn nodraw_corner_interval_before_a_distinct_source_is_opaque_in_both_orders() {
    let control = crate::Control::default();
    let mut budget = Budget::new(&control);
    let z = 72.015;
    let native = crate::emit::stored_vertex(DVec3::Z * z).z;
    let source = surface(
        native,
        Some(boundary(z, vec![SOURCE_OWNER], vec![SOURCE_SIDE])),
        false,
        &mut budget,
    );
    let start = DVec3::new(-71.985, 0.0, 0.0);
    let direction = DVec3::new(1.0, 0.0, 1.0).normalize();
    let blocker = box_hull(DVec3::new(0.0, -4.0, 64.0), DVec3::new(8.0, 4.0, 72.0));
    let end = start + direction * super::super::sky::TRACE_LENGTH;
    let entry = blocker.entry(start, end).unwrap().unwrap().0 * super::super::sky::TRACE_LENGTH;
    let source_distance = native / direction.z;
    assert!((source_distance - entry - 0.042425543703743074).abs() < 1e-10);
    assert!(source_distance - entry < 0.05);
    assert_eq!(
        trace(
            &source,
            start,
            direction,
            vec![ceiling_hull(z, SOURCE_OWNER, SOURCE_SIDE)],
            &mut budget
        ),
        Some(COLOR)
    );
    for reverse in [false, true] {
        let mut hulls = vec![ceiling_hull(z, SOURCE_OWNER, SOURCE_SIDE), blocker.clone()];
        if reverse {
            hulls.reverse();
        }
        assert_eq!(trace(&source, start, direction, hulls, &mut budget), None);
    }
}

#[test]
fn odd_outward_source_plane_accepts_only_its_own_measured_native_boundary() {
    let control = crate::Control::default();
    let mut budget = Budget::new(&control);
    let z = 72.01537;
    let native = crate::emit::stored_vertex(DVec3::Z * z).z;
    assert!(native > z);
    let source = surface(
        native,
        Some(boundary(z, vec![SOURCE_OWNER], vec![SOURCE_SIDE])),
        false,
        &mut budget,
    );
    let owned = ceiling_hull(z, SOURCE_OWNER, SOURCE_SIDE);
    assert!(owned.intersects(DVec3::ZERO, DVec3::Z * native).unwrap());
    assert_eq!(
        trace(
            &source,
            DVec3::ZERO,
            DVec3::Z,
            vec![owned.clone()],
            &mut budget
        ),
        Some(COLOR)
    );
    let distinct = ceiling_hull(z, SOURCE_OWNER + 1, SOURCE_SIDE + 1);
    for hulls in [vec![owned.clone(), distinct.clone()], vec![distinct, owned]] {
        assert_eq!(
            trace(&source, DVec3::ZERO, DVec3::Z, hulls, &mut budget),
            None
        );
    }
    let same_owner_other_side = ceiling_hull(z - 0.01, SOURCE_OWNER, SOURCE_SIDE + 1);
    assert_eq!(
        trace(
            &source,
            DVec3::ZERO,
            DVec3::Z,
            vec![same_owner_other_side],
            &mut budget
        ),
        None
    );
}

#[test]
fn source_owner_without_matching_plane_or_transform_does_not_exempt_an_interval() {
    let control = crate::Control::default();
    let mut budget = Budget::new(&control);
    let z = 72.01537;
    let native = crate::emit::stored_vertex(DVec3::Z * z).z;
    let source = surface(
        native,
        Some(boundary(z, vec![SOURCE_OWNER], vec![SOURCE_SIDE])),
        false,
        &mut budget,
    );
    let mut wrong_plane = ceiling_hull(z, SOURCE_OWNER, SOURCE_SIDE);
    wrong_plane.source.as_mut().unwrap().sides[5].plane ^= 1;
    assert_eq!(
        trace(
            &source,
            DVec3::ZERO,
            DVec3::Z,
            vec![wrong_plane],
            &mut budget
        ),
        None
    );
    assert_eq!(
        trace(
            &source,
            DVec3::ZERO,
            DVec3::Z,
            vec![ceiling_hull(z - 0.01, SOURCE_OWNER, SOURCE_SIDE)],
            &mut budget
        ),
        None
    );
    let translated_z = z + 100.0;
    let translated_native = crate::emit::stored_vertex(DVec3::Z * translated_z).z;
    let translated = surface(
        translated_native,
        Some(boundary(
            translated_z,
            vec![SOURCE_OWNER],
            vec![SOURCE_SIDE],
        )),
        false,
        &mut budget,
    );
    assert_eq!(
        trace(
            &translated,
            DVec3::ZERO,
            DVec3::Z,
            vec![ceiling_hull(translated_z, SOURCE_OWNER, SOURCE_SIDE)],
            &mut budget
        ),
        Some(COLOR)
    );
}

#[test]
fn angled_large_coordinate_source_preserves_its_measured_support_on_grazing_rays() {
    let control = crate::Control::default();
    let mut budget = Budget::new(&control);
    let z = 72.01537;
    let rotation = glam::DQuat::from_rotation_y(std::f64::consts::FRAC_PI_4);
    let translation = DVec3::new(16000.0, -8000.0, 0.0);
    let mut source_boundary = boundary(z, vec![SOURCE_OWNER], vec![SOURCE_SIDE]);
    source_boundary.equation.normal = rotation * source_boundary.equation.normal;
    source_boundary.equation.dist += source_boundary.equation.normal.dot(translation);
    let points = [
        DVec3::new(-128.0, -128.0, z + 0.004),
        DVec3::new(128.0, -128.0, z + 0.004),
        DVec3::new(0.0, 128.0, z + 0.004),
    ]
    .map(|point| crate::emit::stored_vertex(rotation * point + translation));
    let index = surface_points(points, Some(source_boundary), false, &mut budget);
    let mut hull = ceiling_hull(z, SOURCE_OWNER, SOURCE_SIDE);
    let mut mins = DVec3::splat(f64::INFINITY);
    let mut maxs = DVec3::splat(f64::NEG_INFINITY);
    for x in [hull.mins.x, hull.maxs.x] {
        for y in [hull.mins.y, hull.maxs.y] {
            for z in [hull.mins.z, hull.maxs.z] {
                let point = rotation * DVec3::new(x, y, z) + translation;
                mins = mins.min(point);
                maxs = maxs.max(point);
            }
        }
    }
    for plane in &mut hull.planes {
        plane.normal = rotation * plane.normal;
        plane.dist += plane.normal.dot(translation);
    }
    hull.mins = mins;
    hull.maxs = maxs;
    let local_direction = DVec3::new(1.0, 0.0, 0.001).normalize();
    let start = rotation * (DVec3::Z * (z + 0.004) - local_direction * 1000.0) + translation;
    let direction = rotation * local_direction;
    let hit = index
        .geometry
        .nearest(
            start,
            start + direction * super::super::sky::TRACE_LENGTH,
            &mut |work| budget.consume(work),
            &mut |surface| {
                Ok(surface
                    .triangle
                    .fraction(start, start + direction * super::super::sky::TRACE_LENGTH)?
                    .map(|fraction| (fraction, ())))
            },
        )
        .unwrap()
        .unwrap()
        .0;
    let entry = hull
        .entry(start, start + direction * super::super::sky::TRACE_LENGTH)
        .unwrap()
        .unwrap()
        .0;
    assert!((hit - entry) * super::super::sky::TRACE_LENGTH > 0.05);
    assert_eq!(
        trace(&index, start, direction, vec![hull], &mut budget),
        Some(COLOR)
    );
}

#[test]
fn merged_contributors_and_tied_owned_planes_do_not_hide_a_third_blocker() {
    let control = crate::Control::default();
    let mut budget = Budget::new(&control);
    let z = 72.01537;
    let native = crate::emit::stored_vertex(DVec3::Z * z).z;
    let source = surface(
        native,
        Some(boundary(z, vec![7, 8], vec![42, 43])),
        false,
        &mut budget,
    );
    let mut left = ceiling_hull(z, 7, 42);
    left.planes[0].dist = 0.0;
    left.maxs.x = 0.0;
    let mut right = ceiling_hull(z, 8, 43);
    right.planes[1].dist = 0.0;
    right.mins.x = 0.0;
    assert_eq!(
        trace(
            &source,
            DVec3::ZERO,
            DVec3::Z,
            vec![left.clone(), right.clone()],
            &mut budget
        ),
        Some(COLOR)
    );
    for hulls in [
        vec![left.clone(), right.clone(), ceiling_hull(z, 9, 44)],
        vec![ceiling_hull(z, 9, 44), right.clone(), left.clone()],
    ] {
        assert_eq!(
            trace(&source, DVec3::ZERO, DVec3::Z, hulls, &mut budget),
            None
        );
    }
    let start = DVec3::new(-1.0, 0.0, z - 1.0);
    let direction = DVec3::new(1.0, 0.0, 1.0).normalize();
    let mut edge = ceiling_hull(z, 7, 42);
    edge.planes[1].dist = 0.0;
    edge.mins.x = 0.0;
    assert_eq!(
        trace(&source, start, direction, vec![edge], &mut budget),
        Some(COLOR)
    );
}

#[test]
fn terrain_source_has_no_brush_base_plane_exception_and_inside_origins_stay_opaque() {
    let control = crate::Control::default();
    let mut budget = Budget::new(&control);
    let z = 72.02;
    let terrain = surface(z, None, false, &mut budget);
    assert_eq!(
        trace(
            &terrain,
            DVec3::ZERO,
            DVec3::Z,
            vec![ceiling_hull(z - 0.015, SOURCE_OWNER, SOURCE_SIDE)],
            &mut budget
        ),
        None
    );
    let authored = z - 0.005;
    let source = surface(
        z,
        Some(boundary(authored, vec![SOURCE_OWNER], vec![SOURCE_SIDE])),
        false,
        &mut budget,
    );
    for origin in [authored, authored + 0.001] {
        assert_eq!(
            trace(
                &source,
                DVec3::Z * origin,
                DVec3::Z,
                vec![ceiling_hull(authored, SOURCE_OWNER, SOURCE_SIDE)],
                &mut budget
            ),
            None
        );
    }
}

#[test]
fn sky_keeps_opaque_entry_ties_while_accepting_its_rounded_source() {
    let control = crate::Control::default();
    let mut budget = Budget::new(&control);
    let z = 72.01537;
    let native = crate::emit::stored_vertex(DVec3::Z * z).z;
    let source = surface(
        native,
        Some(boundary(z, vec![SOURCE_OWNER], vec![SOURCE_SIDE])),
        true,
        &mut budget,
    );
    let mut sky = vec![false; 6];
    sky[5] = true;
    let owned = ceiling_hull(z, SOURCE_OWNER, SOURCE_SIDE)
        .with_sky(sky)
        .unwrap();
    assert_eq!(
        trace(
            &source,
            DVec3::ZERO,
            DVec3::Z,
            vec![owned.clone()],
            &mut budget
        ),
        Some(COLOR)
    );
    let opaque = ceiling_hull(z, SOURCE_OWNER + 1, SOURCE_SIDE + 1);
    for hulls in [vec![owned.clone(), opaque.clone()], vec![opaque, owned]] {
        assert_eq!(
            trace(&source, DVec3::ZERO, DVec3::Z, hulls, &mut budget),
            None
        );
    }
}

#[test]
fn source_certificate_queries_preserve_cancellation_and_shared_work_failures() {
    let control = crate::Control::default();
    let mut budget = Budget::new(&control);
    let index = surface(64.0, None, false, &mut budget);
    let shadows = ShadowIndex::new(
        vec![ceiling_hull(63.99, SOURCE_OWNER, SOURCE_SIDE)],
        &mut |work| budget.consume(work),
    )
    .unwrap();
    let props =
        ShadowIndex::<PropTriangle>::new(Vec::new(), &mut |work| budget.consume(work)).unwrap();
    budget.consume(100_000_000 - budget.work()).unwrap();
    assert!(
        index
            .trace(DVec3::ZERO, DVec3::Z, 0.0, &shadows, &props, &mut budget)
            .is_err()
    );
    let mut budget = Budget::new(&control);
    control.cancel().unwrap();
    assert!(
        index
            .trace(DVec3::ZERO, DVec3::Z, 0.0, &shadows, &props, &mut budget)
            .is_err()
    );
}

#[test]
fn actual_emission_excludes_nodraw_faces_but_the_shared_hull_builder_keeps_the_blocker() {
    use map_core::{Document, edit};
    use source_formats::gamefs::{Mount, MountedPath};
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    std::fs::write(
        directory.path().join("materials/hidden.vmt"),
        "LightmappedGeneric { %compilenodraw 1 }",
    )
    .unwrap();
    let mut game = crate::test_common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    let mut document = Document::new(crate::test_common::room());
    document
        .edit("radiance blocker fixture", |tx| {
            edit::create_block(
                tx,
                DVec3::new(0.0, -4.0, 64.0),
                DVec3::new(8.0, 4.0, 72.0),
                "hidden",
            )?;
            edit::create_block(
                tx,
                DVec3::new(-128.0, -128.0, 72.015),
                DVec3::new(128.0, 128.0, 80.0),
                "dev/dev_measuregeneric01",
            )?;
            Ok(())
        })
        .unwrap();
    let output = crate::compile_with_options(document.vmf(), &game, crate::Options::default());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let mut bsp = output.bsp.unwrap();
    let origins = output.face_origins.unwrap();
    let mut faces = bsp.lighting_faces(LightingKind::Ldr).unwrap();
    let face_sources = (0..faces.len())
        .map(|index| {
            let mut sides = origins
                .contributors(index)
                .unwrap()
                .map(|origin| match origin {
                    crate::face_origins::Origin::Root { side, .. } => *side,
                    _ => panic!("fixture has no instances or generated faces"),
                })
                .collect::<Vec<_>>();
            sides.sort_unstable();
            Arc::<[u32]>::from(sides)
        })
        .collect::<Vec<_>>();
    let planes = bsp.records::<source_formats::bsp::Plane>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    assert!(
        faces
            .iter()
            .all(|face| infos[face.texinfo as usize].flags & surf::NODRAW == 0)
    );
    let mut colors = Vec::new();
    for face in &mut faces {
        let plane = planes[face.plane as usize];
        if plane.normal[2] < -0.99 && (f64::from(plane.dist) + 72.015).abs() < 0.001 {
            face.styles = [0, 255, 255, 255];
            face.lightmap_size = [0, 0];
            face.light_offset = ((colors.len() + 1) * size_of::<RgbExp32>()) as i32;
            colors.extend(
                [RgbExp32 {
                    rgb: [128, 64, 32],
                    exponent: 0,
                }; 2],
            );
        }
    }
    assert!(!colors.is_empty());
    bsp.set_lighting(LightingKind::Ldr, &colors).unwrap();
    bsp.set_lighting_faces(LightingKind::Ldr, &faces).unwrap();
    let mut report = crate::Report::default();
    let map = Map::load(document.vmf(), &game, &mut report).unwrap();
    let blocker = map
        .brushes
        .iter()
        .position(|brush| brush.sides.iter().all(|side| !side.visible))
        .unwrap();
    let control = crate::Control::default();
    let mut budget = Budget::new(&control);
    let index = build_styled(&map, &bsp, LightingKind::Ldr, &face_sources, &mut budget).unwrap();
    assert!(index.radiance.iter().all(|source| {
        source
            .boundary
            .as_ref()
            .is_none_or(|boundary| !boundary.owners.contains(&blocker))
    }));
    let hulls = super::super::bake::occluders(&map, 0, &mut report, &mut budget).unwrap();
    assert!(hulls.iter().any(|hull| {
        hull.source
            .as_ref()
            .is_some_and(|source| source.brush == blocker)
    }));
    let clear = hulls
        .iter()
        .filter(|hull| hull.source.as_ref().unwrap().brush != blocker)
        .cloned()
        .collect();
    let start = DVec3::new(-71.985, 0.0, 0.0);
    let direction = DVec3::new(1.0, 0.0, 1.0).normalize();
    let color = trace(&index, start, direction, clear, &mut budget).unwrap();
    assert!((color - DVec3::new(64.0, 32.0, 16.0) / 255.0).length() < 1e-10);
    assert_eq!(trace(&index, start, direction, hulls, &mut budget), None);
    let error = build_styled(
        &map,
        &bsp,
        LightingKind::Ldr,
        &face_sources[..face_sources.len() - 1],
        &mut budget,
    )
    .err()
    .unwrap();
    assert!(error.contains("emitted face count"));
}
