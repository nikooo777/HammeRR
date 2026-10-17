use super::*;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

fn attach(session: &mut Session, authority: &crate::path_claims::Authority) {
    session.use_application_path_claims(authority.clone());
}

fn empty(authority: &crate::path_claims::Authority) -> Session {
    let mut session = Session::new(Ok(GameFs {
        game_dir: PathBuf::new(),
        base_dir: PathBuf::new(),
        mounts: Vec::new(),
    }));
    attach(&mut session, authority);
    session
}

#[test]
fn source_aliases_reject_save_and_compile_before_backup_or_job_changes() {
    let root = tempfile::tempdir().unwrap();
    let authority = crate::path_claims::Authority::default();
    let mut owner = empty(&authority);
    let mut peer = empty(&authority);
    peer.configure_recovery(Ok(root.path().join("peer-recovery")));
    let source = root.path().join("source.bsp");
    owner.save(Actor::User, Some(&source)).unwrap();
    let hard = root.path().join("hard.bsp");
    let link = root.path().join("link.bsp");
    let parent = root.path().join("parent");
    fs::hard_link(&source, &hard).unwrap();
    symlink(&source, &link).unwrap();
    symlink(root.path(), &parent).unwrap();
    owner.open(Actor::User, &link, true).unwrap();
    let aliases = [source.clone(), hard, link, parent.join("source.bsp")];
    let history = peer.history_snapshot();
    let status = peer.compile_status();
    let source_bytes = fs::read(&source).unwrap();
    for path in &aliases {
        assert!(
            peer.save(Actor::Agent, Some(path))
                .unwrap_err()
                .contains("conflicts")
        );
        assert!(
            peer.begin_compile(Actor::Agent, Some(path))
                .err()
                .unwrap()
                .contains("conflicts")
        );
        assert!(
            peer.open(Actor::Agent, path, true)
                .unwrap_err()
                .contains("conflicts")
        );
    }
    assert_eq!(peer.history_snapshot(), history);
    assert_eq!(peer.compile_status(), status);
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    assert!(peer.recovery_entries().unwrap().entries.is_empty());
    let inode = fs::metadata(&source).unwrap().ino();
    owner.save(Actor::User, None).unwrap();
    assert_ne!(fs::metadata(&source).unwrap().ino(), inode);
    assert!(peer.save(Actor::Agent, Some(&source)).is_err());
}

#[test]
fn failed_source_transitions_preserve_old_claim_and_release_only_candidate() {
    let root = tempfile::tempdir().unwrap();
    let authority = crate::path_claims::Authority::default();
    let mut owner = empty(&authority);
    let mut peer = empty(&authority);
    let source = root.path().join("source.vmf");
    owner.save(Actor::User, Some(&source)).unwrap();
    let history = owner.history_snapshot();
    let original = fs::read(&source).unwrap();
    let broken = root.path().join("broken.vmf");
    fs::write(&broken, "world { broken").unwrap();
    assert!(owner.open(Actor::User, &broken, true).is_err());
    let absent = root.path().join("absent/new.vmf");
    assert!(owner.save(Actor::User, Some(&absent)).is_err());
    assert_eq!(owner.path(), Some(source.as_path()));
    assert_eq!(owner.history_snapshot(), history);
    assert_eq!(fs::read(&source).unwrap(), original);
    assert!(peer.save(Actor::Agent, Some(&source)).is_err());
    peer.save(Actor::Agent, Some(&broken)).unwrap();
    fs::create_dir(absent.parent().unwrap()).unwrap();
    peer.save(Actor::Agent, Some(&absent)).unwrap();
    owner.new_map(Actor::User, true).unwrap();
    peer.save(Actor::Agent, Some(&source)).unwrap();
}

#[test]
fn derived_pointfile_reservation_cannot_overwrite_a_source_of_either_owner() {
    let root = tempfile::tempdir().unwrap();
    let authority = crate::path_claims::Authority::default();
    let mut owner = empty(&authority);
    let mut peer = empty(&authority);
    let source = root.path().join("source.lin");
    owner.save(Actor::User, Some(&source)).unwrap();
    let bsp = source.with_extension("bsp");
    let bytes = fs::read(&source).unwrap();
    assert!(owner.begin_compile(Actor::User, Some(&bsp)).is_err());
    assert!(peer.begin_compile(Actor::Agent, Some(&bsp)).is_err());
    assert!(owner.compile_token().is_none());
    assert!(peer.compile_token().is_none());
    assert_eq!(fs::read(&source).unwrap(), bytes);
}

