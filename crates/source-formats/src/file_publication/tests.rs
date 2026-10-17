use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

fn retarget(alias: &Path, target: &Path) {
    fs::remove_file(alias).unwrap();
    symlink(target, alias).unwrap();
}

fn stage(destination: &BoundDestination, bytes: &[u8]) -> StagedFile {
    let mut stage = destination.stage().unwrap();
    stage.write_all(bytes).unwrap();
    stage
}

fn aliased_directories() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let original = root.path().join("original");
    let replacement = root.path().join("replacement");
    let alias = root.path().join("alias");
    fs::create_dir(&original).unwrap();
    fs::create_dir(&replacement).unwrap();
    symlink(&original, &alias).unwrap();
    (root, original, replacement, alias)
}

#[test]
fn source_follows_final_symlink_preserves_mode_and_refreshes_published_identity() {
    let root = tempfile::tempdir().unwrap();
    let referring = root.path().join("referring");
    let target = root.path().join("target");
    fs::create_dir(&referring).unwrap();
    fs::create_dir(&target).unwrap();
    let path = target.join("actual.vmf");
    fs::write(&path, b"previous source").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let authored = referring.join("authored.vmf");
    symlink("../target/actual.vmf", &authored).unwrap();
    let destination = BoundDestination::capture(&authored, Policy::FollowSource).unwrap();
    let before = destination.existing.as_ref().unwrap().stamp.identity;
    assert_eq!(destination.authored_path(), authored);
    assert_eq!(destination.physical_path(), path);
    assert_eq!(
        destination.read_existing().unwrap().unwrap(),
        b"previous source"
    );
    assert_eq!(
        destination.read_existing().unwrap().unwrap(),
        b"previous source"
    );
    assert_ne!(destination.claim_slots()[0], destination.claim_slots()[1]);
    let receipt = stage(&destination, b"committed source").commit().unwrap();
    assert!(receipt.warning.is_none());
    assert!(
        fs::symlink_metadata(&authored)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(&authored).unwrap(),
        Path::new("../target/actual.vmf")
    );
    assert_eq!(fs::read(&path).unwrap(), b"committed source");
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o640);
    assert_ne!(
        receipt
            .destination
            .existing
            .as_ref()
            .unwrap()
            .stamp
            .identity,
        before
    );
    receipt.destination.verify().unwrap();
    let next = stage(&receipt.destination, b"next commit")
        .commit()
        .unwrap();
    assert!(next.warning.is_none());
    assert_eq!(
        next.destination.read_existing().unwrap().unwrap(),
        b"next commit"
    );
}

#[test]
fn ordinary_output_replaces_final_symlink_entry_without_writing_its_referent() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("source.vmf");
    let output = root.path().join("output.bsp");
    fs::write(&target, b"source remains").unwrap();
    symlink(&target, &output).unwrap();
    let destination = BoundDestination::capture(&output, Policy::ReplaceEntry).unwrap();
    let receipt = stage(&destination, b"compiled output").commit().unwrap();
    assert!(receipt.warning.is_none());
    assert!(fs::symlink_metadata(&output).unwrap().is_file());
    assert_eq!(fs::read(&output).unwrap(), b"compiled output");
    assert_eq!(fs::read(&target).unwrap(), b"source remains");
    receipt.destination.verify().unwrap();
}

#[test]
fn source_capture_reads_readonly_files_but_staging_and_invalid_targets_fail() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.vmf");
    fs::write(&path, b"readonly source").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o440)).unwrap();
    let source = BoundDestination::capture(&path, Policy::FollowSource).unwrap();
    assert_eq!(source.read_existing().unwrap().unwrap(), b"readonly source");
    assert!(source.stage().is_err());
    let dangling = root.path().join("dangling.vmf");
    symlink("missing.vmf", &dangling).unwrap();
    assert!(BoundDestination::capture(&dangling, Policy::FollowSource).is_err());
    assert!(BoundDestination::capture(root.path(), Policy::FollowSource).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"readonly source");
}

#[test]
fn source_publication_retains_write_only_file_permissions_without_requiring_a_reader() {
    use std::os::unix::fs::FileExt;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.vmf");
    fs::write(&path, b"old source").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o200)).unwrap();
    let readable = File::open(&path).is_ok();
    let destination = BoundDestination::capture(&path, Policy::FollowSource).unwrap();
    assert_eq!(destination.read_existing().is_ok(), readable);
    let receipt = stage(&destination, b"published").commit().unwrap();
    assert!(receipt.warning.is_none());
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o200);
    let mut bytes = [0; 9];
    receipt
        .destination
        .existing
        .as_ref()
        .unwrap()
        .file
        .read_exact_at(&mut bytes, 0)
        .unwrap();
    assert_eq!(&bytes, b"published");
}

