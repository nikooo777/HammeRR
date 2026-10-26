use super::*;
use crate::session::compiled_visibility::{
    GuideMode, Kind, PositionSource, Query, Target, test_support,
};

#[test]
fn compiled_visibility_mcp_matches_shared_inspection_frame_and_single_guide() {
    for invalid in [
        json!({"path":"other.bsp"}),
        json!({"target":{"kind":"leaf","ordinal":-1}}),
        json!({"target":{"kind":"summary","ordinal":1}}),
        json!({"target":{"kind":"position","source":{"kind":"camera","position":[0,0,16]}}}),
        json!({"target":{"kind":"position","source":{"kind":"position","position":[0,0]}}}),
        json!({"target":{"kind":"position","source":{"kind":"position","position":[0,0,16],"ordinal":1}}}),
    ] {
        assert!(serde_json::from_value::<InspectCompiledVisibilityArgs>(invalid).is_err());
    }
    for invalid in [
        json!({"mode":{"kind":"selected","visibility":"pvs"}}),
        json!({"mode":{"kind":"destinations"}}),
        json!({"mode":{"kind":"destinations","visibility":"visible"}}),
        json!({"mode":{"kind":"destinations","visibility":"pvs","limit":1}}),
    ] {
        assert!(serde_json::from_value::<SetCompiledVisibilityOverlayArgs>(invalid).is_err());
    }
    let (_directory, session, _) = test_support::compiled();
    let expected = session
        .inspect_compiled_visibility(&Query::default())
        .unwrap();
    let token = expected.build["token"].as_str().unwrap().to_owned();
    let target = expected.page.rows.iter().find(|row| matches!(row, crate::session::compiled_visibility::Row::Leaf(leaf) if leaf.cluster >= 0)).unwrap().target();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    for tool in [
        "inspect_compiled_visibility",
        "frame_compiled_visibility",
        "set_compiled_visibility_overlay",
    ] {
        assert!(server.get_tool(tool).is_some());
    }
    let actual = server
        .inspect_compiled_visibility(Parameters(serde_json::from_value(json!({})).unwrap()))
        .unwrap();
    assert!(!actual.is_error.unwrap_or(false));
    let actual: Value = serde_json::from_str(&actual.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(actual, serde_json::to_value(expected).unwrap());
    for source in [
        PositionSource::Position {
            position: [0.0, 0.0, 16.0],
        },
        PositionSource::Camera {},
    ] {
        let target = Target::Position { source };
        let expected = lock(&shared)
            .inspect_compiled_visibility(&Query {
                build_token: Some(token.clone()),
                target,
                ..Query::default()
            })
            .unwrap();
        let actual = server
            .inspect_compiled_visibility(Parameters(
                serde_json::from_value(json!({"build_token":token,"target":target})).unwrap(),
            ))
            .unwrap();
        assert!(!actual.is_error.unwrap_or(false));
        let actual: Value =
            serde_json::from_str(&actual.content[0].as_text().unwrap().text).unwrap();
        assert_eq!(actual, serde_json::to_value(expected).unwrap());
    }
    for visibility in [Kind::Pvs, Kind::Pas] {
        let mode = GuideMode::Destinations { visibility };
        let expected = lock(&shared)
            .set_compiled_visibility_overlay(Actor::Agent, Some(token.clone()), Some(target), mode)
            .unwrap();
        let actual = server
            .set_compiled_visibility_overlay(Parameters(
                serde_json::from_value(json!({"build_token":token,"target":target,"mode":mode}))
                    .unwrap(),
            ))
            .unwrap();
        assert!(!actual.is_error.unwrap_or(false));
        let actual: Value =
            serde_json::from_str(&actual.content[0].as_text().unwrap().text).unwrap();
        assert_eq!(actual, expected);
    }
    let result = server
        .frame_compiled_visibility(Parameters(
            serde_json::from_value(json!({"build_token":token,"target":target,"aspect":1.6}))
                .unwrap(),
        ))
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let result = server
        .set_compiled_visibility_overlay(Parameters(
            serde_json::from_value(json!({"build_token":token,"target":target})).unwrap(),
        ))
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
    assert_eq!(
        lock(&shared).compiled_visibility_overlay()["displayed"],
        true
    );
    let camera = lock(&shared).camera();
    let result = server
        .frame_compiled_visibility(Parameters(
            serde_json::from_value(
                json!({"build_token":token,"target":Target::Cluster {ordinal:0}}),
            )
            .unwrap(),
        ))
        .unwrap();
    assert!(result.is_error.unwrap_or(false));
    assert_eq!(lock(&shared).camera(), camera);
    let result = server
        .set_compiled_visibility_overlay(Parameters(serde_json::from_value(json!({})).unwrap()))
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
    assert_eq!(
        lock(&shared).compiled_visibility_overlay()["requested"],
        false
    );
}
