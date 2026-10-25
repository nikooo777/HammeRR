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
    for padding in [json!(-1), json!(0.5), json!(4_294_967_296_u64)] {
        assert!(
            serde_json::from_value::<PreviewAtlasFitArgs>(json!({
                "material":"test/checker","snapshot":"x","padding":padding,
                "rectangle":{"x":0,"y":0,"width":32,"height":16}
            }))
            .is_err()
        );
    }
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

fn report(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn atlas_mcp_padding_is_optional_and_invalid_revisions_retain_the_reviewed_usable_area() {
    let (_directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let mut faces = Vec::new();
    for x in [0.0, 64.0] {
        let ObjectId::Solid(solid) = session
            .create_block(
                Actor::Agent,
                DVec3::new(x, 0.0, 0.0),
                DVec3::new(x + 64.0, 64.0, 64.0),
                "test/checker",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let side = session
            .document()
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
            .unwrap()
            .id;
        faces.push(map_core::surface::FaceId { solid, side });
    }
    session.select_faces(&faces, SelectMode::Replace).unwrap();
    let snapshot = session
        .inspect_material_atlas("test/checker")
        .unwrap()
        .report["snapshot"]
        .clone();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let mut request = json!({
        "material":"test/checker","snapshot":snapshot,
        "rectangle":{"x":16,"y":8,"width":32,"height":16}
    });
    let omitted = report(
        server.preview_atlas_fit(Parameters(serde_json::from_value(request.clone()).unwrap())),
    );
    assert!(omitted["atlas"]["preview"].get("padding").is_none());
    assert!(omitted["atlas"]["preview"].get("mode").is_none());
    assert!(omitted["atlas"]["preview"].get("reference").is_none());
    assert!(
        omitted["atlas"]["preview"]
            .get("usable_rectangle")
            .is_none()
    );
    let zero_preview = lock(&shared).render_document().vmf().clone();
    request["padding"] = json!(0);
    request["mode"] = json!("per_face");
    request["token"] = omitted["atlas"]["preview"]["token"].clone();
    let explicit = report(
        server.preview_atlas_fit(Parameters(serde_json::from_value(request.clone()).unwrap())),
    );
    let mut explicit_atlas = explicit["atlas"].clone();
    explicit_atlas["preview"]["token"] = omitted["atlas"]["preview"]["token"].clone();
    assert_eq!(explicit_atlas, omitted["atlas"]);
    assert_eq!(lock(&shared).render_document().vmf(), &zero_preview);
    request["padding"] = json!(2);
    request["token"] = explicit["atlas"]["preview"]["token"].clone();
    let padded = report(
        server.preview_atlas_fit(Parameters(serde_json::from_value(request.clone()).unwrap())),
    );
    assert_eq!(padded["atlas"]["preview"]["padding"], 2);
    assert_eq!(
        padded["atlas"]["preview"]["usable_rectangle"],
        json!({"x":18,"y":10,"width":28,"height":12})
    );
    assert_eq!(
        padded["atlas"]["preview"]["uv_bounds"],
        json!([[0.28125, 0.3125], [0.71875, 0.6875]])
    );
    let preview = lock(&shared).render_document().vmf().clone();
    request["padding"] = json!(8);
    request["token"] = padded["atlas"]["preview"]["token"].clone();
    let invalid = server
        .preview_atlas_fit(Parameters(serde_json::from_value(request.clone()).unwrap()))
        .unwrap();
    assert!(invalid.is_error.unwrap_or(false));
    assert_eq!(lock(&shared).atlas_fit_state(), padded["atlas"]);
    assert_eq!(lock(&shared).render_document().vmf(), &preview);
    request["padding"] = json!(2);
    request["mode"] = json!("unknown");
    assert!(serde_json::from_value::<PreviewAtlasFitArgs>(request.clone()).is_err());
    request["mode"] = json!("together");
    for reference in [None, Some(json!({"solid":u32::MAX,"side":u32::MAX}))] {
        if let Some(reference) = reference {
            request["reference"] = reference;
        }
        let invalid = server
            .preview_atlas_fit(Parameters(serde_json::from_value(request.clone()).unwrap()))
            .unwrap();
        assert!(invalid.is_error.unwrap_or(false));
        assert_eq!(lock(&shared).atlas_fit_state(), padded["atlas"]);
        assert_eq!(lock(&shared).render_document().vmf(), &preview);
    }
    request["reference"] = json!({"solid":faces[0].solid,"side":faces[0].side});
    let together = report(
        server.preview_atlas_fit(Parameters(serde_json::from_value(request.clone()).unwrap())),
    );
    assert_eq!(together["atlas"]["preview"]["mode"], "together");
    assert_eq!(
        together["atlas"]["preview"]["reference"],
        request["reference"]
    );
    assert_eq!(
        together["atlas"]["preview"]["uv_bounds"],
        padded["atlas"]["preview"]["uv_bounds"]
    );
    {
        let session = lock(&shared);
        for face in &faces {
            let side = session.preview_side(*face).unwrap();
            assert!(
                map_core::texel_uv(side, DVec3::new(64.0, 32.0, 64.0))
                    .abs_diff_eq(DVec2::new(32.0, 16.0), 1e-8)
            );
        }
    }
    let preview = lock(&shared).render_document().vmf().clone();
    request["token"] = together["atlas"]["preview"]["token"].clone();
    request["mode"] = json!("per_face");
    let invalid = server
        .preview_atlas_fit(Parameters(serde_json::from_value(request).unwrap()))
        .unwrap();
    assert!(invalid.is_error.unwrap_or(false));
    assert_eq!(lock(&shared).atlas_fit_state(), together["atlas"]);
    assert_eq!(lock(&shared).render_document().vmf(), &preview);
    report(server.commit_face_preview(Parameters(
        serde_json::from_value(json!({"token":together["atlas"]["preview"]["token"]})).unwrap(),
    )));
    assert_eq!(lock(&shared).document().vmf(), &preview);
    assert_eq!(server.rendering_status()["initialized"], false);
}

#[test]
fn named_atlas_region_mcp_catalog_choice_and_owned_fit_share_session_operations() {
    let (_directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let ObjectId::Solid(solid) = session
        .create_block(
            Actor::Agent,
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
    let original = session.document().vmf().clone();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    for tool in [
        "list_atlas_regions",
        "save_atlas_region",
        "rename_atlas_region",
        "delete_atlas_region",
        "choose_atlas_region",
    ] {
        assert!(server.get_tool(tool).is_some(), "missing tool {tool}");
    }
    assert!(
        serde_json::from_value::<SaveAtlasRegionArgs>(json!({
            "name":"Panel","material":"test/checker","rectangle":{"x":0,"y":0,"width":1,"height":1},
            "snapshot":"x","catalog_snapshot":"x","mode":"together"
        }))
        .is_err()
    );
    let atlas = lock(&shared)
        .inspect_material_atlas("test/checker")
        .unwrap()
        .report;
    let listed = report(server.list_atlas_regions());
    assert_eq!(listed["regions"], json!([]));
    let saved = report(server.save_atlas_region(Parameters(serde_json::from_value(json!({
        "name":"Panel","material":"test/checker","rectangle":{"x":16,"y":8,"width":32,"height":16},
        "padding":2,"snapshot":atlas["snapshot"],"catalog_snapshot":listed["snapshot"]
    })).unwrap())));
    assert_eq!(
        saved["regions"],
        lock(&shared).atlas_regions_state().unwrap()["regions"]
    );
    assert_eq!(lock(&shared).document().vmf().world, original.world);
    let id = saved["saved_id"].clone();
    let renamed = report(
        server.rename_atlas_region(Parameters(
            serde_json::from_value(json!({
                "id":id,"name":"Trim","catalog_snapshot":saved["snapshot"]
            }))
            .unwrap(),
        )),
    );
    let stale = server.choose_atlas_region(Parameters(serde_json::from_value(json!({
        "id":id,"material":"test/checker","snapshot":atlas["snapshot"],"catalog_snapshot":saved["snapshot"]
    })).unwrap())).unwrap();
    assert!(stale.is_error.unwrap_or(false));
    let chosen = report(server.choose_atlas_region(Parameters(serde_json::from_value(json!({
        "id":id,"material":"test/checker","snapshot":atlas["snapshot"],"catalog_snapshot":renamed["snapshot"]
    })).unwrap())));
    assert_eq!(chosen["name"], "Trim");
    let preview = report(server.preview_atlas_fit(Parameters(serde_json::from_value(json!({
        "material":chosen["material"],"snapshot":chosen["snapshot"],"rectangle":chosen["rectangle"],"padding":chosen["padding"]
    })).unwrap())));
    assert_eq!(
        preview["atlas"]["preview"]["usable_rectangle"],
        json!({"x":18,"y":10,"width":28,"height":12})
    );
    assert_eq!(lock(&shared).document().undo_labels().count(), 3);
    let rejected = server
        .delete_atlas_region(Parameters(
            serde_json::from_value(json!({"id":id,"catalog_snapshot":renamed["snapshot"]}))
                .unwrap(),
        ))
        .unwrap();
    assert!(rejected.is_error.unwrap_or(false));
    report(server.cancel_face_preview(Parameters(
        serde_json::from_value(json!({"token":preview["atlas"]["preview"]["token"]})).unwrap(),
    )));
    let deleted = report(server.delete_atlas_region(Parameters(
        serde_json::from_value(json!({"id":id,"catalog_snapshot":renamed["snapshot"]})).unwrap(),
    )));
    assert_eq!(deleted["regions"], json!([]));
    assert_eq!(server.rendering_status()["initialized"], false);
}