#[test]
fn searchable_writable_parent_does_not_require_directory_read_permission() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("private");
    fs::create_dir(&parent).unwrap();
    let path = parent.join("source.vmf");
    fs::write(&path, b"old source").unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o300)).unwrap();
    let destination = BoundDestination::capture(&path, Policy::FollowSource).unwrap();
    assert_eq!(destination.read_existing().unwrap().unwrap(), b"old source");
    let receipt = stage(&destination, b"published").commit().unwrap();
    assert!(receipt.warning.is_none());
    assert_eq!(fs::read(&path).unwrap(), b"published");
    assert_eq!(fs::metadata(&parent).unwrap().mode() & 0o777, 0o300);
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn absent_parent_keeps_ancestor_suffix_without_creating_directories() {
    let (_root, original, replacement, alias) = aliased_directories();
    let destination =
        BoundDestination::capture(&alias.join("nested/source.vmf"), Policy::FollowSource).unwrap();
    assert_eq!(
        destination.physical_path(),
        original.join("nested/source.vmf")
    );
    assert_eq!(
        destination.claim_slots()[0].leaf,
        Path::new("nested/source.vmf")
    );
    assert!(destination.read_existing().unwrap().is_none());
    assert_eq!(
        destination.stage().unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    assert!(!original.join("nested").exists());
    symlink(&replacement, original.join("nested")).unwrap();
    assert!(destination.verify().is_err());
    assert!(destination.stage().is_err());
    assert!(!replacement.join("source.vmf").exists());
}

#[test]
fn absent_parent_that_appears_is_bound_to_its_opened_directory_before_staging() {
    let root = tempfile::tempdir().unwrap();
    let requested = root.path().join("nested/source.vmf");
    let destination = BoundDestination::capture(&requested, Policy::FollowSource).unwrap();
    fs::create_dir(root.path().join("nested")).unwrap();
    let after_creation = BoundDestination::capture(&requested, Policy::ReplaceEntry).unwrap();
    assert_eq!(destination.physical_path(), after_creation.physical_path());
    assert_ne!(destination.claim_slots(), after_creation.claim_slots());
    let receipt = stage(&destination, b"source").commit().unwrap();
    assert!(receipt.warning.is_none());
    assert_eq!(
        receipt.destination.claim_slots()[0].leaf,
        Path::new("source.vmf")
    );
    assert_eq!(
        receipt.destination.read_existing().unwrap().unwrap(),
        b"source"
    );
}

#[test]
fn retarget_before_commit_rejects_and_cleans_only_the_original_stage() {
    let (_root, original, replacement, alias) = aliased_directories();
    fs::write(original.join("source.vmf"), b"old original").unwrap();
    fs::write(replacement.join("source.vmf"), b"replacement source").unwrap();
    let destination =
        BoundDestination::capture(&alias.join("source.vmf"), Policy::FollowSource).unwrap();
    let stage = stage(&destination, b"uncommitted");
    let basename = stage.basename.clone();
    fs::write(replacement.join(&basename), b"unrelated stage name").unwrap();
    retarget(&alias, &replacement);
    assert!(stage.commit().is_err());
    assert!(!original.join(&basename).exists());
    assert_eq!(
        fs::read(original.join("source.vmf")).unwrap(),
        b"old original"
    );
    assert_eq!(
        fs::read(replacement.join("source.vmf")).unwrap(),
        b"replacement source"
    );
    assert_eq!(
        fs::read(replacement.join(basename)).unwrap(),
        b"unrelated stage name"
    );
}

#[test]
fn replacing_parent_directory_cannot_redirect_publication_or_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("parent");
    let detached = root.path().join("detached");
    fs::create_dir(&parent).unwrap();
    let destination =
        BoundDestination::capture(&parent.join("output.bsp"), Policy::ReplaceEntry).unwrap();
    let stage = stage(&destination, b"uncommitted");
    let basename = stage.basename.clone();
    fs::rename(&parent, &detached).unwrap();
    fs::create_dir(&parent).unwrap();
    fs::write(parent.join(&basename), b"unrelated").unwrap();
    assert!(stage.commit().is_err());
    assert!(!detached.join(&basename).exists());
    assert!(!parent.join("output.bsp").exists());
    assert_eq!(fs::read(parent.join(basename)).unwrap(), b"unrelated");
}

