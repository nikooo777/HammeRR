use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use serde_json::{Value, json};
use slint::Model;
use source_formats::fgd::Catalog;
use std::cell::Cell;
use std::sync::{Arc, Mutex, mpsc};

const TIMEOUT: Duration = Duration::from_secs(5);
type ColorTarget = Rc<RefCell<Option<(u32, String, u64)>>>;

struct Fixture {
    document: DocumentHandle,
    access: DocumentAccess,
    shared: SharedSession,
    _app: AppWindow,
    entity: EntityWindow,
    color: ColorWindow,
    color_target: ColorTarget,
    picker_owner: Option<Rc<()>>,
    placement: Rc<entity_placement_ui::Controller>,
    opens: Rc<Cell<usize>>,
    solid: ObjectId,
    directory: Rc<tempfile::TempDir>,
}

fn software() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
}

fn fixture() -> Fixture {
    let directory = Rc::new(tempfile::tempdir().unwrap());
    let (mut session, solid) = crate::test_assets::placement();
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session.set_definitions(
        Actor::User,
        Catalog::parse(
            r#"
@PointClass = fixture [ tint(color1) : "Tint" : "0.5 0.25 0.75" ]
@SolidClass = func_detail []
"#,
        )
        .unwrap(),
    );
    let ObjectId::Entity(id) = session
        .create_entity(Actor::User, "fixture", None, BTreeMap::new())
        .unwrap()
    else {
        panic!()
    };
    let generation = session.document_generation();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let app = AppWindow::new().unwrap();
    let entity = EntityWindow::new().unwrap();
    let picker_owner = Rc::new(());
    bind_entity_actions(&entity, &app, &access, {
        let owner = Rc::downgrade(&picker_owner);
        move || owner.upgrade().is_some()
    });
    entity.set_classname("fixture".into());
    entity.set_origin_x("16".into());
    entity.set_origin_y("32".into());
    entity.set_origin_z("64".into());
    entity.invoke_filter_changed();
    let placement = Rc::new(entity_placement_ui::Controller::new_with_access(&access).unwrap());
    let opens = Rc::new(Cell::new(0));
    bind_entity_pick_surface(&entity, &access, {
        let (placement, opens) = (Rc::downgrade(&placement), Rc::downgrade(&opens));
        move |window, shared| {
            let (Some(placement), Some(opens)) = (placement.upgrade(), opens.upgrade()) else {
                return;
            };
            opens.set(opens.get() + 1);
            placement.initialize(&lock(shared), &window.get_classname());
            let _ = window.hide();
            placement.window.show().unwrap();
        }
    });
    let color = ColorWindow::new().unwrap();
    let color_target = Rc::new(RefCell::new(Some((id, "tint".into(), generation))));
    bind_color_actions(&color, &access, {
        let target = Rc::downgrade(&color_target);
        move || target.upgrade().and_then(|target| target.borrow().clone())
    });
    color.set_red(255);
    color.set_green(0);
    color.set_blue(0);
    Fixture {
        document,
        access,
        shared,
        _app: app,
        entity,
        color,
        color_target,
        picker_owner: Some(picker_owner),
        placement,
        opens,
        solid,
        directory,
    }
}

fn state(shared: &SharedSession) -> Value {
    let session = lock(shared);
    json!({
        "map": session.document().vmf().write(&Default::default()),
        "render": session.render_document().vmf().write(&Default::default()),
        "history": session.history_snapshot(), "placement": session.get_entity_placement(),
        "property_draft": session.property_draft(), "camera": format!("{:?}", session.camera()),
        "generation": session.document_generation(), "scene": session.scene_serial(),
        "view": session.view_serial(), "log": session.log_serial(),
        "definitions": session.definitions_serial(),
        "selection": session.selected_ids().iter().map(ToString::to_string).collect::<Vec<_>>()
    })
}

