use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use crate::session::SelectMode;
use slint::Model;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug)]
enum Family {
    Primitive,
    Polygon,
    Repeat,
}

const FAMILIES: [Family; 3] = [Family::Primitive, Family::Polygon, Family::Repeat];

struct Fixture {
    directory: tempfile::TempDir,
    document: DocumentHandle,
    access: DocumentAccess,
    shared: SharedSession,
    primitive: PrimitiveWindow,
    polygon: crate::polygon_ui::Controller,
    repeat: crate::repeat_ui::Controller,
    source: map_core::ObjectId,
    family: Family,
}

impl Fixture {
    fn new(family: Family) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("No assets needed".into()));
        session.configure_recovery(Ok(directory.path().join("recovery")));
        let source = session
            .create_block(Actor::User, DVec3::ZERO, DVec3::splat(64.0), "test", false)
            .unwrap();
        session.select(&[source], SelectMode::Replace).unwrap();
        let shared = Arc::new(Mutex::new(session));
        let document = crate::test_assets::document(shared.clone());
        let access = DocumentAccess::for_document(document.clone());
        let primitive = PrimitiveWindow::new().unwrap();
        bind_with_access(&primitive, &access);
        let polygon = crate::polygon_ui::Controller::new_with_access(&access).unwrap();
        let repeat = crate::repeat_ui::Controller::new_with_access(&access).unwrap();
        let fixture = Self {
            directory,
            document,
            access,
            shared,
            primitive,
            polygon,
            repeat,
            source,
            family,
        };
        fixture.initialize();
        fixture
    }

    fn initialize(&self) {
        let mut session = lock(&self.shared);
        match self.family {
            Family::Primitive => initialize(&self.primitive, &mut session, "test"),
            Family::Polygon => {
                self.polygon.initialize(&mut session, "test").unwrap();
                self.polygon.window.set_center_u("0".into());
                self.polygon.window.set_center_v("0".into());
                self.polygon.window.set_span("256".into());
                drop(session);
                for (x, y) in [(0.25, 0.75), (0.75, 0.75), (0.75, 0.25), (0.25, 0.25)] {
                    self.polygon.window.invoke_draw_corner(x, y);
                }
            }
            Family::Repeat => self.repeat.initialize(&session).unwrap(),
        }
    }

    fn window(&self) -> &slint::Window {
        match self.family {
            Family::Primitive => self.primitive.window(),
            Family::Polygon => self.polygon.window.window(),
            Family::Repeat => self.repeat.window.window(),
        }
    }

    fn message(&self) -> slint::SharedString {
        match self.family {
            Family::Primitive => self.primitive.get_message(),
            Family::Polygon => self.polygon.window.get_message(),
            Family::Repeat => self.repeat.window.get_message(),
        }
    }

    fn preview(&self) {
        match self.family {
            Family::Primitive => self.primitive.invoke_preview_shape(),
            Family::Polygon => self.polygon.window.invoke_preview(),
            Family::Repeat => self.repeat.window.invoke_preview(),
        }
    }

    fn apply(&self) {
        match self.family {
            Family::Primitive => self.primitive.invoke_apply(),
            Family::Polygon => self.polygon.window.invoke_apply(),
            Family::Repeat => self.repeat.window.invoke_apply(),
        }
    }

    fn cancel(&self) {
        match self.family {
            Family::Primitive => self.primitive.invoke_cancel(),
            Family::Polygon => self.polygon.window.invoke_cancel(),
            Family::Repeat => self.repeat.window.invoke_cancel(),
        }
    }

    fn form(&self) -> serde_json::Value {
        match self.family {
            Family::Primitive => serde_json::json!({
                "token": self.primitive.get_token().to_string(),
                "summary": self.primitive.get_summary().to_string(),
                "bounds": self.primitive.get_actual_bounds().to_string(),
                "can_preview": self.primitive.get_can_preview(),
                "material": self.primitive.get_material().to_string(),
                "feedback": self.primitive.get_feedback().to_string(),
            }),
            Family::Polygon => serde_json::json!({
                "corners": self.polygon.window.get_corners().iter().map(|row| (
                    row.u.to_string(), row.v.to_string(), row.x, row.y
                )).collect::<Vec<_>>(),
                "outline": self.polygon.window.get_outline().to_string(),
                "ready": self.polygon.window.get_ready(),
                "summary": self.polygon.window.get_summary().to_string(),
                "material": self.polygon.window.get_material().to_string(),
            }),
            Family::Repeat => serde_json::json!({
                "ready": self.repeat.window.get_ready(),
                "summary": self.repeat.window.get_summary().to_string(),
                "selection": self.repeat.window.get_selection().to_string(),
                "count": self.repeat.window.get_count(),
            }),
        }
    }

    fn rejected_actions(&self, error: &str) {
        macro_rules! rejected {
            ($action:expr) => {{
                $action;
                assert_eq!(self.message(), error, "{:?}", self.family);
            }};
        }
        match self.family {
            Family::Primitive => {
                rejected!(self.primitive.invoke_edited());
                rejected!(self.primitive.invoke_focus_preview());
            }
            Family::Polygon => {
                rejected!(self.polygon.window.invoke_edited());
                rejected!(self.polygon.window.invoke_view_edited());
                rejected!(self.polygon.window.invoke_draw_corner(0.5, 0.5));
                rejected!(self.polygon.window.invoke_add_corner());
                rejected!(
                    self.polygon
                        .window
                        .invoke_change_corner(0, "8".into(), "8".into())
                );
                rejected!(self.polygon.window.invoke_remove_corner(0));
                rejected!(self.polygon.window.invoke_undo_corner());
                rejected!(self.polygon.window.invoke_clear_corners());
                rejected!(self.polygon.window.invoke_frame());
            }
            Family::Repeat => rejected!(self.repeat.window.invoke_invalidate()),
        }
        rejected!(self.preview());
        rejected!(self.apply());
        self.window().show().unwrap();
        rejected!(self.cancel());
        assert!(!self.window().is_visible());
        self.window().show().unwrap();
        rejected!(
            self.window()
                .dispatch_event(slint::platform::WindowEvent::CloseRequested)
        );
        assert!(!self.window().is_visible());
    }
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
fn managed_geometry_callbacks_preserve_reviews_and_peers_through_real_drain() {
    backend();
    for family in FAMILIES {
        let target = Fixture::new(family);
        let peer = Fixture::new(family);
        let original = lock(&target.shared).document().vmf().clone();
        target.preview();
        let draft = lock(&target.shared).render_document().vmf().clone();
        assert_ne!(draft, original, "{:?}: {}", family, target.message());
        target.apply();
        assert_eq!(lock(&target.shared).document().vmf(), &draft);
        lock(&target.shared).undo(Actor::User).unwrap();
        lock(&target.shared)
            .select(&[target.source], SelectMode::Replace)
            .unwrap();
        target.initialize();
        target.preview();
        target.cancel();
        assert_eq!(lock(&target.shared).render_document().vmf(), &original);
        target.initialize();
        target.preview();
        let expected_form = target.form();
        let history = lock(&target.shared).history_snapshot();
        let camera = lock(&target.shared).camera();
        let render = lock(&target.shared).render_document().vmf().clone();
        let peer_source = lock(&peer.shared).document().vmf().clone();
        let peer_form = peer.form();
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
        target.rejected_actions("the editor is closing");
        assert_eq!(target.form(), expected_form);
        assert_eq!(lock(&target.shared).render_document().vmf(), &render);
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
        target.rejected_actions("the document is closed");
        assert_eq!(target.form(), expected_form);
        assert_eq!(lock(&target.shared).document().vmf(), &original);
        assert_eq!(lock(&target.shared).render_document().vmf(), &render);
        assert_eq!(lock(&target.shared).history_snapshot(), history);
        assert_eq!(lock(&target.shared).camera(), camera);
        assert_eq!(store.list().unwrap().entries.len(), 1);
        assert_eq!(store.read(&entries[0].id).unwrap().1, recovered);
        assert_eq!(lock(&peer.shared).document().vmf(), &peer_source);
        assert_eq!(peer.form(), peer_form);
        peer.preview();
        let peer_draft = lock(&peer.shared).render_document().vmf().clone();
        peer.apply();
        assert_eq!(lock(&peer.shared).document().vmf(), &peer_draft);
        assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
        let weak = Arc::downgrade(&target.shared);
        target.window().hide().unwrap();
        let Fixture {
            directory: _directory,
            document,
            access,
            shared,
            primitive,
            polygon,
            repeat,
            source: _,
            family,
        } = target;
        drop(access);
        drop(shared);
        drop(document);
        assert!(weak.upgrade().is_none());
        match family {
            Family::Primitive => {
                primitive.invoke_preview_shape();
                assert_eq!(primitive.get_message(), "the document is closed");
            }
            Family::Polygon => {
                polygon.window.invoke_preview();
                assert_eq!(polygon.window.get_message(), "the document is closed");
            }
            Family::Repeat => {
                repeat.window.invoke_preview();
                assert_eq!(repeat.window.get_message(), "the document is closed");
            }
        }
    }
}