#[test]
fn retarget_after_last_verification_commits_to_bound_parent_and_returns_warning() {
    for after_commit in [false, true] {
        let (_root, original, replacement, alias) = aliased_directories();
        let destination =
            BoundDestination::capture(&alias.join("output.bsp"), Policy::ReplaceEntry).unwrap();
        let stage = stage(&destination, b"published");
        let basename = stage.basename.clone();
        fs::write(replacement.join(&basename), b"unrelated").unwrap();
        let receipt = stage
            .commit_with(
                || {
                    if !after_commit {
                        retarget(&alias, &replacement);
                    }
                },
                || {
                    if after_commit {
                        retarget(&alias, &replacement);
                    }
                },
            )
            .unwrap();
        assert!(
            receipt
                .warning
                .as_deref()
                .unwrap()
                .contains("File was published")
        );
        assert_eq!(fs::read(original.join("output.bsp")).unwrap(), b"published");
        assert!(!replacement.join("output.bsp").exists());
        assert_eq!(fs::read(replacement.join(&basename)).unwrap(), b"unrelated");
        assert!(!original.join(&basename).exists());
        let actual = FileIdentity::capture(&fs::metadata(original.join("output.bsp")).unwrap());
        assert!(receipt.destination.file_identities().contains(&actual));
        assert!(receipt.destination.verify().is_err());
        assert!(receipt.destination.stage().is_err());
        let retained = receipt
            .destination
            .existing
            .as_ref()
            .unwrap()
            .file
            .metadata()
            .unwrap();
        assert_eq!(FileIdentity::capture(&retained), actual);
    }
}

#[test]
fn displaced_stage_name_is_neither_published_nor_removed_on_failure_or_drop() {
    for commit in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("output.bsp");
        fs::write(&path, b"old output").unwrap();
        let destination = BoundDestination::capture(&path, Policy::ReplaceEntry).unwrap();
        let stage = stage(&destination, b"owned staging bytes");
        let staged = root.path().join(&stage.basename);
        let displaced = root.path().join("displaced");
        fs::rename(&staged, &displaced).unwrap();
        fs::write(&staged, b"unrelated replacement").unwrap();
        if commit {
            assert!(stage.commit().is_err());
        } else {
            drop(stage);
        }
        assert_eq!(fs::read(staged).unwrap(), b"unrelated replacement");
        assert_eq!(fs::read(displaced).unwrap(), b"owned staging bytes");
        assert_eq!(fs::read(path).unwrap(), b"old output");
    }
}

#[test]
fn stage_displacement_after_binding_verification_is_rejected_before_rename() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("output.bsp");
    fs::write(&path, b"old output").unwrap();
    let destination = BoundDestination::capture(&path, Policy::ReplaceEntry).unwrap();
    let stage = stage(&destination, b"owned staging bytes");
    let staged = root.path().join(&stage.basename);
    let displaced = root.path().join("displaced");
    assert!(
        stage
            .commit_with(
                || {
                    fs::rename(&staged, &displaced).unwrap();
                    fs::write(&staged, b"unrelated replacement").unwrap();
                },
                || {},
            )
            .is_err()
    );
    assert_eq!(fs::read(staged).unwrap(), b"unrelated replacement");
    assert_eq!(fs::read(displaced).unwrap(), b"owned staging bytes");
    assert_eq!(fs::read(path).unwrap(), b"old output");
}

#[test]
fn drop_after_parent_retarget_removes_only_its_bound_stage() {
    let (_root, original, replacement, alias) = aliased_directories();
    let destination =
        BoundDestination::capture(&alias.join("source.vmf"), Policy::FollowSource).unwrap();
    let stage = stage(&destination, b"unused");
    let basename = stage.basename.clone();
    fs::write(replacement.join(&basename), b"unrelated").unwrap();
    retarget(&alias, &replacement);
    drop(stage);
    assert!(!original.join(&basename).exists());
    assert_eq!(fs::read(replacement.join(basename)).unwrap(), b"unrelated");
}

