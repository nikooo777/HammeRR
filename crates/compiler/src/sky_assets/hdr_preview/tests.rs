use super::*;
use crate::sky_assets::tests::{Fixture, vtf};

fn family() -> Fixture {
    let fixture = Fixture::new();
    fixture.family("test_hdr", "");
    fixture
}
#[test]
fn selected_hdr_channel_reuses_bounded_provenance_and_deduplicates_original_vtf_bytes() {
    let fixture = family();
    let faces = load_hdr_sky_preview(&fixture.game, "test_hdr");
    for face in &faces {
        assert!(face.error.is_none(), "{:?}", face.error);
        assert!(face.source.is_some() && face.texture_source.is_some());
        assert_eq!(
            face.texture.as_ref().unwrap().format,
            source_formats::vtf::ImageFormat::Rgba16161616F
        );
    }
    assert!(Arc::ptr_eq(
        faces[0].texture.as_ref().unwrap(),
        faces[1].texture.as_ref().unwrap()
    ));
}
#[test]
fn hdr_admission_does_not_require_or_substitute_ldr_and_rejects_non_direct_or_partial_materials() {
    let fixture = family();
    for suffix in SUFFIXES {
        fixture.write(
            &format!("materials/skybox/test_hdr{suffix}.vmt"),
            "Sky { $hdrbasetexture skybox/shared_hdr }",
        );
    }
    assert!(
        load_hdr_sky_preview(&fixture.game, "test_hdr")
            .iter()
            .all(|face| face.error.is_none())
    );
    for text in [
        "Sky { $basetexture test/checker }",
        "Sky { $hdrbasetexture skybox/shared_hdr",
        "Patch { include materials/skybox/test_hdrbk.vmt }",
        "Sky { $hdrbasetexture skybox/shared_hdr $hdrcompressedtexture skybox/shared_hdr }",
        "Sky { $hdrbasetexture skybox/shared_hdr $color red }",
        "Sky { $hdrbasetexture skybox/shared_hdr $hdrbasetexture skybox/shared_hdr }",
    ] {
        fixture.write("materials/skybox/test_hdrrt.vmt", text);
        let faces = load_hdr_sky_preview(&fixture.game, "test_hdr");
        assert!(
            faces[0].error.is_some() && faces[0].texture.is_none(),
            "{text}"
        );
        assert!(faces[1].texture.is_some());
    }
}
#[test]
fn hdr_preview_enforces_static_color_layout_and_read_limits() {
    let fixture = family();
    let original = vtf(24, 2, 2, 1);
    for (offset, value, size) in [(24, 2u16, 80 + 2 * 2 * 8 * 2), (63, 0, original.len())] {
        let mut bytes = original.clone();
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        bytes.resize(size, 0);
        fixture.write("materials/skybox/shared_hdr.vtf", &bytes);
        assert!(
            load_hdr_sky_preview(&fixture.game, "test_hdr")
                .iter()
                .all(|face| face.texture.is_none())
        );
    }
    fixture.write("materials/skybox/shared_hdr.vtf", vtf(0, 2, 2, 1));
    assert!(
        load_hdr_sky_preview(&fixture.game, "test_hdr")[0]
            .error
            .as_ref()
            .unwrap()
            .contains("RGBA16F or RGBA16")
    );
    fixture.write(
        "materials/skybox/shared_hdr.vtf",
        vec![0; MAX_HDR_PREVIEW_TEXTURE_BYTES + 1],
    );
    assert!(
        load_hdr_sky_preview(&fixture.game, "test_hdr")[0]
            .error
            .as_ref()
            .unwrap()
            .contains("byte read limit")
    );
    assert!(
        load_hdr_sky_preview(&fixture.game, "../outside")
            .iter()
            .all(|face| face.source.is_none() && face.error.is_some())
    );
}
