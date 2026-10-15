mod common;

use source_formats::gamefs::{Mount, MountedPath};

#[test]
fn malformed_assets_stop_compilation_and_preserve_previous_output() {
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials");
    std::fs::create_dir(&materials).unwrap();
    let mut fs = common::empty_game();
    fs.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    let mut vmf = common::room();
    vmf.world.solids[0].sides[0].material = "broken".into();
    let source = vmf.clone();
    let destination = directory.path().join("existing.bsp");
    std::fs::write(&destination, b"previous successful build").unwrap();
    std::fs::write(materials.join("broken.vtf"), b"not a VTF").unwrap();
    for (text, message) in [
        ("LightmappedGeneric { $basetexture }", "missing a value"),
        ("Patch { include materials/absent.vmt }", "doesn't exist"),
        ("Patch { include materials/broken.vmt }", "deeply"),
        ("LightmappedGeneric { $basetexture broken }", "broken.vtf"),
        ("LightmappedGeneric { $basetexture ../outside }", "path"),
    ] {
        std::fs::write(materials.join("broken.vmt"), text).unwrap();
        let output = compiler::compile(&vmf, &fs);
        assert!(output.report.has_errors(), "{:?}", output.report);
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains(message)),
            "{:?}",
            output.report
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"previous successful build"
        );
        assert_eq!(vmf, source);
        assert!(compiler::map::Map::load(&vmf, &fs, &mut compiler::Report::default()).is_err());
    }
    std::fs::write(
        materials.join("broken.vmt"),
        "LightmappedGeneric { $basetexture absent }",
    )
    .unwrap();
    let output = compiler::compile(&vmf, &fs);
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|item| item.message.contains("absent.vtf") && item.message.contains("not found"))
    );
    let mut texture = vec![0u8; 84];
    texture[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7_u32), (8, 2), (12, 80)] {
        texture[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for offset in [16, 18, 24, 63] {
        texture[offset..offset + 2].copy_from_slice(&1u16.to_le_bytes());
    }
    texture[56] = 1;
    texture[57..61].copy_from_slice(&(-1i32).to_le_bytes());
    texture[32..36].copy_from_slice(&f32::NAN.to_le_bytes());
    source_formats::vtf::Vtf::parse(texture.clone()).unwrap();
    std::fs::write(materials.join("broken.vtf"), texture).unwrap();
    std::fs::write(
        materials.join("broken.vmt"),
        "LightmappedGeneric { $basetexture broken }",
    )
    .unwrap();
    let output = compiler::compile(&vmf, &fs);
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|item| item.message.contains("reflectivity must be finite"))
    );
}
