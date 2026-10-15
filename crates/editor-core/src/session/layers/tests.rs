use super::*;

#[test]
fn versioned_layer_drafts_are_atomic_and_do_not_cancel_foreign_work() {
    let (mut session, id, _) = crate::test_assets::clipboard();
    let before = session.document.vmf().clone();
    let report = session
        .begin_layer_edit(None, false, &session.layer_stamp())
        .unwrap();
    let token = report["draft"]["token"].as_u64().unwrap();
    let report = session
        .update_layer_edit(
            token,
            0,
            "Architecture".into(),
            [40, 160, 220],
            None,
            vec![id],
        )
        .unwrap();
    assert_eq!(report["draft"]["version"], 1);
    assert_eq!(session.document.vmf(), &before);
    assert!(session.apply_layer_edit(Actor::Agent, token, 0).is_err());
    assert!(session.cancel_layer_edit(token, 0).is_err());
    let report = session.apply_layer_edit(Actor::User, token, 1).unwrap();
    let layer = report["layer"].as_u64().unwrap() as u32;
    assert_eq!(
        Graph::read(&session.document)
            .unwrap()
            .members(layer, false, true)
            .unwrap(),
        vec![id]
    );
    session.undo(Actor::Agent).unwrap();
    assert_eq!(session.document.vmf(), &before);
    session.redo(Actor::Agent).unwrap();
    let report = session
        .begin_layer_edit(Some(layer), false, &session.layer_stamp())
        .unwrap();
    let token = report["draft"]["token"].as_u64().unwrap();
    session
        .update_layer_edit(token, 0, "Invalid".into(), [0, 0, 0], Some(layer), vec![id])
        .unwrap();
    let before = session.document.vmf().clone();
    assert!(session.apply_layer_edit(Actor::Agent, token, 1).is_err());
    assert_eq!(session.document.vmf(), &before);
    session.cancel_layer_edit(token, 1).unwrap();
    let report = session
        .begin_layer_edit(Some(layer), true, &session.layer_stamp())
        .unwrap();
    let token = report["draft"]["token"].as_u64().unwrap();
    session.new_map(Actor::Agent, true).unwrap();
    assert!(session.apply_layer_edit(Actor::Agent, token, 0).is_err());
    assert!(session.cancel_layer_edit(token, 0).is_err());
}

#[test]
fn layer_selection_visibility_and_clipboard_share_native_memberships() {
    let (mut session, id, _) = crate::test_assets::clipboard();
    let report = session
        .begin_layer_edit(None, false, &session.layer_stamp())
        .unwrap();
    let token = report["draft"]["token"].as_u64().unwrap();
    let layer = session.apply_layer_edit(Actor::Agent, token, 0).unwrap()["layer"]
        .as_u64()
        .unwrap() as u32;
    let ObjectId::Solid(solid) = id else {
        unreachable!()
    };
    let stamp = session.layer_stamp();
    session
        .set_layer_visibility(Actor::User, layer, true, "hide", &stamp)
        .unwrap();
    assert!(session.document.solid(solid).unwrap().hidden);
    session
        .select_layer_members(
            layer,
            true,
            false,
            SelectMode::Replace,
            &session.layer_stamp(),
        )
        .unwrap();
    assert!(session.selected_ids().is_empty());
    session
        .select_layer_members(
            layer,
            true,
            true,
            SelectMode::Replace,
            &session.layer_stamp(),
        )
        .unwrap();
    assert_eq!(session.selected_ids(), vec![id]);
    session.undo(Actor::Agent).unwrap();
    assert!(!session.document.solid(solid).unwrap().hidden);
    session
        .capture_clipboard(
            Actor::Agent,
            &[id],
            &session.clipboard_stamp(),
            false,
            false,
        )
        .unwrap();
    let token = session
        .preview_paste(
            map_core::transfer::Options::default(),
            &session.clipboard_stamp(),
        )
        .unwrap()["preview"]["token"]
        .as_u64()
        .unwrap();
    session.apply_paste(Actor::Agent, token).unwrap();
    let graph = Graph::read(&session.document).unwrap();
    assert_eq!(graph.records.len(), 2);
    assert_eq!(
        graph.memberships[&session.selected_ids()[0]].layers.len(),
        1
    );
}
