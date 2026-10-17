use glam::{DMat3, DVec3};
use map_core::Plane;

use super::*;

fn box_geometry(mins: DVec3, maxs: DVec3) -> BrushGeometry {
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
    BrushGeometry::from_planes(&planes).unwrap()
}

fn transformed(geometry: &BrushGeometry, matrix: DMat3, translation: DVec3) -> BrushGeometry {
    let planes = geometry
        .faces
        .iter()
        .map(|face| {
            let normal = matrix * face.plane.normal;
            Plane {
                normal,
                dist: face.plane.dist + normal.dot(translation),
            }
        })
        .collect::<Vec<_>>();
    BrushGeometry::from_planes(&planes).unwrap()
}

#[test]
fn analytic_box_and_tetrahedron_volumes_survive_rigid_transforms() {
    let cube = box_geometry(DVec3::ZERO, DVec3::splat(100.0));
    let tetrahedron = BrushGeometry::from_planes(&[
        Plane {
            normal: -DVec3::X,
            dist: 0.0,
        },
        Plane {
            normal: -DVec3::Y,
            dist: 0.0,
        },
        Plane {
            normal: -DVec3::Z,
            dist: 0.0,
        },
        Plane {
            normal: DVec3::ONE.normalize(),
            dist: 60.0 / 3.0_f64.sqrt(),
        },
    ])
    .unwrap();
    let rotation = DMat3::from_rotation_z(0.37) * DMat3::from_rotation_y(0.23);
    for (matrix, translation) in [
        (DMat3::IDENTITY, DVec3::ZERO),
        (rotation, DVec3::new(12000.0, -14000.0, 8000.0)),
        (
            DMat3::from_diagonal(DVec3::new(-1.0, 1.0, 1.0)) * rotation,
            DVec3::splat(-9000.0),
        ),
    ] {
        for (geometry, expected) in [(&cube, 1_000_000.0), (&tetrahedron, 36_000.0)] {
            let actual = measure(&transformed(geometry, matrix, translation))
                .unwrap()
                .volume;
            assert!(
                (actual - expected).abs() < expected * 1e-8,
                "{actual} != {expected}"
            );
        }
    }
    let mut planes = tetrahedron
        .faces
        .iter()
        .map(|face| face.plane)
        .collect::<Vec<_>>();
    planes.push(Plane {
        normal: DVec3::X,
        dist: 30.0,
    });
    let truncated = BrushGeometry::from_planes(&planes).unwrap();
    assert!((measure(&truncated).unwrap().volume - 31_500.0).abs() < 1e-6);
}

#[test]
fn strict_overlap_excludes_clear_below_and_rejects_uncertain_equality() {
    let leaf = box_geometry(DVec3::ZERO, DVec3::splat(100.0));
    for (width, expected) in [(9.0, false), (11.0, true), (100.0, true)] {
        let helper = box_geometry(DVec3::ZERO, DVec3::new(width, 100.0, 100.0));
        assert_eq!(claims(&helper, &leaf, &mut |_| Ok(())).unwrap(), expected);
    }
    for width in [10.0, 10.000001, 9.999999] {
        let helper = box_geometry(DVec3::ZERO, DVec3::new(width, 100.0, 100.0));
        assert!(
            claims(&helper, &leaf, &mut |_| Ok(()))
                .unwrap_err()
                .contains("10%")
        );
    }
    for mins in [
        DVec3::new(100.0, 0.0, 0.0),
        DVec3::new(100.0, 100.0, 0.0),
        DVec3::splat(100.0),
    ] {
        let touching = box_geometry(mins, mins + DVec3::splat(20.0));
        assert!(!claims(&touching, &leaf, &mut |_| Ok(())).unwrap());
    }
}

