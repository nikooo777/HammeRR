use super::*;
use crate::session::atlas_texturing::Rectangle;

#[test]
fn atlas_mcp_png_is_cpu_only_and_generic_commit_cancel_cannot_take_foreign_drafts() {
    assert!(
        serde_json::from_value::<InspectMaterialAtlasArgs>(
            json!({"material":"test/checker","path":"other.vtf"})
        )
        .is_err()
    );
    assert!(serde_json::from_value::<PreviewAtlasFitArgs>(json!({"material":"test/checker","snapshot":"x","rectangle":{"x":-1,"y":0,"width":1,"height":1}})).is_err());
    let (_directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "test/checker",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let face = map_core::surface::FaceId {
        solid,
        side: session.document().solid(solid).unwrap().sides[0].id,
    };
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    let atlas = session.inspect_material_atlas("test/checker").unwrap();
    session
        .preview_atlas_fit(
            Actor::User,
            &[face],
            "test/checker",
            Rectangle {
                x: 16,
                y: 8,
                width: 32,
                height: 16,
            },
            atlas.report["snapshot"].as_str().unwrap(),
            None,
        )
        .unwrap();
    let token = session.face_preview_token().unwrap().to_string();
    let original = session.document().vmf().clone();
    let preview = session.render_document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let inspected = server
        .inspect_material_atlas(Parameters(
            serde_json::from_value(json!({"material":"test/checker"})).unwrap(),
        ))
        .unwrap();
    assert!(!inspected.is_error.unwrap_or(false));
    let report: Value =
        serde_json::from_str(&inspected.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(report, atlas.report);
    assert!(inspected.content[1].as_image().is_some());
    for result in [
        server.commit_face_preview(Parameters(
            serde_json::from_value(json!({"token":token})).unwrap(),
        )),
        server.cancel_face_preview(Parameters(
            serde_json::from_value(json!({"token":token})).unwrap(),
        )),
    ] {
        assert!(result.unwrap().is_error.unwrap_or(false));
    }
    assert_eq!(server.rendering_status()["initialized"], false);
    let session = lock(&shared);
    assert_eq!(session.face_preview_token().unwrap().to_string(), token);
    assert_eq!(session.document().vmf(), &original);
    assert_eq!(session.render_document().vmf(), &preview);
}
