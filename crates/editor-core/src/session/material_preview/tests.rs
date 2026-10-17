use super::*;
use crate::session::{Actor, PropertyTarget};

#[test]
fn material_channels_preserve_color_bytes_and_inspect_decoded_alpha_without_edits() {
    let (directory, game) = crate::test_assets::alpha_game();
    let mut session = Session::new(Ok(game));
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let token = session.property_draft().unwrap().token;
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let color = session.material_preview("test/checker").unwrap();
    let explicit = session
        .material_preview_channel("test/checker", Channel::Color)
        .unwrap();
    let alpha = session
        .material_preview_channel("test/checker", Channel::Alpha)
        .unwrap();
    let bytes = std::fs::read(directory.path().join("materials/test/checker.vtf")).unwrap();
    assert_eq!(color.image.as_ref().unwrap().pixels, bytes[80..]);
    assert_eq!(
        color.image.as_ref().unwrap().pixels,
        explicit.image.unwrap().pixels
    );
    assert_eq!(color.source, alpha.source);
    assert_eq!(alpha.alpha_range, Some([0, 255]));
    let provenance = alpha.source.as_ref().unwrap();
    assert_eq!(provenance.texture, "materials/test/checker.vtf");
    assert!(!provenance.cached);
    assert_eq!(
        (provenance.mip, provenance.frame, provenance.face),
        (0, 0, 0)
    );
    for (expected, actual) in bytes[80..]
        .as_chunks::<4>()
        .0
        .iter()
        .zip(alpha.image.unwrap().pixels.as_chunks::<4>().0.iter())
    {
        assert_eq!(*actual, [expected[3], expected[3], expected[3], 255]);
    }
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.property_draft().unwrap().token, token);
}

#[test]
fn material_channels_keep_cached_pixels_and_source_together_until_reload() {
    let (directory, game) = crate::test_assets::alpha_game();
    let mut session = Session::new(Ok(game));
    session.materials.load(&session.game, "test/checker");
    let before = session
        .material_preview_channel("test/checker", Channel::Alpha)
        .unwrap();
    assert!(before.source.as_ref().unwrap().cached);
    let folder = directory.path().join("materials/test");
    let mut replacement = std::fs::read(folder.join("checker.vtf")).unwrap();
    for pixel in replacement[80..].as_chunks_mut::<4>().0.iter_mut() {
        pixel.copy_from_slice(&[11, 22, 33, 47]);
    }
    std::fs::write(folder.join("replacement.vtf"), replacement).unwrap();
    std::fs::write(
        folder.join("checker.vmt"),
        "UnlitGeneric { $basetexture test/replacement }",
    )
    .unwrap();
    let retained = session
        .material_preview_channel("test/checker", Channel::Alpha)
        .unwrap();
    assert_eq!(retained.source, before.source);
    assert_eq!(retained.image.unwrap().pixels, before.image.unwrap().pixels);
    session.reload_material_assets(Actor::User);
    let refreshed = session
        .material_preview_channel("test/checker", Channel::Alpha)
        .unwrap();
    assert_eq!(
        refreshed.source.as_ref().unwrap().texture,
        "materials/test/replacement.vtf"
    );
    assert_ne!(refreshed.source, before.source);
    assert_eq!(refreshed.alpha_range, Some([47, 47]));
    assert!(
        refreshed
            .image
            .unwrap()
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [47, 47, 47, 255])
    );
}

#[test]
fn material_alpha_preserves_missing_and_failure_states_instead_of_white_substitutes() {
    let (directory, game) = crate::test_assets::game();
    let session = Session::new(Ok(game));
    let missing = session
        .material_preview_channel("test/absent", Channel::Alpha)
        .unwrap();
    assert!(missing.missing && missing.image.is_none());
    assert!(missing.source.is_none() && missing.alpha_range.is_none());
    let folder = directory.path().join("materials/test");
    for text in [
        "LightmappedGeneric { $basetexture }",
        "LightmappedGeneric { $basetexture test/broken }",
    ] {
        std::fs::write(folder.join("broken.vmt"), text).unwrap();
        std::fs::write(folder.join("broken.vtf"), b"broken").unwrap();
        assert_eq!(
            session.material_preview("test/broken").unwrap_err(),
            session
                .material_preview_channel("test/broken", Channel::Alpha)
                .unwrap_err()
        );
    }
    assert!(
        session
            .material_preview_channel("", Channel::Alpha)
            .is_err()
    );
    let bytes = std::fs::read(folder.join("checker.vtf")).unwrap();
    let mut rgb = bytes[..80].to_vec();
    rgb[52..56].copy_from_slice(&2u32.to_le_bytes());
    rgb.extend(
        bytes[80..]
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|pixel| pixel[..3].iter().copied()),
    );
    std::fs::write(folder.join("opaque.vtf"), rgb).unwrap();
    std::fs::write(
        folder.join("opaque.vmt"),
        "UnlitGeneric { $basetexture test/opaque }",
    )
    .unwrap();
    let opaque = session
        .material_preview_channel("test/opaque", Channel::Alpha)
        .unwrap();
    assert!(!opaque.missing && opaque.source.is_some());
    assert_eq!(opaque.alpha_range, Some([255, 255]));
    assert!(opaque.image.unwrap().pixels.iter().all(|byte| *byte == 255));
}