fn form(f: &Fixture) -> Value {
    let placement = &f.placement.window;
    json!({
        "entity": {"filter": f.entity.get_filter().to_string(), "class": f.entity.get_classname().to_string(),
            "rows": f.entity.get_classes().iter().map(|row| (row.classname.to_string(), row.description.to_string())).collect::<Vec<_>>(),
            "definition": f.entity.get_definition().to_string(), "status": f.entity.get_catalog_status().to_string(),
            "brush": f.entity.get_brush_mode(), "origin": [f.entity.get_origin_x().to_string(), f.entity.get_origin_y().to_string(), f.entity.get_origin_z().to_string()]},
        "color": {"target": *f.color_target.borrow(), "red": f.color.get_red(), "green": f.color.get_green(),
            "blue": f.color.get_blue(), "intensity": f.color.get_intensity().to_string(), "has_intensity": f.color.get_has_intensity()},
        "placement": {"class": placement.get_classname().to_string(), "offset": placement.get_offset().to_string(),
            "align": placement.get_align(), "snap": placement.get_snap(), "summary": placement.get_summary().to_string(),
            "grid": placement.get_grid().to_string(), "ready": placement.get_ready(), "owned": placement.get_owned(), "shared": placement.get_shared_draft()},
        "opens": f.opens.get()
    })
}

fn pick(f: &Fixture) {
    f.placement.window.invoke_pick();
    assert_eq!(
        lock(&f.shared).get_entity_placement()["draft"]["picking"],
        true
    );
    f.placement
        .pick_at_ray(
            &mut lock(&f.shared),
            Ray {
                origin: DVec3::new(32.0, 0.0, 64.0),
                direction: DVec3::Y,
            },
        )
        .unwrap();
    assert!(f.placement.window.get_ready());
}

fn rejected(f: &Fixture, error: &str) {
    let before = state(&f.shared);
    let fields = form(f);
    f.entity.show().unwrap();
    f.color.show().unwrap();
    f.placement.window.show().unwrap();
    f.entity.invoke_filter_changed();
    f.entity.invoke_class_selected();
    f.entity.invoke_pick_surface();
    f.entity.invoke_create();
    assert_eq!(f.entity.get_message(), error);
    f.color.invoke_apply();
    assert_eq!(f.color.get_message(), error);
    f.placement.window.invoke_invalidate();
    f.placement.window.invoke_pick();
    f.placement.window.invoke_review();
    f.placement.window.invoke_apply();
    assert_eq!(f.placement.window.get_message(), error);
    assert_eq!(state(&f.shared), before);
    assert_eq!(form(f), fields);
    assert!(f.entity.window().is_visible());
    assert!(f.color.window().is_visible());
    assert!(f.placement.window.window().is_visible());
    f.entity.invoke_cancel();
    f.color.invoke_cancel();
    f.placement.window.invoke_cancel();
    assert!(!f.entity.window().is_visible());
    assert!(!f.color.window().is_visible());
    assert!(!f.placement.window.window().is_visible());
    f.entity.show().unwrap();
    f.entity
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    f.color.show().unwrap();
    f.color
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!f.entity.window().is_visible());
    assert!(!f.color.window().is_visible());
    f.placement.window.show().unwrap();
    f.placement
        .window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!f.placement.window.window().is_visible());
    assert_eq!(state(&f.shared), before);
    assert_eq!(form(f), fields);
}

