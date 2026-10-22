#![cfg(target_os = "linux")]

#[allow(dead_code)]
mod common;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture() -> (tempfile::TempDir, Vec<u8>) {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("gameinfo.txt"),
        "GameInfo { FileSystem { SearchPaths { game |gameinfo_path|. } } }",
    )
    .unwrap();
    let source = common::room().write(&Default::default()).into_bytes();
    (directory, source)
}

fn compile(directory: &Path, source: &Path, destination: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hammerr-compile"))
        .current_dir(directory)
        .arg(source)
        .arg("--game")
        .arg(directory.join("gameinfo.txt"))
        .arg("-o")
        .arg(destination)
        .args(["--lighting", "fullbright", "--physics", "none"])
        .output()
        .unwrap()
}

#[test]
fn source_aliases_refuse_before_bsp_or_pointfile_publication() {
    for alias in ["source_symlink", "relative_path", "parent_symlink"] {
        let (directory, original) = fixture();
        let physical = directory.path().join("physical");
        fs::create_dir(&physical).unwrap();
        let source = physical.join("source-data.bsp");
        fs::write(&source, &original).unwrap();
        let pointfile = source.with_extension("lin");
        fs::write(&pointfile, b"previous LIN").unwrap();
        let input = match alias {
            "source_symlink" => {
                let input = directory.path().join("author.vmf");
                symlink(&source, &input).unwrap();
                input
            }
            "relative_path" => PathBuf::from("physical/source-data.bsp"),
            "parent_symlink" => {
                let parent = directory.path().join("alias");
                symlink(&physical, &parent).unwrap();
                parent.join("source-data.bsp")
            }
            _ => unreachable!(),
        };
        let output = compile(directory.path(), &input, &source);
        assert!(!output.status.success(), "{alias}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("distinct from the source map"),
            "{alias}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(&source).unwrap(), original, "{alias}");
        assert_eq!(fs::read(&pointfile).unwrap(), b"previous LIN", "{alias}");
        if alias == "source_symlink" {
            assert_eq!(fs::read_link(&input).unwrap(), source);
        }
    }
}

#[test]
fn source_pointfile_alias_preserves_both_existing_files() {
    let (directory, original) = fixture();
    let source = directory.path().join("output.lin");
    fs::write(&source, &original).unwrap();
    let input = directory.path().join("author.vmf");
    symlink(&source, &input).unwrap();
    let destination = directory.path().join("output.bsp");
    fs::write(&destination, b"previous BSP").unwrap();
    let output = compile(directory.path(), &input, &destination);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("distinct from the source map"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_eq!(fs::read(&destination).unwrap(), b"previous BSP");
    assert_eq!(fs::read_link(input).unwrap(), source);
}

#[test]
fn distinct_output_entries_compile_without_changing_the_source() {
    for entry in ["regular", "symlink", "hardlink"] {
        let (directory, original) = fixture();
        let source = directory.path().join("author.vmf");
        fs::write(&source, &original).unwrap();
        let destination = directory.path().join("output.bsp");
        match entry {
            "regular" => fs::write(&destination, b"previous BSP").unwrap(),
            "symlink" => symlink(&source, &destination).unwrap(),
            "hardlink" => fs::hard_link(&source, &destination).unwrap(),
            _ => unreachable!(),
        }
        let pointfile = destination.with_extension("lin");
        fs::write(&pointfile, b"previous LIN").unwrap();
        let output = compile(directory.path(), &source, &destination);
        assert!(
            output.status.success(),
            "{entry}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(&source).unwrap(), original, "{entry}");
        assert_eq!(fs::read(&pointfile).unwrap(), b"previous LIN", "{entry}");
        let bytes = fs::read(&destination).unwrap();
        assert!(bytes.starts_with(b"VBSP"), "{entry}");
        let bsp = source_formats::bsp::Bsp::parse(&bytes).unwrap();
        assert_eq!(
            bsp.records::<source_formats::bsp::Model>().unwrap().len(),
            1,
            "{entry}"
        );
        assert!(!fs::symlink_metadata(destination).unwrap().is_symlink());
    }
}
