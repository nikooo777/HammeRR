use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use std::cell::RefCell;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    document: DocumentHandle,
    access: DocumentAccess,
    shared: SharedSession,
    window: ComponentsWindow,
    face: Component,
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("No assets needed".into()));
        session.configure_recovery(Ok(directory.path().join("recovery")));
        let map_core::ObjectId::Solid(solid) = session
            .create_block(Actor::User, DVec3::ZERO, DVec3::splat(64.0), "test", false)
            .unwrap()
        else {
            unreachable!()
        };
        let side = session
            .document()
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let shared = Arc::new(Mutex::new(session));
        let document = crate::test_assets::document(shared.clone());
        let access = DocumentAccess::for_document(document.clone());
        let window = ComponentsWindow::new().unwrap();
        bind_with_access(&window, &access);
        let fixture = Self {
            directory,
            document,
            access,
            shared,
            window,
            face: Component {
                solid,
                sides: vec![side],
            },
        };
        fixture.reset();
        fixture
    }

    fn reset(&self) {
        let mut session = lock(&self.shared);
        let stamp = session.component_stamp();
        session
            .set_component_picking(Some(Kind::Face), &[self.face.solid], &stamp)
            .unwrap();
        let stamp = session.component_stamp();
        session
            .select_components(
                std::slice::from_ref(&self.face),
                SelectMode::Replace,
                &stamp,
            )
            .unwrap();
        self.window.set_kind(2);
        self.window.set_operation(0);
        refresh(&self.window, &session);
    }

    fn stage_partition(&self) {
        self.window.set_operation(6);
        self.window.invoke_preview_edit();
        assert!(
            self.window.get_has_partition_preview(),
            "{}",
            self.window.get_message()
        );
        assert!(self.window.get_partition_flat());
        assert!(self.window.get_ready());
    }

    fn report(&self) -> serde_json::Value {
        lock(&self.shared).component_report(&[]).unwrap()
    }
}

fn rows(model: slint::ModelRc<ComponentRow>) -> Vec<(String, String, String, bool)> {
    model
        .iter()
        .map(|row| {
            (
                row.key.to_string(),
                row.label.to_string(),
                row.detail.to_string(),
                row.selected,
            )
        })
        .collect()
}

fn form(window: &ComponentsWindow) -> serde_json::Value {
    serde_json::json!({
        "rows": rows(window.get_rows()),
        "partitions": rows(window.get_partition_rows()),
        "saved": rows(window.get_saved_partitions()),
        "creases": window.get_crease_rows().iter().map(|row| (
            row.key.to_string(), row.label.to_string(), row.choice,
            row.choices.iter().map(|choice| choice.to_string()).collect::<Vec<_>>()
        )).collect::<Vec<_>>(),
        "source_ids": window.get_source_ids().iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        "source_offset": window.get_source_offset(), "source_index": window.get_source_index(),
        "stamp": window.get_stamp().to_string(), "token": window.get_token().to_string(),
        "summary": window.get_summary().to_string(), "preview": window.get_preview_summary().to_string(),
        "ready": window.get_ready(), "picking": window.get_picking(),
        "partition": window.get_has_partition_preview(), "flat": window.get_partition_flat(),
        "extrusion": window.get_has_extrusion_preview(),
        "top_material": window.get_top_material().to_string(),
        "top_material_scope": window.get_top_material_scope().to_string(),
        "rotation": window.get_texture_rotation().to_string(),
        "rotation_value": window.get_texture_rotation_value(),
        "pivot": [window.get_pivot_x().to_string(), window.get_pivot_y().to_string(), window.get_pivot_z().to_string()],
        "weld": [window.get_weld_x().to_string(), window.get_weld_y().to_string(), window.get_weld_z().to_string()],
        "rise": window.get_split_rise().to_string(), "rise_value": window.get_split_rise_value(),
        "height": window.get_extrusion_height().to_string(), "height_value": window.get_extrusion_height_value(),
        "cut": [window.get_cut_first().to_string(), window.get_cut_second().to_string()],
        "cut_values": [window.get_cut_first_value(), window.get_cut_second_value()],
    })
}