#[test]
fn save_keeps_source_symlink_mode_and_referring_instance_directory() {
    let root = tempfile::tempdir().unwrap();
    let referring = root.path().join("referring");
    let physical = root.path().join("physical");
    fs::create_dir(&referring).unwrap();
    fs::create_dir(&physical).unwrap();
    let target = physical.join("parent.vmf");
    let alias = referring.join("parent.vmf");
    let text = Document::empty().vmf().write(&WriteOptions::default());
    fs::write(&target, &text).unwrap();
    fs::write(referring.join("child.vmf"), &text).unwrap();
    fs::write(physical.join("child.vmf"), b"wrong source directory").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    symlink(&target, &alias).unwrap();
    let authority = crate::path_claims::Authority::default();
    let mut owner = empty(&authority);
    let mut peer = empty(&authority);
    owner.open(Actor::User, &alias, true).unwrap();
    let location = compiler::instances::SourceLocation::capture(owner.path().unwrap()).unwrap();
    assert_eq!(
        location.reference("child.vmf").unwrap().path(),
        referring.join("child.vmf")
    );
    owner.save(Actor::User, None).unwrap();
    assert!(fs::symlink_metadata(&alias).unwrap().is_symlink());
    assert_eq!(owner.path(), Some(alias.as_path()));
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o640);
    assert!(peer.save(Actor::Agent, Some(&target)).is_err());
    fs::set_permissions(&target, fs::Permissions::from_mode(0o200)).unwrap();
    owner.save(Actor::User, Some(&alias)).unwrap();
    assert!(fs::symlink_metadata(&alias).unwrap().is_symlink());
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o200);
    let request = peer
        .begin_compile(Actor::Agent, Some(&referring.join("parent.bsp")))
        .unwrap();
    drop(request);
    peer.finish_compile(Actor::Agent, &Err("abandoned request".into()));
}

#[test]
fn retained_source_rejects_parent_retarget_without_backup_or_replacement_writes() {
    let root = tempfile::tempdir().unwrap();
    let original = root.path().join("original");
    let replacement = root.path().join("replacement");
    fs::create_dir(&original).unwrap();
    fs::create_dir(&replacement).unwrap();
    let alias = root.path().join("alias");
    symlink(&original, &alias).unwrap();
    let authority = crate::path_claims::Authority::default();
    let mut owner = empty(&authority);
    owner.configure_recovery(Ok(root.path().join("recovery")));
    owner
        .save(Actor::User, Some(&alias.join("parent.vmf")))
        .unwrap();
    let bytes = fs::read(original.join("parent.vmf")).unwrap();
    fs::write(replacement.join("parent.vmf"), b"replacement").unwrap();
    fs::remove_file(&alias).unwrap();
    symlink(&replacement, &alias).unwrap();
    let history = owner.history_snapshot();
    assert!(owner.save(Actor::User, None).is_err());
    assert_eq!(owner.history_snapshot(), history);
    assert_eq!(fs::read(original.join("parent.vmf")).unwrap(), bytes);
    assert_eq!(
        fs::read(replacement.join("parent.vmf")).unwrap(),
        b"replacement"
    );
    assert!(owner.recovery_entries().unwrap().entries.is_empty());
    let mut peer = empty(&authority);
    assert!(
        peer.save(Actor::Agent, Some(&original.join("parent.vmf")))
            .is_err()
    );
    owner
        .save(Actor::User, Some(&alias.join("parent.vmf")))
        .unwrap();
    assert_eq!(fs::read(original.join("parent.vmf")).unwrap(), bytes);
    assert_ne!(
        fs::read(replacement.join("parent.vmf")).unwrap(),
        b"replacement"
    );
    assert!(
        peer.save(Actor::Agent, Some(&replacement.join("parent.vmf")))
            .is_err()
    );
    peer.save(Actor::Agent, Some(&original.join("parent.vmf")))
        .unwrap();
}

