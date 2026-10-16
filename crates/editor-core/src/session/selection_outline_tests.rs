use super::*;

#[test]
fn selection_outlines_change_only_view_state_and_preserve_picks_faces_and_previews() {
    let mut session = Session::new(Err("no assets needed".into()));
    let object = session
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/material",
            false,
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let selection = session.selected_ids();
    let ray = Ray {
        origin: DVec3::new(32.0, -128.0, 32.0),
        direction: DVec3::Y,
    };
    let hit = session.pick(&ray);
    let candidates = session.object_hit_candidates(&ray).unwrap();
    let scene = session.build_scene();
    let serial = session.scene_serial();
    assert!(session.view().selection_outlines);
    assert!(!scene.selection_outline_ranges.is_empty());
    session
        .preview_block(DVec3::splat(128.0), DVec3::splat(160.0))
        .unwrap();
    let block_preview = session.block_preview();
    for enabled in [false, true] {
        session.set_view(ViewOptions {
            selection_outlines: enabled,
            ..session.view()
        });
        assert_eq!(session.view().selection_outlines, enabled);
        assert_eq!(session.pick(&ray), hit);
        assert_eq!(session.block_preview(), block_preview);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        assert_eq!(session.selected_ids(), selection);
        let changed = session.build_scene();
        assert_eq!(changed.vertices, scene.vertices);
        assert_eq!(changed.indices, scene.indices);
        assert_eq!(changed.lines, scene.lines);
        assert_eq!(
            changed.selection_outline_ranges,
            scene.selection_outline_ranges
        );
        assert_eq!(session.scene_serial(), serial);
    }
    session.cancel_block_preview();
    assert_eq!(session.object_hit_candidates(&ray).unwrap(), candidates);
    let ObjectId::Solid(solid) = object else {
        unreachable!()
    };
    let face = FaceId {
        solid,
        side: session.document().solid(solid).unwrap().sides[0].id,
    };
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    let face_pick = session.pick(&ray);
    session.set_view(ViewOptions {
        selection_outlines: false,
        ..session.view()
    });
    assert_eq!(session.selected_faces(), [face]);
    assert_eq!(session.pick(&ray), face_pick);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
}

#[test]
fn selection_outlines_leave_measurement_guides_in_the_unfiltered_line_channel() {
    let mut session = Session::new(Err("no assets needed".into()));
    let request = measurements::Request {
        id: None,
        name: "Clearance".into(),
        start: DVec3::new(0.0, 0.0, 96.0),
        end: DVec3::new(128.0, 0.0, 96.0),
    };
    session
        .preview_measurement(Actor::Agent, request, &session.measurement_stamp())
        .unwrap();
    let before = session.build_scene();
    assert!(!before.lines.is_empty());
    assert!(before.selection_outline_ranges.is_empty());
    session.set_view(ViewOptions {
        selection_outlines: false,
        ..session.view()
    });
    let after = session.build_scene();
    assert_eq!(after.lines, before.lines);
    assert_eq!(after.guide_line_ranges(), before.guide_line_ranges());
    assert!(session.measurement_state()["draft"].is_object());
    assert_eq!(session.measurement_state()["draft"]["valid"], true);
}