#[test]
fn entity_color_and_surface_placement_act_live_then_reject_real_drain_with_live_peer() {
    software();
    let f = fixture();
    let peer = fixture();
    assert_eq!(f.entity.get_classes().row_count(), 1);
    f.entity.set_filter("missing".into());
    f.entity.invoke_filter_changed();
    assert_eq!(f.entity.get_classes().row_count(), 0);
    f.entity.set_filter("".into());
    f.entity.invoke_class_selected();
    assert_eq!(f.entity.get_classes().row_count(), 1);
    let count = lock(&f.shared).document().vmf().entities.len();
    f.entity.invoke_create();
    assert_eq!(lock(&f.shared).document().vmf().entities.len(), count + 1);
    lock(&f.shared)
        .select(&[f.solid], SelectMode::Replace)
        .unwrap();
    f.entity.set_brush_mode(true);
    f.entity.set_classname("func_detail".into());
    f.entity.invoke_create();
    assert!(
        lock(&f.shared)
            .document()
            .vmf()
            .entities
            .iter()
            .any(|entity| entity.classname() == Some("func_detail"))
    );
    f.color.invoke_apply();
    let id = f.color_target.borrow().as_ref().unwrap().0;
    assert_eq!(
        lock(&f.shared)
            .document()
            .entity(id)
            .unwrap()
            .properties
            .get("tint"),
        Some("1 0 0")
    );
    f.entity.set_brush_mode(false);
    f.entity.set_classname("fixture".into());
    f.entity.invoke_pick_surface();
    assert_eq!(f.opens.get(), 1);
    assert!(f.placement.window.window().is_visible());
    pick(&f);
    let history = lock(&f.shared).document().undo_labels().count();
    f.placement.window.invoke_apply();
    assert!(lock(&f.shared).get_entity_placement()["draft"].is_null());
    assert_eq!(
        lock(&f.shared).document().undo_labels().count(),
        history + 1
    );
    lock(&f.shared).undo(Actor::User).unwrap();
    f.entity.invoke_pick_surface();
    pick(&f);
    let gate = f.document.admit().unwrap();
    let draining = f.document.clone();
    let (send, receive) = mpsc::channel();
    let thread = std::thread::spawn(move || send.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while f.document.lifecycle() == DocumentLifecycle::Live
        || lock(&f.shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(receive.try_recv().is_err());
    let peer_before = (state(&peer.shared), form(&peer));
    rejected(&f, "the editor is closing");
    assert_eq!((state(&peer.shared), form(&peer)), peer_before);
    drop(gate);
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    thread.join().unwrap();
    assert_eq!(f.document.lifecycle(), DocumentLifecycle::Finished);
    rejected(&f, "the document is closed");
    assert_eq!((state(&peer.shared), form(&peer)), peer_before);
    peer.color.invoke_apply();
    assert_ne!(state(&peer.shared), peer_before.0);
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    let entries = lock(&f.shared).recovery_entries().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let store = recovery::Store {
        directory: f.directory.path().join("recovery"),
    };
    let (_, text) = store.read(&entries[0].id).unwrap();
    assert_eq!(
        source_formats::vmf::Vmf::parse(&text).unwrap(),
        *lock(&f.shared).document().vmf()
    );
    peer.document.shutdown().unwrap();
}

#[test]
fn stale_color_generation_and_exact_placement_tokens_preserve_replacements_and_foreign_owners() {
    software();
    let f = fixture();
    f.entity.invoke_pick_surface();
    pick(&f);
    let old = lock(&f.shared).get_entity_placement()["draft"]["token"]
        .as_u64()
        .unwrap();
    lock(&f.shared)
        .cancel_entity_placement(Actor::User, old)
        .unwrap();
    let mut session = lock(&f.shared);
    let snapshot = session.entity_placement_stamp();
    let foreign = session
        .begin_entity_placement(
            Actor::Agent,
            session::entity_placement::Request {
                classname: "fixture".into(),
                properties: BTreeMap::new(),
                offset: 3.0,
                align: false,
                snap: false,
            },
            &snapshot,
        )
        .unwrap();
    drop(session);
    let before = state(&f.shared);
    f.placement.window.invoke_pick();
    f.placement.window.invoke_review();
    f.placement.window.invoke_apply();
    f.placement.window.invoke_cancel();
    assert_eq!(state(&f.shared), before);
    f.placement.initialize(&lock(&f.shared), "fixture");
    assert!(!f.placement.window.get_owned());
    f.placement.window.invoke_apply();
    f.placement.window.invoke_cancel();
    assert_eq!(state(&f.shared), before);
    f.placement.initialize(&lock(&f.shared), "fixture");
    f.placement.window.invoke_review();
    assert!(f.placement.window.get_owned());
    assert_ne!(
        lock(&f.shared).get_entity_placement()["draft"]["token"],
        foreign["draft"]["token"]
    );
    f.placement.window.invoke_cancel();
    assert!(lock(&f.shared).get_entity_placement()["draft"].is_null());
    lock(&f.shared).new_map(Actor::Agent, true).unwrap();
    let before = state(&f.shared);
    f.color.invoke_apply();
    assert_eq!(
        f.color.get_message(),
        "Map changed. Open this picker again."
    );
    assert_eq!(state(&f.shared), before);
    f.document.shutdown().unwrap();
}

#[test]
fn retained_entity_family_windows_release_document_owners_and_vanished_origins_abandon_access() {
    software();
    let f = fixture();
    let cold = EntityWindow::new().unwrap();
    let vanished = cold.as_weak();
    drop(cold);
    with_entity(
        &vanished,
        &f.access.downgrade(),
        || panic!("vanished entity picker checked its parent"),
        |_, _| panic!("vanished entity picker accessed document"),
    );
    let cold = ColorWindow::new().unwrap();
    let vanished = cold.as_weak();
    drop(cold);
    with_color(&vanished, &f.access.downgrade(), |_, _| {
        panic!("vanished color picker accessed document")
    });
    f.entity.invoke_pick_surface();
    f.placement.window.invoke_pick();
    f.entity.hide().unwrap();
    f.color.hide().unwrap();
    f.placement.window.hide().unwrap();
    let entity = f.entity.clone_strong();
    let color = f.color.clone_strong();
    let placement = f.placement.window.clone_strong();
    let weak_controller = Rc::downgrade(&f.placement);
    let weak_target = Rc::downgrade(&f.color_target);
    let weak_session = Arc::downgrade(&f.shared);
    let continuation = f.access.downgrade();
    let store = recovery::Store {
        directory: f.directory.path().join("recovery"),
    };
    let _directory = f.directory.clone();
    drop(f);
    assert!(weak_controller.upgrade().is_none());
    assert!(weak_target.upgrade().is_none());
    assert!(weak_session.upgrade().is_none());
    assert!(continuation.upgrade().is_err());
    assert_eq!(store.list().unwrap().entries.len(), 1);
    entity.invoke_filter_changed();
    entity.invoke_create();
    entity.invoke_pick_surface();
    color.invoke_apply();
    placement.invoke_pick();
    placement.invoke_review();
    placement.invoke_apply();
    assert_eq!(entity.get_message(), "the document is closed");
    assert_eq!(color.get_message(), "the document is closed");
    assert_eq!(placement.get_message(), "the document is closed");
    entity.invoke_cancel();
    color.invoke_cancel();
    placement.invoke_cancel();
}

#[test]
fn retained_entity_actions_abandon_a_lost_picker_owner_while_the_document_stays_live() {
    software();
    let mut f = fixture();
    let owner = Rc::downgrade(f.picker_owner.as_ref().unwrap());
    f.entity.set_filter("missing".into());
    f.entity
        .set_definition("Retained parent-owned definition".into());
    let before = (state(&f.shared), form(&f));
    drop(f.picker_owner.take());
    assert!(owner.upgrade().is_none());
    f.entity.invoke_filter_changed();
    f.entity.invoke_class_selected();
    f.entity.invoke_create();
    assert_eq!((state(&f.shared), form(&f)), before);
    assert_eq!(f.document.lifecycle(), DocumentLifecycle::Live);
    f.color.invoke_apply();
    assert_ne!(state(&f.shared), before.0);
    f.document.shutdown().unwrap();
}

#[test]
fn failed_entity_family_admission_preserves_forms_and_memoized_recovery_failure() {
    software();
    let f = fixture();
    f.entity.invoke_pick_surface();
    pick(&f);
    let path = f.directory.path().join("recovery");
    std::fs::write(&path, b"blocked").unwrap();
    let error = f.document.shutdown().unwrap_err();
    let expected = format!("document shutdown failed: {error}");
    assert_eq!(
        f.document.lifecycle(),
        DocumentLifecycle::Failed(error.clone())
    );
    rejected(&f, &expected);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    rejected(&f, &expected);
    assert_eq!(f.document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(path).unwrap().count(), 0);
}
