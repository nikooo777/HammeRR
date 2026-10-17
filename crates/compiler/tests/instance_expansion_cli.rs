mod common;

use glam::DVec3;
use map_core::{Document, edit};
use source_formats::vmf::Entity;

#[test]
fn cli_expands_saved_relative_brushes_and_preserves_output_on_bad_child() {
    let directory = tempfile::tempdir().unwrap();
    let gameinfo = directory.path().join("gameinfo.txt");
    std::fs::write(
        &gameinfo,
        "GameInfo { FileSystem { SearchPaths { game |gameinfo_path|. } } }",
    )
    .unwrap();
    let mut child = Document::empty();
    child
        .edit("child", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-16.0, -16.0, 16.0),
                DVec3::new(16.0, 16.0, 48.0),
                "dev/instance",
            )?;
            Ok(())
        })
        .unwrap();
    let child_path = directory.path().join("child.vmf");
    std::fs::write(&child_path, child.vmf().write(&Default::default())).unwrap();
    let mut parent = common::room();
    let mut helper = Entity::new(parent.object_ids().max().unwrap() + 1, "func_instance");
    helper.properties.set("file", "child.vmf");
    helper.properties.set("origin", "100 0 0");
    parent.entities.push(helper);
    let physical = directory.path().join("physical");
    std::fs::create_dir(&physical).unwrap();
    let physical_map = physical.join("parent.vmf");
    std::fs::write(&physical_map, parent.write(&Default::default())).unwrap();
    let input = directory.path().join("parent.vmf");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&physical_map, &input).unwrap();
    #[cfg(not(unix))]
    std::fs::copy(&physical_map, &input).unwrap();
    let bsp = directory.path().join("output.bsp");
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_hammerr-compile"))
            .arg(&input)
            .arg("--game")
            .arg(&gameinfo)
            .arg("-o")
            .arg(&bsp)
            .output()
            .unwrap()
    };
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let baseline = compiler::compile(&common::room(), &common::empty_game());
    assert_eq!(baseline.stats.brushes, 6);
    assert!(String::from_utf8_lossy(&output.stdout).contains("7 brushes"));
    let previous = std::fs::read(&bsp).unwrap();
    assert!(previous.starts_with(b"VBSP"));
    std::fs::write(bsp.with_extension("lin"), b"previous LIN").unwrap();
    let mut bad = child.into_vmf();
    bad.entities.push(Entity::new(
        bad.object_ids().max().unwrap() + 1,
        "func_instance_parms",
    ));
    std::fs::write(&child_path, bad.write(&Default::default())).unwrap();
    let output = run();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("func_instance_parms"));
    assert_eq!(std::fs::read(&bsp).unwrap(), previous);
    assert_eq!(
        std::fs::read(bsp.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
}