#[test]
fn managed_geometry_failed_shutdown_keeps_forms_and_memoizes_recovery_failure() {
    backend();
    for family in FAMILIES {
        let target = Fixture::new(family);
        target.preview();
        let expected = target.form();
        let render = lock(&target.shared).render_document().vmf().clone();
        let recovery = target.directory.path().join("recovery");
        if recovery.exists() {
            std::fs::remove_dir_all(&recovery).unwrap();
        }
        std::fs::write(&recovery, b"blocked").unwrap();
        let error = target.document.shutdown().unwrap_err();
        target.rejected_actions(&format!("document shutdown failed: {error}"));
        assert_eq!(target.form(), expected);
        assert_eq!(lock(&target.shared).render_document().vmf(), &render);
        std::fs::remove_file(&recovery).unwrap();
        std::fs::create_dir(&recovery).unwrap();
        assert_eq!(target.document.shutdown().unwrap_err(), error);
        assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
    }
}

#[test]
fn stale_geometry_reviews_cannot_apply_or_cancel_foreign_previews() {
    backend();
    for family in FAMILIES {
        let target = Fixture::new(family);
        target.preview();
        {
            let mut session = lock(&target.shared);
            session
                .preview_transform(
                    Actor::Agent,
                    &[target.source],
                    map_core::edit::Transform {
                        translation: DVec3::Z * 20.0,
                        ..Default::default()
                    },
                    None,
                )
                .unwrap();
        }
        let original = lock(&target.shared).document().vmf().clone();
        let foreign = lock(&target.shared).render_document().vmf().clone();
        target.apply();
        assert_eq!(lock(&target.shared).document().vmf(), &original);
        assert_eq!(lock(&target.shared).render_document().vmf(), &foreign);
        target.window().show().unwrap();
        target.cancel();
        assert!(!target.window().is_visible());
        assert_eq!(lock(&target.shared).render_document().vmf(), &foreign);
    }
}

#[test]
fn admitted_primitive_form_action_finishes_before_final_recovery() {
    backend();
    let target = Fixture::new(Family::Primitive);
    target.preview();
    let original = lock(&target.shared).document().vmf().clone();
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
    target.primitive.invoke_edited();
    assert!(target.primitive.get_token().is_empty());
    assert_eq!(lock(&target.shared).render_document().vmf(), &original);
    completed.borrow().recv_timeout(TIMEOUT).unwrap().unwrap();
    joining.borrow_mut().take().unwrap().join().unwrap();
    assert_eq!(target.document.lifecycle(), DocumentLifecycle::Finished);
}