fn rejected_actions(window: &ComponentsWindow, error: &str) {
    let component = window.get_rows().row_data(0).unwrap().key;
    let crease = window
        .get_crease_rows()
        .row_data(0)
        .map(|row| row.key)
        .unwrap_or_default();
    let partition = window
        .get_partition_rows()
        .row_data(0)
        .map(|row| row.key)
        .unwrap_or_default();
    macro_rules! rejected {
        ($action:expr) => {{
            $action;
            assert_eq!(window.get_message(), error);
        }};
    }
    rejected!(window.invoke_choose_texture_rotation(45.0));
    rejected!(window.invoke_preview_partition_texture());
    rejected!(window.invoke_fit_partition_texture());
    rejected!(window.invoke_justify_partition_texture());
    rejected!(window.invoke_inherit_partition_texture());
    rejected!(window.invoke_preview_partition_material());
    rejected!(window.invoke_inherit_partition_material());
    rejected!(window.invoke_resume_saved_partition());
    rejected!(window.invoke_remove_saved_partition());
    rejected!(window.invoke_choose_partition(partition));
    rejected!(window.invoke_frame_partitions());
    rejected!(window.invoke_flatten_partitions());
    rejected!(window.invoke_choose_crease_pair(crease, 1));
    rejected!(window.invoke_choose_source());
    rejected!(window.invoke_all_sources());
    rejected!(window.invoke_page_sources(1));
    rejected!(window.invoke_toggle_picking());
    rejected!(window.invoke_change_kind());
    rejected!(window.invoke_refresh());
    rejected!(window.invoke_choose(component));
    rejected!(window.invoke_select_all());
    rejected!(window.invoke_clear());
    rejected!(window.invoke_choose_pivot());
    rejected!(window.invoke_choose_weld_mode());
    rejected!(window.invoke_edited());
    rejected!(window.invoke_choose_cut_position(0, 25.0));
    rejected!(window.invoke_preview_cut());
    rejected!(window.invoke_choose_extrusion_height(48.0));
    rejected!(window.invoke_choose_rise(24.0));
    rejected!(window.invoke_move_options());
    rejected!(window.invoke_preview_edit());
    rejected!(window.invoke_apply());
    rejected!(window.invoke_discard_preview());
    window.show().unwrap();
    rejected!(window.invoke_cancel());
    assert!(!window.window().is_visible());
    window.show().unwrap();
    rejected!(
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::CloseRequested)
    );
    assert!(!window.window().is_visible());
}

fn backend() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
}