#[test]
fn oblique_overlap_uses_hulls_and_is_bounded_before_construction() {
    let leaf = box_geometry(DVec3::ZERO, DVec3::splat(100.0));
    let helper = box_geometry(DVec3::ZERO, DVec3::new(12.0, 100.0, 100.0));
    let matrix = DMat3::from_rotation_z(0.41);
    let translation = DVec3::new(-12000.0, 10000.0, 5000.0);
    let leaf = transformed(&leaf, matrix, translation);
    let helper = transformed(&helper, matrix, translation);
    assert!(claims(&helper, &leaf, &mut |_| Ok(())).unwrap());
    let error = claims(&helper, &leaf, &mut |_| {
        Err("test shared geometry budget".into())
    })
    .unwrap_err();
    assert_eq!(error, "test shared geometry budget");
    let control = Control::default();
    let mut checkpoints = 0;
    let error = claims(&helper, &leaf, &mut |_| {
        checkpoints += 1;
        if checkpoints == 2 {
            control.cancel()?;
        }
        control.checkpoint()
    })
    .unwrap_err();
    assert_eq!(checkpoints, 2);
    assert_eq!(error, "Compilation cancelled");

    let slender = box_geometry(DVec3::new(-50.0, -2.0, 0.0), DVec3::new(50.0, 2.0, 10.0));
    let a = transformed(&slender, matrix, DVec3::ZERO);
    let b = transformed(&slender, matrix, matrix * DVec3::new(0.0, 12.0, 0.0));
    assert!(a.mins.cmplt(b.maxs).all() && b.mins.cmplt(a.maxs).all());
    assert!(!claims(&a, &b, &mut |_| Ok(())).unwrap());
}

#[test]
fn open_inverted_and_nonfinite_hulls_fail_measurement() {
    let cube = box_geometry(DVec3::ZERO, DVec3::splat(100.0));
    let mut open = cube.clone();
    open.faces.pop();
    assert!(measure(&open).is_err());
    let mut inverted = cube.clone();
    inverted.faces[0].vertices.reverse();
    assert!(measure(&inverted).is_err());
    let mut invalid = cube;
    invalid.faces[0].vertices[0].x = f64::NAN;
    assert!(measure(&invalid).is_err());
}

#[test]
fn edge_cross_axes_certify_disjoint_skew_hulls_when_face_axes_overlap() {
    let rod = box_geometry(DVec3::new(-100.0, -0.1, -0.1), DVec3::new(100.0, 0.1, 0.1));
    let u = DVec3::ONE.normalize();
    let v = DVec3::new(1.0, -1.0, 0.0).normalize();
    let matrix = DMat3::from_cols(u, v, u.cross(v));
    let shifted = transformed(&rod, matrix, DVec3::X.cross(u).normalize() * 5.0);
    assert!(rod.mins.cmplt(shifted.maxs).all() && shifted.mins.cmplt(rod.maxs).all());
    assert!(separated_edges(&rod, &shifted, &mut |_| Ok(())).unwrap());
    assert!(!claims(&rod, &shifted, &mut |_| Ok(())).unwrap());

    let cube = box_geometry(DVec3::splat(-100.0), DVec3::splat(100.0));
    let r = DVec3::new(1.0, -1.0, 0.0).normalize();
    let t = DVec3::new(1.0, 1.0, -2.0).normalize();
    let matrix = DMat3::from_cols(u, (r + t).normalize(), (-r + t).normalize());
    let shifted = transformed(&cube, matrix, -r * 290.0);
    assert!(cube.mins.cmplt(shifted.maxs).all() && shifted.mins.cmplt(cube.maxs).all());
    assert!(separated_edges(&cube, &shifted, &mut |_| Ok(())).unwrap());
    assert!(!claims(&cube, &shifted, &mut |_| Ok(())).unwrap());
}

#[test]
fn invalid_helper_attempts_reserve_work_and_cannot_bypass_input_limit() {
    let source = crate::test_common::room();
    let game = crate::test_common::empty_game();
    let mut report = Report::default();
    let mut map = Map::load(&source, &game, &mut report).unwrap();
    let mut helper = Entity::new(1000, "func_viscluster");
    let mut solid = source.world.solids[0].clone();
    solid.sides[0].plane.0 = [DVec3::ZERO; 3];
    helper.solids.push(solid);
    let control = Control::default();
    for _ in 0..MAX_HELPERS {
        load(&mut map, &helper, &mut report, &control).unwrap();
    }
    assert_eq!(map.viscluster_attempts, MAX_HELPERS);
    assert!(map.visclusters.is_empty());
    assert!(map.viscluster_work >= MAX_HELPERS * MAX_POINTS * MAX_POINTS);
    let work = map.viscluster_work;
    load(&mut map, &helper, &mut report, &control).unwrap();
    assert_eq!(map.viscluster_work, work);
    assert_eq!(map.viscluster_attempts, MAX_HELPERS);
    assert!(
        report
            .diagnostics
            .last()
            .unwrap()
            .message
            .contains("including invalid helpers")
    );
}
