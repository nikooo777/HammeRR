use super::*;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::MetadataExt;

fn names(path: &Path) -> BTreeSet<std::ffi::OsString> {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect()
}

#[test]
fn early_stage_is_reused_for_bsp_and_lin_in_ordinary_and_guarded_commits() {
    for leak in [false, true] {
        for checked in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("output.bsp");
            fs::write(&path, b"previous BSP").unwrap();
            let selected = if leak {
                path.with_extension("lin")
            } else {
                path.clone()
            };
            let control = Control::default();
            let lease = checked.then(|| OutputLease::acquire(&selected, &control).unwrap());
            let reservation = WriteReservation::new(&path, &control).unwrap();
            let stage = reservation.staged.file().metadata().unwrap();
            let before = names(root.path());
            let directory = FileStamp::capture_container(root.path()).unwrap();
            let mut output = compile(&test_common::room(), &test_common::empty_game());
            assert!(output.bsp.is_some());
            if leak {
                output.bsp = None;
                output.leak = Some(Leak {
                    entity: 1,
                    points: vec![DVec3::ZERO, DVec3::ONE],
                });
            }
            let prepared = output
                .prepare_write_reserved(reservation, &control)
                .unwrap()
                .unwrap();
            let retained = prepared.staged.file().metadata().unwrap();
            assert_eq!(
                (retained.dev(), retained.ino(), retained.mode()),
                (stage.dev(), stage.ino(), stage.mode())
            );
            assert_eq!(names(root.path()), before);
            directory.verify_metadata().unwrap();
            let receipt = match lease {
                Some(lease) => prepared
                    .commit_checked_with_receipt(&lease, &control, || directory.verify_metadata())
                    .unwrap(),
                None => prepared.commit_receipt(&control).unwrap(),
            };
            assert_eq!(receipt.success, !leak);
            assert!(receipt.publication.warning.is_none());
            assert_eq!(receipt.publication.destination.physical_path(), selected);
            let published = fs::metadata(&selected).unwrap();
            assert_eq!(
                (published.dev(), published.ino(), published.mode()),
                (stage.dev(), stage.ino(), stage.mode())
            );
            if leak {
                assert_eq!(fs::read(&path).unwrap(), b"previous BSP");
                assert_eq!(fs::read_to_string(selected).unwrap(), "0 0 0\n1 1 1\n");
            } else {
                Bsp::parse(&fs::read(selected).unwrap()).unwrap();
            }
        }
    }
}

#[test]
fn cancelled_preparation_retires_the_early_stage_without_touching_previous_output() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("output.bsp");
    fs::write(&path, b"previous BSP").unwrap();
    let before = names(root.path());
    let control = Control::default();
    let reservation = WriteReservation::new(&path, &control).unwrap();
    assert_eq!(names(root.path()).len(), before.len() + 1);
    let output = compile(&test_common::room(), &test_common::empty_game());
    control.cancel().unwrap();
    assert!(
        output
            .prepare_write_reserved(reservation, &control)
            .is_err()
    );
    assert_eq!(names(root.path()), before);
    assert_eq!(fs::read(path).unwrap(), b"previous BSP");
}
