use super::super::props::PropTriangle;
use super::*;
use crate::Control;

fn floor(budget: &mut Budget<'_>) -> SurfaceIndex {
    let points = [
        DVec3::new(-2048.0, -2048.0, 0.0),
        DVec3::new(2048.0, -2048.0, 0.0),
        DVec3::new(2048.0, 2048.0, 0.0),
        DVec3::new(-2048.0, 2048.0, 0.0),
    ];
    let surfaces = [[0, 1, 2], [0, 2, 3]].map(|indices| Surface {
        triangle: Triangle::new(indices.map(|index| points[index])).unwrap(),
        normal: DVec3::Z,
        source: 0,
        coordinates: None,
        boundary_error: 0.0,
    });
    SurfaceIndex {
        geometry: ShadowIndex::new(surfaces.into(), &mut |work| budget.consume(work)).unwrap(),
        radiance: vec![Radiance {
            average: DVec3::new(32.0, 16.0, 8.0) / 255.0,
            reflectivity: DVec3::ONE,
            samples: Vec::new(),
            dimensions: [0; 2],
            projection: [[0.0; 4]; 2],
            mins: [0; 2],
            sky: false,
            extras: Vec::new(),
            boundary: None,
        }],
        _allocation: budget.reserve_bytes(0).unwrap(),
    }
}

fn plate(owner: Option<u32>) -> PropTriangle {
    let mut triangle = PropTriangle::new([
        DVec3::new(-4096.0, -4096.0, 8.0),
        DVec3::new(4096.0, -4096.0, 8.0),
        DVec3::new(0.0, 4096.0, 8.0),
    ])
    .unwrap();
    if let Some(owner) = owner {
        triangle.set_owner(owner);
    }
    triangle
}

#[test]
fn source_search_keeps_nearby_positive_hits_and_shadow_endpoint_policy() {
    let control = Control::default();
    let mut budget = Budget::new(&control);
    let surfaces = floor(&mut budget);
    let triangle = Triangle::new([
        DVec3::new(-2048.0, -2048.0, 0.0),
        DVec3::new(2048.0, -2048.0, 0.0),
        DVec3::new(2048.0, 2048.0, 0.0),
    ])
    .unwrap();
    let point = DVec3::new(0.0, 0.0, 0.00001);
    let end = point + DVec3::NEG_Z * 65536.0;
    assert!(triangle.intersection(point, end).unwrap().is_none());
    let (fraction, _) = triangle.source_intersection(point, end).unwrap().unwrap();
    assert!((fraction - 1.52587890625e-10).abs() < 1e-22);
    assert!(
        triangle
            .source_intersection(DVec3::ZERO, end)
            .unwrap()
            .is_none()
    );
    let shadows = ShadowIndex::new(Vec::<Occluder>::new(), &mut |_| Ok(())).unwrap();
    let props = ShadowIndex::new(Vec::<PropTriangle>::new(), &mut |_| Ok(())).unwrap();
    let hit = surfaces
        .trace(point, DVec3::NEG_Z, 0.0, &shadows, &props, &mut budget)
        .unwrap()
        .unwrap();
    assert!(hit.front && !hit.sky);
    assert_eq!(hit.point, DVec3::ZERO);
    assert_eq!(hit.color * 255.0, DVec3::new(32.0, 16.0, 8.0));
    assert!(
        surfaces
            .trace(point, DVec3::Z, 0.0, &shadows, &props, &mut budget)
            .unwrap()
            .is_none()
    );
}

#[test]
fn reflected_owner_skip_preserves_other_unknown_and_world_blockers() {
    let control = Control::default();
    let mut budget = Budget::new(&control);
    let surfaces = floor(&mut budget);
    let shadows = ShadowIndex::new(Vec::<Occluder>::new(), &mut |_| Ok(())).unwrap();
    let point = DVec3::Z * 16.0;
    for (owners, skipped, visible) in [
        (vec![Some(7)], None, false),
        (vec![Some(7)], Some(7), true),
        (vec![Some(7)], Some(9), false),
        (vec![Some(7), Some(9)], Some(7), false),
        (vec![Some(9), Some(7)], Some(7), false),
        (vec![None], Some(7), false),
    ] {
        let props =
            ShadowIndex::new(owners.into_iter().map(plate).collect(), &mut |_| Ok(())).unwrap();
        let hit = surfaces
            .trace_skipping_owner(
                point,
                DVec3::NEG_Z,
                0.0,
                &shadows,
                &props,
                skipped,
                &mut budget,
            )
            .unwrap();
        assert_eq!(hit.is_some(), visible);
    }
    let hull = Occluder::new(
        vec![
            map_core::Plane {
                normal: DVec3::X,
                dist: 16.0,
            },
            map_core::Plane {
                normal: DVec3::NEG_X,
                dist: 16.0,
            },
            map_core::Plane {
                normal: DVec3::Y,
                dist: 16.0,
            },
            map_core::Plane {
                normal: DVec3::NEG_Y,
                dist: 16.0,
            },
            map_core::Plane {
                normal: DVec3::Z,
                dist: 12.0,
            },
            map_core::Plane {
                normal: DVec3::NEG_Z,
                dist: -8.0,
            },
        ],
        DVec3::new(-16.0, -16.0, 8.0),
        DVec3::new(16.0, 16.0, 12.0),
    )
    .unwrap();
    let world = ShadowIndex::new(vec![hull], &mut |_| Ok(())).unwrap();
    let props = ShadowIndex::new(vec![plate(Some(7))], &mut |_| Ok(())).unwrap();
    assert!(
        surfaces
            .trace_skipping_owner(
                point,
                DVec3::NEG_Z,
                0.0,
                &world,
                &props,
                Some(7),
                &mut budget
            )
            .unwrap()
            .is_none()
    );
    for cancelled in [false, true] {
        let control = Control::default();
        let mut budget = Budget::new(&control);
        if cancelled {
            control.cancel().unwrap();
        } else {
            budget.consume(100_000_000).unwrap();
        }
        assert!(
            surfaces
                .trace_skipping_owner(
                    point,
                    DVec3::NEG_Z,
                    0.0,
                    &shadows,
                    &props,
                    Some(7),
                    &mut budget
                )
                .is_err()
        );
    }
}
