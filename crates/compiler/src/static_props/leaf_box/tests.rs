use super::*;

#[path = "../../../tests/common/mod.rs"]
mod common;

fn header() -> ModelHeader {
    ModelHeader {
        checksum: 0,
        flags: source_formats::mdl::STATIC_PROP_FLAG,
        hull_mins: [-64.0, -2.0, -1.0],
        hull_maxs: [64.0, 2.0, 1.0],
        render_mins: [-64.0, -2.0, -1.0],
        render_maxs: [64.0, 2.0, 1.0],
        illumination: [0.0; 3],
        skins: 1,
    }
}

fn plane(normal: DVec3, distance: f64) -> Plane {
    Plane {
        normal: normal.as_vec3().to_array(),
        dist: distance as f32,
        kind: 0,
    }
}

fn node(plane: i32, children: [i32; 2]) -> Node {
    Node {
        plane,
        children,
        ..Default::default()
    }
}

fn quadrants() -> Space {
    Space {
        nodes: vec![node(0, [1, 2]), node(1, [-1, -2]), node(1, [-3, -4])],
        planes: vec![plane(DVec3::X, 20.0), plane(DVec3::Y, -20.0)],
        leaves: vec![Leaf::default(); 4],
        head: 0,
        work: 0,
        clipped_points: 0,
    }
}

fn rotation(pitch: f64, yaw: f64, roll: f64) -> DQuat {
    DQuat::from_euler(
        EulerRot::ZYX,
        yaw.to_radians(),
        pitch.to_radians(),
        roll.to_radians(),
    )
}

fn leaf(space: &Space, point: DVec3) -> u16 {
    let mut current = space.head;
    while current >= 0 {
        let node = space.nodes[current as usize];
        let plane = space.planes[node.plane as usize];
        let distance =
            DVec3::from_array(plane.normal.map(f64::from)).dot(point) - f64::from(plane.dist);
        current = node.children[usize::from(distance < 0.0)];
    }
    (-i64::from(current) - 1) as u16
}