#[test]
fn final_source_symlink_retarget_rejects_before_writing_either_target() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first.vmf");
    let second = root.path().join("second.vmf");
    let alias = root.path().join("source.vmf");
    fs::write(&first, b"first").unwrap();
    fs::write(&second, b"second").unwrap();
    symlink(&first, &alias).unwrap();
    let destination = BoundDestination::capture(&alias, Policy::FollowSource).unwrap();
    let stage = stage(&destination, b"uncommitted");
    retarget(&alias, &second);
    assert!(stage.commit().is_err());
    assert_eq!(fs::read(first).unwrap(), b"first");
    assert_eq!(fs::read(second).unwrap(), b"second");
}

#[test]
fn final_source_link_retarget_after_verification_keeps_the_published_referent_claimed() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first.vmf");
    let second = root.path().join("second.vmf");
    let alias = root.path().join("source.vmf");
    fs::write(&first, b"first").unwrap();
    fs::write(&second, b"second").unwrap();
    symlink(&first, &alias).unwrap();
    let destination = BoundDestination::capture(&alias, Policy::FollowSource).unwrap();
    let receipt = stage(&destination, b"published")
        .commit_with(|| retarget(&alias, &second), || {})
        .unwrap();
    assert!(receipt.warning.is_some());
    assert_eq!(fs::read(&first).unwrap(), b"published");
    assert_eq!(fs::read(&second).unwrap(), b"second");
    let identity = FileIdentity::capture(&fs::metadata(&first).unwrap());
    assert!(receipt.destination.file_identities().contains(&identity));
    assert!(receipt.destination.stage().is_err());
}

#[test]
fn claims_keep_hardlink_identity_and_referring_and_target_slots() {
    let (_root, original, _replacement, alias) = aliased_directories();
    let first = original.join("first.vmf");
    let hardlink = original.join("hardlink.vmf");
    let link = original.join("source-link.vmf");
    fs::write(&first, b"source").unwrap();
    fs::hard_link(&first, &hardlink).unwrap();
    symlink(&first, &link).unwrap();
    let a = BoundDestination::capture(&alias.join("first.vmf"), Policy::FollowSource).unwrap();
    let b = BoundDestination::capture(&hardlink, Policy::FollowSource).unwrap();
    let c = BoundDestination::capture(&link, Policy::FollowSource).unwrap();
    assert_eq!(a.file_identities(), b.file_identities());
    assert_ne!(a.claim_slots(), b.claim_slots());
    assert!(c.claim_slots().contains(&a.claim_slots()[0]));
    assert!(c.file_identities().contains(&a.file_identities()[0]));
    let output = BoundDestination::capture(&link, Policy::ReplaceEntry).unwrap();
    assert!(c.claim_slots().contains(&output.claim_slots()[0]));
    assert!(c.file_identities().contains(&output.file_identities()[0]));
}

#[test]
fn newly_inserted_destination_and_changed_source_preserve_external_bytes() {
    for existing in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source.vmf");
        if existing {
            fs::write(&path, b"old source").unwrap();
        }
        let destination = BoundDestination::capture(&path, Policy::FollowSource).unwrap();
        let stage = stage(&destination, b"uncommitted");
        fs::write(&path, b"externally changed source").unwrap();
        assert!(stage.commit().is_err());
        assert_eq!(fs::read(path).unwrap(), b"externally changed source");
    }
}

#[test]
fn source_growth_after_reader_admission_rejects_before_collecting_appended_bytes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.vmf");
    fs::write(&path, b"admitted").unwrap();
    let destination = BoundDestination::capture(&path, Policy::FollowSource).unwrap();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(1);
    let reads = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let read_count = &reads;
        let reader = scope.spawn(move || {
            destination.read_existing_with(|| {
                let previous = read_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if previous == 0 {
                    ready_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                }
            })
        });
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let mut writer = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writer.write_all(&[b'x'; 128 * 1024]).unwrap();
        resume_tx.send(()).unwrap();
        let error = reader.join().unwrap().unwrap_err();
        assert!(error.to_string().contains("grew while reading"));
    });
    assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(fs::metadata(path).unwrap().len(), 8 + 128 * 1024);
}

