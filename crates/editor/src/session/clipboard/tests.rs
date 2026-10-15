use super::*;
use glam::DVec3;

#[derive(Default)]
pub(crate) struct Memory {
    pub text: String,
    pub fail: bool,
}

pub(crate) struct MemoryTransport(pub Arc<std::sync::Mutex<Memory>>);
impl Transport for MemoryTransport {
    fn read(&mut self) -> Result<String, String> {
        let state = self.0.lock().unwrap();
        if state.fail {
            Err("Native read failed".into())
        } else {
            Ok(state.text.clone())
        }
    }
    fn write(&mut self, text: &str) -> Result<(), String> {
        let mut state = self.0.lock().unwrap();
        if state.fail {
            return Err("Native publication failed".into());
        }
        state.text = text.into();
        Ok(())
    }
}

pub(crate) fn fixture() -> (Session, ObjectId, Arc<std::sync::Mutex<Memory>>) {
    let mut session = Session::new(Err("No game needed".into()));
    let id = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(32.0),
            "brick/test",
            false,
        )
        .unwrap();
    session.select(&[id], SelectMode::Replace).unwrap();
    let memory = Arc::new(std::sync::Mutex::new(Memory::default()));
    session.clipboard.transport = Box::new(MemoryTransport(memory.clone()));
    (session, id, memory)
}

fn capture(session: &mut Session, id: ObjectId, cut: bool, system: bool) -> Result<Value, String> {
    session.capture_clipboard(Actor::Agent, &[id], &session.clipboard_stamp(), cut, system)
}
fn preview(session: &mut Session, options: transfer::Options) -> u64 {
    session
        .preview_paste(options, &session.clipboard_stamp())
        .unwrap()["preview"]["token"]
        .as_u64()
        .unwrap()
}

#[test]
fn detached_copy_and_paste_share_ownership_undo_and_cross_map_roundtrip() {
    for actor in [Actor::User, Actor::Agent] {
        let (mut session, id, _) = fixture();
        let before = session.document.vmf().clone();
        let camera = session.camera();
        let history = session.document.undo_labels().count();
        capture(&mut session, id, false, false).unwrap();
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.document.undo_labels().count(), history);
        assert_eq!(camera, session.camera());
        let exported = session
            .export_clipboard(session.clipboard.content, false)
            .unwrap()["text"]
            .as_str()
            .unwrap()
            .to_string();
        session.new_map(actor, true).unwrap();
        let empty = session.document.vmf().clone();
        let token = preview(
            &mut session,
            transfer::Options {
                offset: DVec3::X * 64.0,
                ..Default::default()
            },
        );
        assert_eq!(session.document.vmf(), &empty);
        assert_eq!(session.document.undo_labels().count(), 0);
        let expected = session.render_document().vmf().clone();
        let result = session.apply_paste(actor, token).unwrap();
        assert_eq!(session.document.vmf(), &expected);
        assert_eq!(result["objects"].as_array().unwrap().len(), 1);
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &empty);
        session.redo(actor).unwrap();
        assert_eq!(session.document.vmf(), &expected);
        let mut destination = Session::new(Err("No game".into()));
        destination
            .import_clipboard(actor, Some(&exported), &destination.clipboard_stamp())
            .unwrap();
        let token = preview(&mut destination, transfer::Options::default());
        destination.apply_paste(actor, token).unwrap();
        assert_eq!(destination.document.vmf().world.solids.len(), 1);
    }
}