#[test]
fn pending_plain_output_claims_cover_both_destinations_and_retire_on_drop() {
    let root = tempfile::tempdir().unwrap();
    let authority = crate::path_claims::Authority::default();
    let mut owner = empty(&authority);
    let mut peer = empty(&authority);
    let bsp = root.path().join("pending.bsp");
    let pointfile = bsp.with_extension("lin");
    let request = owner.begin_compile(Actor::User, Some(&bsp)).unwrap();
    assert!(peer.save(Actor::Agent, Some(&bsp)).is_err());
    assert!(peer.save(Actor::Agent, Some(&pointfile)).is_err());
    drop(request);
    owner.finish_compile(Actor::User, &Err("abandoned request".into()));
    peer.save(Actor::Agent, Some(&bsp)).unwrap();
    peer.save(Actor::Agent, Some(&pointfile)).unwrap();
}

#[test]
fn actual_plain_bsp_and_lin_claims_survive_later_failure_and_cancellation() {
    let (root, game, _) = crate::test_assets::game_process(false);
    let authority = crate::path_claims::Authority::default();
    let mut owner = Session::new(Ok(game));
    attach(&mut owner, &authority);
    let mut peer = empty(&authority);
    owner.replace_document(Document::new(crate::test_assets::room()), None);
    let source = root.path().join("room.vmf");
    owner.save(Actor::User, Some(&source)).unwrap();
    let bsp = root.path().join("successful.bsp");
    let result = owner.begin_compile(Actor::User, Some(&bsp)).unwrap().run();
    assert!(result.as_ref().unwrap().published());
    owner.finish_compile(Actor::User, &result);
    let bsp_result = result;
    let bsp_bytes = fs::read(&bsp).unwrap();
    let wall = owner.document().vmf().world.solids.last().unwrap().id;
    owner
        .delete_objects(Actor::User, &[ObjectId::Solid(wall)])
        .unwrap();
    let leaking = root.path().join("leaking.bsp");
    let result = owner
        .begin_compile(Actor::User, Some(&leaking))
        .unwrap()
        .run();
    assert!(result.as_ref().unwrap().output.leak.is_some());
    owner.finish_compile(Actor::User, &result);
    let pointfile_result = result;
    let pointfile = leaking.with_extension("lin");
    let pointfile_bytes = fs::read(&pointfile).unwrap();
    let status = owner.compile_status();
    assert!(owner.begin_compile(Actor::User, Some(&pointfile)).is_err());
    let paired = root.path().join("paired.bsp");
    fs::write(&paired, b"unclaimed output").unwrap();
    fs::hard_link(&paired, paired.with_extension("lin")).unwrap();
    assert!(owner.begin_compile(Actor::User, Some(&paired)).is_err());
    let lin_on_bsp = root.path().join("lin-on-bsp.bsp");
    fs::hard_link(&bsp, lin_on_bsp.with_extension("lin")).unwrap();
    assert!(owner.begin_compile(Actor::User, Some(&lin_on_bsp)).is_err());
    let bsp_on_lin = root.path().join("bsp-on-lin.bsp");
    fs::hard_link(&pointfile, &bsp_on_lin).unwrap();
    assert!(owner.begin_compile(Actor::User, Some(&bsp_on_lin)).is_err());
    assert_eq!(owner.compile_status(), status);
    assert_eq!(fs::read(&bsp).unwrap(), bsp_bytes);
    assert_eq!(fs::read(&pointfile).unwrap(), pointfile_bytes);
    let same_role = owner.begin_compile(Actor::User, Some(&bsp)).unwrap();
    drop(same_role);
    assert!(!owner.compiling());
    assert_eq!(owner.compile_status()["job"]["status"], "failed");
    assert!(peer.save(Actor::Agent, Some(&bsp)).is_err());
    assert!(peer.save(Actor::Agent, Some(&pointfile)).is_err());
    assert_eq!(fs::read(&bsp).unwrap(), bsp_bytes);
    assert_eq!(fs::read(&pointfile).unwrap(), pointfile_bytes);
    owner.finish_compile(Actor::User, &Err("abandoned same-role rebuild".into()));
    let cancelled = root.path().join("cancelled.bsp");
    let request = owner.begin_compile(Actor::User, Some(&cancelled)).unwrap();
    owner
        .cancel_compile(Actor::User, owner.compile_token().unwrap())
        .unwrap();
    let result = request.run();
    assert!(result.is_err());
    owner.finish_compile(Actor::User, &result);
    owner.finish_compile(Actor::User, &Err("injected later worker failure".into()));
    assert!(peer.save(Actor::Agent, Some(&bsp)).is_err());
    assert!(peer.save(Actor::Agent, Some(&pointfile)).is_err());
    assert!(peer.begin_compile(Actor::Agent, Some(&bsp)).is_err());
    let alias = root.path().join("pointfile-alias.bsp");
    fs::hard_link(&pointfile, &alias).unwrap();
    assert!(peer.begin_compile(Actor::Agent, Some(&alias)).is_err());
    assert_eq!(fs::read(&bsp).unwrap(), bsp_bytes);
    assert_eq!(fs::read(&pointfile).unwrap(), pointfile_bytes);
    peer.save(Actor::Agent, Some(&cancelled)).unwrap();
    owner.new_map(Actor::User, true).unwrap();
    peer.save(Actor::Agent, Some(&bsp)).unwrap();
    peer.save(Actor::Agent, Some(&pointfile)).unwrap();
    assert!(bsp_result.as_ref().unwrap().published());
    assert!(pointfile_result.as_ref().unwrap().output.leak.is_some());
}

