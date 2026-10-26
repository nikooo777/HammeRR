use super::*;
use crate::session::compiled_visibility::{Query, Target, test_support};

#[test]
fn compiled_visibility_mcp_matches_shared_inspection_frame_and_single_guide() {
    for invalid in [
        json!({"path":"other.bsp"}),
        json!({"target":{"kind":"leaf","ordinal":-1}}),
        json!({"target":{"kind":"summary","ordinal":1}}),
    ] {
        assert!(serde_json::from_value::<InspectCompiledVisibilityArgs>(invalid).is_err());
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