#[test]
fn native_publication_failure_never_cuts_and_bad_import_keeps_valid_draft() {
    let (mut session, id, memory) = fixture();
    capture(&mut session, id, false, true).unwrap();
    let original = memory.lock().unwrap().text.clone();
    let report = session.clipboard_report();
    let token = preview(&mut session, transfer::Options::default());
    let draft = session.render_document().vmf().clone();
    let before = session.document.vmf().clone();
    let history = session.document.undo_labels().count();
    memory.lock().unwrap().fail = true;
    assert!(capture(&mut session, id, true, true).is_err());
    assert_eq!(session.document.vmf(), &before);
    assert_eq!(session.document.undo_labels().count(), history);
    assert_eq!(memory.lock().unwrap().text, original);
    assert_eq!(session.render_document().vmf(), &draft);
    assert!(
        session
            .import_clipboard(Actor::Agent, Some("world {"), &session.clipboard_stamp())
            .is_err()
    );
    assert_eq!(session.render_document().vmf(), &draft);
    assert!(
        session
            .import_clipboard(Actor::Agent, Some(&original), "stale")
            .is_err()
    );
    session.cancel_paste(token).unwrap();
    assert_eq!(session.clipboard_report()["payload"], report["payload"]);
    memory.lock().unwrap().fail = false;
    capture(&mut session, id, true, true).unwrap();
    assert!(session.document.vmf().world.solids.is_empty());
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document.vmf(), &before);
}

#[test]
fn preview_replacements_selection_definition_and_clipboard_changes_guard_apply_cancel() {
    let (mut session, id, _) = fixture();
    capture(&mut session, id, false, false).unwrap();
    let old = preview(&mut session, transfer::Options::default());
    let new = preview(
        &mut session,
        transfer::Options {
            offset: DVec3::X * 16.0,
            ..Default::default()
        },
    );
    let before = session.render_document().vmf().clone();
    assert!(session.cancel_paste(old).is_err());
    assert!(session.apply_paste(Actor::Agent, old).is_err());
    assert_eq!(session.render_document().vmf(), &before);
    session
        .preview_transform(
            Actor::Agent,
            &[id],
            edit::Transform {
                translation: DVec3::Z * 10.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let foreign = session.render_document().vmf().clone();
    assert!(session.cancel_paste(new).is_err());
    assert_eq!(session.render_document().vmf(), &foreign);
    session.cancel_transform_preview();
    let token = preview(&mut session, transfer::Options::default());
    session.definitions_serial += 1;
    assert!(session.apply_paste(Actor::Agent, token).is_err());
    session.cancel_paste(token).unwrap();
    let token = preview(&mut session, transfer::Options::default());
    capture(&mut session, id, false, false).unwrap();
    assert!(session.apply_paste(Actor::Agent, token).is_err());
    let token = preview(&mut session, transfer::Options::default());
    session.select(&[], SelectMode::Replace).unwrap();
    assert!(session.apply_paste(Actor::Agent, token).is_err());
}

#[test]
fn same_map_cut_names_restore_only_before_any_map_edit_or_imported_receipt() {
    let (mut session, brush, _) = fixture();
    let id = session
        .edit(Actor::User, "named", |tx| {
            edit::create_entity(
                tx,
                "info_target",
                Some(DVec3::ZERO),
                &[("targetname".into(), "original".into())],
                Vec::new(),
            )
        })
        .unwrap();
    capture(&mut session, id, true, false).unwrap();
    assert!(session.can_restore_cut_names());
    let token = preview(
        &mut session,
        transfer::Options {
            restore_names: true,
            ..Default::default()
        },
    );
    let result = session.apply_paste(Actor::User, token).unwrap();
    assert_eq!(result["names"][0]["after"], "original");
    assert!(!session.can_restore_cut_names());
    let ObjectId::Entity(id) = session.selected_ids()[0] else {
        panic!()
    };
    capture(&mut session, ObjectId::Entity(id), true, false).unwrap();
    session
        .edit(Actor::Agent, "edit elsewhere", |tx| {
            edit::translate(tx, &[brush], DVec3::X, true)
        })
        .unwrap();
    assert!(!session.can_restore_cut_names());
    assert!(
        session
            .preview_paste(
                transfer::Options {
                    restore_names: true,
                    ..Default::default()
                },
                &session.clipboard_stamp()
            )
            .is_err()
    );
    let text = session
        .export_clipboard(session.clipboard.content, false)
        .unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string();
    let mut other = Session::new(Err("No game".into()));
    other
        .import_clipboard(Actor::Agent, Some(&text), &other.clipboard_stamp())
        .unwrap();
    assert!(!other.can_restore_cut_names());
}