#[test]
fn output_retarget_preserves_stage_and_directory_while_adopting_the_lin_observation() {
    let (_root, original, _, alias) = aliased_directories();
    let bsp = original.join("output.bsp");
    let lin = original.join("output.lin");
    fs::write(&bsp, b"previous BSP").unwrap();
    fs::write(&lin, b"previous LIN").unwrap();
    fs::set_permissions(&bsp, fs::Permissions::from_mode(0o400)).unwrap();
    fs::set_permissions(&lin, fs::Permissions::from_mode(0o200)).unwrap();
    let first = BoundDestination::capture(&bsp, Policy::ReplaceEntry).unwrap();
    let next = BoundDestination::capture(&alias.join("output.lin"), Policy::ReplaceEntry).unwrap();
    let mut staged = stage(&first, b"pointfile");
    let basename = staged.basename.clone();
    let opened = staged.file().metadata().unwrap();
    let parent = EntryStamp::capture(&fs::metadata(&original).unwrap());
    staged.retarget_output(&next).unwrap();
    assert_eq!(staged.basename, basename);
    let retained = staged.file().metadata().unwrap();
    assert_eq!(
        (retained.dev(), retained.ino(), retained.mode()),
        (opened.dev(), opened.ino(), opened.mode())
    );
    assert_eq!(
        EntryStamp::capture(&fs::metadata(&original).unwrap()),
        parent
    );
    assert_eq!(
        staged.destination.existing.as_ref().unwrap().stamp,
        next.existing.as_ref().unwrap().stamp
    );
    let receipt = staged.commit().unwrap();
    assert!(receipt.warning.is_none());
    assert_eq!(
        receipt.destination.authored_path(),
        alias.join("output.lin")
    );
    assert_eq!(fs::read(&bsp).unwrap(), b"previous BSP");
    assert_eq!(fs::read(&lin).unwrap(), b"pointfile");
    assert_eq!(fs::metadata(lin).unwrap().mode(), opened.mode());
    assert!(!original.join(basename).exists());
}

#[test]
fn output_retarget_rejects_foreign_parent_source_policy_and_its_own_staging_name() {
    let (_root, original, replacement, _) = aliased_directories();
    let bsp = original.join("output.bsp");
    fs::write(&bsp, b"previous BSP").unwrap();
    let first = BoundDestination::capture(&bsp, Policy::ReplaceEntry).unwrap();
    let mut staged = stage(&first, b"unpublished");
    let foreign =
        BoundDestination::capture(&replacement.join("output.lin"), Policy::ReplaceEntry).unwrap();
    assert!(staged.retarget_output(&foreign).is_err());
    let source =
        BoundDestination::capture(&original.join("output.lin"), Policy::FollowSource).unwrap();
    assert!(staged.retarget_output(&source).is_err());
    let self_name =
        BoundDestination::capture(&original.join(&staged.basename), Policy::ReplaceEntry).unwrap();
    assert!(staged.retarget_output(&self_name).is_err());
    assert_eq!(staged.destination.authored_path(), bsp);
    let basename = staged.basename.clone();
    drop(staged);
    assert!(!original.join(basename).exists());
    assert!(!replacement.join("output.lin").exists());
    assert_eq!(fs::read(bsp).unwrap(), b"previous BSP");
}

#[test]
fn output_retarget_rejects_changed_lin_and_preserves_displaced_staging_names() {
    for displaced in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let bsp = root.path().join("output.bsp");
        let lin = root.path().join("output.lin");
        fs::write(&bsp, b"previous BSP").unwrap();
        fs::write(&lin, b"previous LIN").unwrap();
        let first = BoundDestination::capture(&bsp, Policy::ReplaceEntry).unwrap();
        let next = BoundDestination::capture(&lin, Policy::ReplaceEntry).unwrap();
        let mut staged = stage(&first, b"unpublished");
        let staging_path = root.path().join(&staged.basename);
        let moved = root.path().join("displaced");
        if displaced {
            fs::rename(&staging_path, &moved).unwrap();
            fs::write(&staging_path, b"unrelated").unwrap();
        } else {
            fs::write(&lin, b"new independent LIN bytes").unwrap();
        }
        assert!(staged.retarget_output(&next).is_err());
        assert_eq!(staged.destination.authored_path(), bsp);
        drop(staged);
        if displaced {
            assert_eq!(fs::read(staging_path).unwrap(), b"unrelated");
            assert_eq!(fs::read(moved).unwrap(), b"unpublished");
            assert_eq!(fs::read(&lin).unwrap(), b"previous LIN");
        } else {
            assert!(!staging_path.exists());
            assert_eq!(fs::read(&lin).unwrap(), b"new independent LIN bytes");
        }
        assert_eq!(fs::read(bsp).unwrap(), b"previous BSP");
    }
}