fn assert_sampled_boxes(space: &mut Space) {
    for pitch in [-70.0, 0.0, 25.0] {
        for yaw in [-135.0, 0.0, 45.0, 135.0] {
            for roll in [-40.0, 0.0, 80.0] {
                let turn = rotation(pitch, yaw, roll);
                let origin = DVec3::new(0.125, -0.25, 0.375);
                let leaves = space
                    .prop_leaves(header(), origin, turn, &Control::default())
                    .unwrap();
                for x in 0..=16 {
                    for y in 0..=4 {
                        for z in 0..=2 {
                            let local = DVec3::new(
                                -64.0 + x as f64 * 8.0,
                                -2.0 + y as f64,
                                -1.0 + z as f64,
                            );
                            assert!(leaves.contains(&leaf(space, origin + turn * local)));
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn rotated_box_rejects_empty_aabb_corners_and_retains_sampled_header_volume() {
    let mut space = quadrants();
    let turn = rotation(0.0, 45.0, 0.0);
    let broad = space
        .intersect(
            bounds(header(), DVec3::ZERO, turn).unwrap(),
            &Control::default(),
        )
        .unwrap();
    assert_eq!(broad, [0, 1, 2, 3]);
    let precise = space
        .prop_leaves(header(), DVec3::ZERO, turn, &Control::default())
        .unwrap();
    assert_eq!(precise, [0, 2, 3]);
    assert_sampled_boxes(&mut space);
    space.nodes = vec![
        node(0, [1, 2]),
        node(1, [3, 4]),
        node(1, [5, 6]),
        node(2, [-1, -2]),
        node(2, [-3, -4]),
        node(2, [-5, -6]),
        node(2, [-7, -8]),
    ];
    space.planes = vec![
        plane(DVec3::new(-1.0, 1.0, 0.0).normalize(), 1.0),
        plane(DVec3::new(0.0, 1.0, 1.0).normalize(), -2.0),
        plane(DVec3::new(1.0, 0.0, 1.0).normalize(), 3.0),
    ];
    space.leaves = vec![Leaf::default(); 8];
    assert_sampled_boxes(&mut space);
}

#[test]
fn rotated_boxes_keep_world_axis_boundary_padding_at_oblique_planes() {
    let turn = rotation(0.0, 45.0, 0.0);
    let normal = turn * DVec3::Y;
    let bias = LEAF_BIAS * normal.abs().element_sum();
    for (gap, expected) in [(bias - 1e-5, vec![0, 1]), (bias + 1e-5, vec![1])] {
        let mut space = quadrants();
        space.nodes = vec![node(0, [-1, -2])];
        space.planes = vec![plane(normal, 2.0 + gap)];
        assert_eq!(
            space
                .prop_leaves(header(), DVec3::ZERO, turn, &Control::default())
                .unwrap(),
            expected
        );
    }
}

#[test]
fn unrotated_membership_is_identical_and_degenerate_header_extents_remain_conservative() {
    let mut space = quadrants();
    for origin in [
        DVec3::ZERO,
        DVec3::new(84.005, 0.0, 0.0),
        DVec3::new(84.02, 0.0, 0.0),
    ] {
        let expected = space
            .intersect(
                bounds(header(), origin, DQuat::IDENTITY).unwrap(),
                &Control::default(),
            )
            .unwrap();
        assert_eq!(
            space
                .prop_leaves(header(), origin, DQuat::IDENTITY, &Control::default())
                .unwrap(),
            expected
        );
    }
    let mut flat = header();
    flat.render_mins[2] = 0.0;
    flat.render_maxs[2] = 0.0;
    flat.hull_mins = flat.render_mins;
    flat.hull_maxs = flat.render_maxs;
    let turn = rotation(30.0, 45.0, 0.0);
    let leaves = space
        .prop_leaves(flat, DVec3::ZERO, turn, &Control::default())
        .unwrap();
    for x in -64..=64 {
        assert!(leaves.contains(&leaf(&space, turn * DVec3::new(x as f64, 0.0, 0.0))));
    }
}

#[test]
fn clipping_limits_cancellation_cycles_and_bad_references_fail_explicitly() {
    let turn = rotation(0.0, 45.0, 0.0);
    for limits in [
        Limits {
            work: 0,
            ..Default::default()
        },
        Limits {
            points: 0,
            ..Default::default()
        },
        Limits {
            fragment_points: 23,
            ..Default::default()
        },
        Limits {
            fragment_faces: 5,
            ..Default::default()
        },
    ] {
        let mut space = quadrants();
        assert!(
            space
                .intersect_rotated(
                    header(),
                    DVec3::ZERO,
                    turn,
                    bounds(header(), DVec3::ZERO, turn).unwrap(),
                    &Control::default(),
                    limits
                )
                .unwrap_err()
                .contains("budget")
        );
    }
    let mut space = quadrants();
    let control = Control::default();
    let limits = Limits {
        cancel_after_work: Some(40),
        ..Default::default()
    };
    assert!(
        space
            .intersect_rotated(
                header(),
                DVec3::ZERO,
                turn,
                bounds(header(), DVec3::ZERO, turn).unwrap(),
                &control,
                limits
            )
            .unwrap_err()
            .contains("cancelled")
    );
    assert!(control.cancelled() && space.clipped_points > 0);
    let mut space = quadrants();
    space.nodes[0].children[0] = 0;
    assert!(
        space
            .intersect_rotated(
                header(),
                DVec3::ZERO,
                turn,
                bounds(header(), DVec3::ZERO, turn).unwrap(),
                &Control::default(),
                Limits {
                    work: 1000,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .contains("budget")
    );
    for invalid_plane in [false, true] {
        let mut space = quadrants();
        if invalid_plane {
            space.nodes[0].plane = i32::MAX;
        } else {
            space.nodes[0].children[0] = i32::MAX;
        }
        assert!(
            space
                .prop_leaves(header(), DVec3::ZERO, turn, &Control::default())
                .unwrap_err()
                .contains("invalid static-prop")
        );
    }
}

fn with_limits<T>(limits: Limits, operation: impl FnOnce() -> T) -> T {
    struct Restore(Option<Limits>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_LIMITS.with(|slot| slot.set(self.0));
        }
    }
    let _restore = Restore(TEST_LIMITS.with(|slot| slot.replace(Some(limits))));
    operation()
}

fn model() -> Vec<u8> {
    let mut data = vec![0; 240];
    data[..4].copy_from_slice(b"IDST");
    for (offset, value) in [(4, 48_i32), (76, 240), (152, 16), (224, 1)] {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, values) in [
        (104, header().hull_mins),
        (116, header().hull_maxs),
        (128, header().render_mins),
        (140, header().render_maxs),
    ] {
        for (axis, value) in values.into_iter().enumerate() {
            data[offset + axis * 4..offset + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    data
}

#[test]
fn real_compile_limits_preserve_previous_output_and_unrotated_bytes() {
    use source_formats::{
        gamefs::{Mount, MountedPath},
        vmf::Entity,
    };

    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("models")).unwrap();
    std::fs::write(directory.path().join("models/box.mdl"), model()).unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    let mut vmf = common::room();
    let id = vmf.object_ids().chain(vmf.side_ids()).max().unwrap() + 1;
    let mut prop = Entity::new(id, "prop_static");
    for (key, value) in [
        ("model", "models/box.mdl"),
        ("origin", "0 0 64"),
        ("solid", "2"),
        ("angles", "0 0 0"),
    ] {
        prop.properties.set(key, value);
    }
    vmf.entities.push(prop);
    let path = directory.path().join("previous.bsp");
    let baseline = crate::compile(&vmf, &game);
    assert!(!baseline.report.has_errors(), "{:?}", baseline.report);
    assert!(baseline.write(&path).unwrap());
    let bytes = std::fs::read(&path).unwrap();
    let zero = Limits {
        work: 0,
        points: 0,
        fragment_points: 0,
        fragment_faces: 0,
        cancel_after_work: Some(0),
    };
    let unchanged = with_limits(zero, || crate::compile(&vmf, &game));
    assert_eq!(unchanged.bsp.unwrap().to_bytes(), bytes);
    vmf.entities
        .last_mut()
        .unwrap()
        .properties
        .set("angles", "0 45 0");
    let source = vmf.clone();
    for limits in [
        Limits {
            points: 0,
            ..Default::default()
        },
        Limits {
            work: 1,
            ..Default::default()
        },
        Limits {
            cancel_after_work: Some(31),
            ..Default::default()
        },
    ] {
        let failed = with_limits(limits, || crate::compile(&vmf, &game));
        assert!(failed.report.has_errors() && failed.bsp.is_none());
        assert!(!failed.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(vmf, source);
    }
    let successful = crate::compile(&vmf, &game);
    assert!(!successful.report.has_errors(), "{:?}", successful.report);
    let reopened = source_formats::vmf::Vmf::parse(&vmf.write(&Default::default())).unwrap();
    assert_eq!(
        crate::compile(&reopened, &game).bsp.unwrap().to_bytes(),
        successful.bsp.unwrap().to_bytes()
    );
}
