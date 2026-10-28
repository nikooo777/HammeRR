use std::fs;

use source_formats::gamefs::GameFs;

fn fixture(extra: &str) -> (tempfile::TempDir, GameFs) {
    let (directory, game) = crate::test_assets::game();
    let skybox = directory.path().join("materials/skybox");
    fs::create_dir_all(&skybox).unwrap();
    for suffix in render::sky::SUFFIXES {
        fs::write(
            skybox.join(format!("test{suffix}.vmt")),
            format!("Sky {{ $basetexture test/checker $hdrbasetexture skybox/hdr {extra} }}"),
        )
        .unwrap();
    }
    let source = fs::read(directory.path().join("materials/test/checker.vtf")).unwrap();
    let mut hdr = source[..80].to_vec();
    hdr[52..56].copy_from_slice(&24u32.to_le_bytes());
    hdr.resize(80 + 64 * 32 * 8, 0);
    fs::write(skybox.join("hdr.vtf"), hdr).unwrap();
    (directory, game)
}

#[test]
fn structure_readiness_does_not_imply_editor_preview_support() {
    let (_directory, game) =
        fixture("$basetexturetransform \"center .5 .5 scale 1 1 rotate 15 translate 0 0\"");
    let mut vmf = map_core::Document::empty().vmf().clone();
    vmf.world.properties.set("skyname", "test");
    let report = compiler::sky_assets::inspect_sky_assets(&vmf, &game, None);
    assert!(report.ldr_ready && report.hdr_ready);
    assert!(!render::sky::Skybox::load(&game, "test").ready());
}
