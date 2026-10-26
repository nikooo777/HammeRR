use std::sync::{Arc, Mutex};

use glam::DVec3;
use slint::Model;

use super::*;
use crate::runtime::{DocumentAccess, lock};

#[test]
fn graph_controls_share_full_selection_filtered_rows_framing_and_stale_refusal() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("graph needs no assets".into()));
    let mut ids = Vec::new();
    for (name, x) in [("source", 0.0), ("branch", 64.0), ("branch", 128.0)] {
        let object = session
            .create_entity(
                Actor::Agent,
                "logic_relay",
                Some(DVec3::new(x, 0.0, 64.0)),
                std::collections::BTreeMap::from([("targetname".into(), name.into())]),
            )
            .unwrap();
        let ObjectId::Entity(id) = object else {
            panic!()
        };
        ids.push(id);
    }
    session
        .edit(Actor::Agent, "shared fan-out", |tx| {
            for target in ["branch", "!activator"] {
                map_core::connections::set(
                    tx,
                    ids[0],
                    None,
                    &map_core::connections::Spec {
                        output: "OnTrigger".into(),
                        target: target.into(),
                        input: "Trigger".into(),
                        parameter: String::new(),
                        delay: 0.0,
                        times: -1,
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
    session
        .select(&[ObjectId::Entity(ids[0])], SelectMode::Replace)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let form = Form::new(&DocumentAccess::unmanaged(shared.clone())).unwrap();
    form.show(&lock(&shared), 1.5).unwrap();
    let window = &form.window;
    window.invoke_inspect();
    let captured_roots = window.get_roots();
    assert!(window.get_root_summary().contains("source · logic_relay"));
    assert!(window.get_can_act());
    assert_eq!(window.get_rows().row_count(), 3);
    assert!(window.get_summary().contains("3 reached nodes"));
    window.set_section("Relationships".into());
    window.invoke_change_section();
    assert_eq!(window.get_rows().row_count(), 2);
    window.set_section("Unresolved (whole map)".into());
    window.invoke_change_section();
    assert_eq!(window.get_rows().row_count(), 1);
    assert!(
        window
            .get_rows()
            .row_data(0)
            .unwrap()
            .title
            .contains("runtime_dependent")
    );
    window.set_search("source".into());
    window.invoke_fields_changed();
    assert!(!window.get_can_act());
    window.set_section("Nodes".into());
    window.invoke_inspect();
    assert_eq!(window.get_rows().row_count(), 1);
    let expected = lock(&shared)
        .inspect_entity_graph(form.state.borrow().query.as_ref().unwrap())
        .unwrap();
    assert_eq!(
        serde_json::to_value(form.state.borrow().report.as_ref().unwrap()).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    let history = lock(&shared).history_snapshot();
    window.invoke_select_connected();
    assert_eq!(lock(&shared).selected_ids().len(), 3);
    assert_eq!(window.get_roots(), captured_roots);
    assert_eq!(lock(&shared).history_snapshot(), history);
    let selected = lock(&shared).selected_ids();
    let camera = lock(&shared).camera();
    window.invoke_frame_connected();
    assert_eq!(lock(&shared).selected_ids(), selected);
    assert_ne!(lock(&shared).camera(), camera);
    window.set_section("Relationships".into());
    window.invoke_change_section();
    assert_eq!(window.get_rows().row_count(), 2);
    assert!(
        window
            .get_rows()
            .row_data(0)
            .unwrap()
            .title
            .contains("branch · logic_relay")
    );
    crate::ui::tests::assert_dialog_button_visible(window, "Select connected");
    crate::ui::tests::assert_dialog_button_visible(window, "Frame connected");
    crate::ui::tests::assert_dialog_button_visible(window, "Close");
    crate::ui::tests::save_ui_snapshot(window.window(), "entity-graph");
    lock(&shared)
        .edit(Actor::Agent, "changed graph", |tx| {
            tx.entity_mut(ids[1])?.properties.set("targetname", "other");
            Ok(())
        })
        .unwrap();
    let before = lock(&shared).selected_ids();
    window.invoke_select_connected();
    assert_eq!(lock(&shared).selected_ids(), before);
    assert!(!window.get_can_act());
    assert!(window.get_message().contains("snapshot changed"));
    window.invoke_inspect();
    assert!(window.get_can_act());
    lock(&shared)
        .edit(Actor::Agent, "invalidate idle graph", |tx| {
            tx.entity_mut(ids[2])?.properties.set("targetname", "other");
            Ok(())
        })
        .unwrap();
    form.refresh_visible(&lock(&shared), 1.0);
    assert!(!window.get_can_act());
    assert_eq!(window.get_rows().row_count(), 0);
    window.invoke_roots_from_selection();
    assert_eq!(traversal(window).unwrap().roots.len(), 3);
    assert!(!window.get_can_act());
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
}