#[test]
fn managed_components_preserve_sources_and_previews_through_real_drain_and_retirement() {
    backend();
    let target = Fixture::new();
    let peer = Fixture::new();
    let original = lock(&target.shared).document().vmf().clone();
    target.window.invoke_toggle_picking();
    assert!(!target.window.get_picking());
    target.window.invoke_toggle_picking();
    assert!(target.window.get_picking());
    target.window.invoke_all_sources();
    target.window.set_source_index(1);
    target.window.invoke_choose_source();
    assert!(target.window.get_message().is_empty());
    target.window.invoke_page_sources(1);
    assert_eq!(target.window.get_source_offset(), 0);
    target.reset();
    target.window.invoke_choose(key(&target.face).into());
    assert!(lock(&target.shared).component_selection().is_empty());
    target.window.invoke_choose(key(&target.face).into());
    target.window.set_pivot_mode(1);
    target.window.invoke_choose_pivot();
    assert_eq!(target.window.get_pivot_z(), "64");
    target.window.invoke_preview_edit();
    assert!(target.window.get_ready(), "{}", target.window.get_message());
    target.window.invoke_discard_preview();
    assert_eq!(lock(&target.shared).render_document().vmf(), &original);
    target.window.invoke_preview_edit();
    let draft = lock(&target.shared).render_document().vmf().clone();
    target.window.invoke_apply();
    assert_eq!(lock(&target.shared).document().vmf(), &draft);
    lock(&target.shared).undo(Actor::User).unwrap();
    target.reset();
    target.stage_partition();
    target.window.set_pivot_mode(1);
    target.window.set_weld_target_mode(2);
    let expected_form = form(&target.window);
    let report = target.report();
    let history = lock(&target.shared).history_snapshot();
    let camera = lock(&target.shared).camera();
    let render = lock(&target.shared).render_document().vmf().clone();
    let peer_report = peer.report();
    let peer_form = form(&peer.window);
    let peer_source = lock(&peer.shared).document().vmf().clone();
    let gate = target.document.admit().unwrap();
    let draining = target.document.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let drain = std::thread::spawn(move || done.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while target.document.lifecycle() == DocumentLifecycle::Live
        || lock(&target.shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    rejected_actions(&target.window, "the editor is closing");
    assert_eq!(form(&target.window), expected_form);
    assert_eq!(target.report(), report);
    assert!(completed.try_recv().is_err());
    drop(gate);
    completed.recv_timeout(TIMEOUT).unwrap().unwrap();
    drain.join().unwrap();
    let store = crate::recovery::Store {
        directory: target.directory.path().join("recovery"),
    };
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let recovered = store.read(&entries[0].id).unwrap().1;
    rejected_actions(&target.window, "the document is closed");
    assert_eq!(form(&target.window), expected_form);
    assert_eq!(target.report(), report);
    assert_eq!(lock(&target.shared).document().vmf(), &original);
    assert_eq!(lock(&target.shared).render_document().vmf(), &render);
    assert_eq!(lock(&target.shared).history_snapshot(), history);
    assert_eq!(lock(&target.shared).camera(), camera);
    assert_eq!(store.list().unwrap().entries.len(), 1);
    assert_eq!(store.read(&entries[0].id).unwrap().1, recovered);
    assert_eq!(peer.report(), peer_report);
    assert_eq!(form(&peer.window), peer_form);
    assert_eq!(lock(&peer.shared).document().vmf(), &peer_source);
    peer.window.invoke_preview_edit();
    assert!(peer.window.get_ready());
    let peer_draft = lock(&peer.shared).render_document().vmf().clone();
    peer.window.invoke_apply();
    assert_eq!(lock(&peer.shared).document().vmf(), &peer_draft);
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    target.window.hide().unwrap();
    let weak = Arc::downgrade(&target.shared);
    let Fixture {
        directory: _directory,
        document,
        access,
        shared,
        window,
        face: _,
    } = target;
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak.upgrade().is_none());
    rejected_actions(&window, "the document is closed");
}

#[test]
fn managed_partition_callbacks_keep_material_cut_saved_and_extrusion_targets() {
    backend();
    let target = Fixture::new();
    let original = lock(&target.shared).document().vmf().clone();
    target.stage_partition();
    let crease = target.window.get_crease_rows().row_data(0).unwrap();
    target
        .window
        .invoke_choose_crease_pair(crease.key, crease.choice);
    assert!(!target.window.get_ready());
    target.window.invoke_preview_edit();
    assert!(target.window.get_ready());
    target.window.set_top_material("test/planned".into());
    target.window.invoke_edited();
    target.window.invoke_preview_partition_material();
    assert!(target.window.get_ready(), "{}", target.window.get_message());
    let material_target = partition_material_target(&lock(&target.shared));
    let first = target.window.get_partition_rows().row_data(0).unwrap().key;
    target.window.invoke_choose_partition(first.clone());
    let report = target.report();
    assert!(
        choose_partition_material(
            &target.window,
            &mut lock(&target.shared),
            &material_target,
            Some("test/stale")
        )
        .is_err()
    );
    assert_eq!(target.report(), report);
    target.window.invoke_choose_partition(first);
    target.window.invoke_frame_partitions();
    assert!(target.window.get_message().is_empty());
    target.window.invoke_inherit_partition_material();
    assert!(target.window.get_ready());
    target.window.invoke_choose_texture_rotation(45.0);
    target.window.invoke_preview_partition_texture();
    assert!(target.window.get_ready(), "{}", target.window.get_message());
    target.window.invoke_inherit_partition_texture();
    assert!(target.window.get_ready());
    target.window.invoke_choose_cut_position(0, 25.0);
    target.window.invoke_choose_cut_position(1, 25.0);
    target.window.invoke_preview_cut();
    assert!(target.window.get_ready());
    assert_eq!(
        target.report()["preview"]["partition"]["edge_parameters"],
        serde_json::json!([0.25, 0.25])
    );
    target.window.invoke_choose_rise(16.0);
    target.window.invoke_preview_edit();
    assert!(target.window.get_ready() && !target.window.get_partition_flat());
    target.window.invoke_flatten_partitions();
    assert!(target.window.get_ready() && target.window.get_partition_flat());
    target.window.invoke_apply();
    assert_eq!(target.window.get_saved_partitions().row_count(), 1);
    let saved = lock(&target.shared).document().vmf().clone();
    assert_eq!(saved.world, original.world);
    target.window.invoke_resume_saved_partition();
    assert!(target.window.get_has_partition_preview());
    target.window.invoke_discard_preview();
    target.window.invoke_remove_saved_partition();
    assert_eq!(target.window.get_saved_partitions().row_count(), 0);
    lock(&target.shared).undo(Actor::User).unwrap();
    refresh(&target.window, &lock(&target.shared));
    target.window.invoke_resume_saved_partition();
    assert!(target.window.get_has_partition_preview());
    target.window.set_partition_mode(1);
    target.window.invoke_choose_extrusion_height(32.0);
    target.window.invoke_preview_edit();
    assert!(
        target.window.get_ready() && target.window.get_has_extrusion_preview(),
        "{}",
        target.window.get_message()
    );
    let extrusion = lock(&target.shared).render_document().vmf().clone();
    target.window.invoke_apply();
    assert_eq!(lock(&target.shared).document().vmf(), &extrusion);
    lock(&target.shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&target.shared).document().vmf(), &saved);
    target.window.invoke_cancel();
}