#[test]
fn ordinary_output_replaces_an_unowned_symlink_entry_without_writing_its_source_target() {
    let (root, game, _) = crate::test_assets::game_process(false);
    let authority = crate::path_claims::Authority::default();
    let mut source_owner = empty(&authority);
    let source = root.path().join("source.vmf");
    source_owner.save(Actor::User, Some(&source)).unwrap();
    let bytes = fs::read(&source).unwrap();
    let output = root.path().join("entry.bsp");
    symlink(&source, &output).unwrap();
    let mut compiler = Session::new(Ok(game));
    attach(&mut compiler, &authority);
    compiler.replace_document(Document::new(crate::test_assets::room()), None);
    let result = compiler
        .begin_compile(Actor::Agent, Some(&output))
        .unwrap()
        .run();
    assert!(result.as_ref().unwrap().published());
    compiler.finish_compile(Actor::Agent, &result);
    assert!(!fs::symlink_metadata(&output).unwrap().is_symlink());
    assert_eq!(fs::read(&source).unwrap(), bytes);
    assert!(source_formats::bsp::Bsp::parse(&fs::read(&output).unwrap()).is_ok());
}

#[test]
fn guarded_instance_publication_retains_claims_after_actual_commit() {
    let (_root, game, source, _child) = crate::test_assets::instance_map(false);
    let authority = crate::path_claims::Authority::default();
    let mut owner = Session::new(Ok(game));
    attach(&mut owner, &authority);
    let mut peer = empty(&authority);
    owner.open(Actor::User, &source, true).unwrap();
    let result = owner.begin_compile(Actor::User, None).unwrap().run();
    assert!(!result.as_ref().unwrap().published());
    let status = peer.compile_status();
    assert!(
        peer.begin_compile(Actor::Agent, Some(&source.with_extension("bsp")))
            .is_err()
    );
    assert_eq!(peer.compile_status(), status);
    owner.finish_compile(Actor::User, &result);
    assert!(result.as_ref().unwrap().published());
    drop(result);
    assert!(
        peer.save(Actor::Agent, Some(&source.with_extension("bsp")))
            .is_err()
    );
    let request = owner
        .begin_compile(Actor::User, Some(&source.with_file_name("abandoned.bsp")))
        .unwrap();
    drop(request);
    owner.finish_compile(Actor::User, &Err("abandoned request".into()));
    assert!(
        peer.save(Actor::Agent, Some(&source.with_extension("bsp")))
            .is_err()
    );
    peer.save(Actor::Agent, Some(&source.with_file_name("abandoned.bsp")))
        .unwrap();
}