#[test]
fn stale_component_tokens_cannot_apply_discard_or_close_foreign_preview() {
    backend();
    let target = Fixture::new();
    target.window.invoke_preview_edit();
    let owned = target.window.get_token();
    let stamp = target.window.get_stamp();
    {
        let mut session = lock(&target.shared);
        let current = session.component_stamp();
        session
            .preview_component_move(DVec3::Z * 24.0, false, false, &current)
            .unwrap();
    }
    let foreign = target.report();
    let render = lock(&target.shared).render_document().vmf().clone();
    let source = lock(&target.shared).document().vmf().clone();
    for action in 0..4 {
        target.window.set_token(owned.clone());
        target.window.set_stamp(stamp.clone());
        target.window.set_ready(true);
        target.window.show().unwrap();
        match action {
            0 => target.window.invoke_apply(),
            1 => target.window.invoke_discard_preview(),
            2 => target.window.invoke_cancel(),
            _ => target
                .window
                .window()
                .dispatch_event(slint::platform::WindowEvent::CloseRequested),
        }
        assert_eq!(target.report(), foreign);
        assert_eq!(lock(&target.shared).render_document().vmf(), &render);
        assert_eq!(lock(&target.shared).document().vmf(), &source);
    }
    target.window.hide().unwrap();
    refresh(&target.window, &lock(&target.shared));
    let expected = form(&target.window);
    let recovery = target.directory.path().join("recovery");
    if recovery.exists() {
        std::fs::remove_dir_all(&recovery).unwrap();
    }
    std::fs::write(&recovery, b"blocked").unwrap();
    let error = target.document.shutdown().unwrap_err();
    rejected_actions(
        &target.window,
        &format!("document shutdown failed: {error}"),
    );
    assert_eq!(form(&target.window), expected);
    assert_eq!(target.report(), foreign);
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(target.document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
}

#[test]
fn admitted_component_form_delegate_finishes_while_shutdown_waits_for_callback() {
    backend();
    let target = Fixture::new();
    target.stage_partition();
    let draining = target.document.clone();
    let observed = target.document.clone();
    let shared = target.shared.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let completed = Rc::new(RefCell::new(completed));
    let pending = completed.clone();
    let joining = Rc::new(RefCell::new(None));
    let joined = joining.clone();
    BEFORE_ACTION.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            *joined.borrow_mut() = Some(std::thread::spawn(move || {
                done.send(draining.shutdown()).unwrap()
            }));
            let deadline = Instant::now() + TIMEOUT;
            while observed.lifecycle() == DocumentLifecycle::Live
                || lock(&shared).compile_settings()["editable"] != false
            {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            assert!(pending.borrow().try_recv().is_err());
        }));
    });
    target.window.invoke_choose_rise(24.0);
    assert_eq!(target.window.get_split_rise(), "24");
    assert_eq!(target.window.get_split_rise_value(), 24.0);
    assert!(!target.window.get_ready());
    assert!(target.window.get_message().contains("Settings changed"));
    completed.borrow().recv_timeout(TIMEOUT).unwrap().unwrap();
    joining.borrow_mut().take().unwrap().join().unwrap();
    assert_eq!(target.document.lifecycle(), DocumentLifecycle::Finished);
}
