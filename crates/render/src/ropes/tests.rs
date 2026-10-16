use super::*;

fn node(id: u32, class: &str, origin: &str, name: &str, next: &str) -> Entity {
    let mut entity = Entity::new(id, class);
    for (key, value) in [("origin", origin), ("targetname", name), ("NextKey", next)] {
        entity.properties.set(key, value);
    }
    entity
}

fn fixture() -> Vmf {
    let mut vmf = Vmf::parse("world { \"id\" \"1\" \"classname\" \"worldspawn\" }").unwrap();
    vmf.entities = vec![
        node(2, "move_rope", "0 0 128", "start", "end"),
        node(3, "keyframe_rope", "256 0 160", "end", ""),
    ];
    vmf
}

#[test]
fn authored_slack_width_and_subdivision_produce_bounded_static_geometry_and_exact_picks() {
    let mut vmf = fixture();
    vmf.entities[0].properties.set("Slack", "64");
    vmf.entities[0].properties.set("Width", "12");
    vmf.entities[0].properties.set("Subdiv", "8");
    let preview = prepare(&vmf, |_| true);
    assert!(preview.issues.is_empty());
    assert_eq!(preview.links.len(), 1);
    let link = &preview.links[0];
    assert_eq!(link.points.len(), MAX_CURVE_SEGMENTS + 1);
    assert_eq!(link.points[0], DVec3::new(0.0, 0.0, 128.0));
    assert_eq!(*link.points.last().unwrap(), DVec3::new(256.0, 0.0, 160.0));
    assert!(link.points[32].z < 128.0);
    assert!((link.length() - (256.0_f64.hypot(32.0) + 64.0)).abs() < 1e-5);
    let triangles = link.triangles().collect::<Vec<_>>();
    assert_eq!(triangles.len(), MAX_CURVE_SEGMENTS * TUBE_SIDES * 2);
    assert!(triangles.iter().flatten().all(|point| point.is_finite()));
    let ray = Ray {
        origin: link.points[32] - DVec3::Y * 100.0,
        direction: DVec3::Y * 3.0,
    };
    let hit = link.pick(&ray).unwrap();
    assert_eq!(hit.object, ObjectId::Entity(2));
    assert!(hit.distance > 93.0 && hit.distance < 97.0);
    assert!(
        link.pick(&Ray {
            origin: ray.origin + DVec3::Z * 20.0,
            ..ray
        })
        .is_none()
    );
    vmf.entities[0].properties.set("Slack", "0");
    let straight = prepare(&vmf, |_| true);
    let link = &straight.links[0];
    assert_eq!(link.points[32], DVec3::new(128.0, 0.0, 144.0));
}

#[test]
fn bad_links_are_diagnostic_and_never_produce_a_substitute_span() {
    for (next, expected) in [
        ("missing", "no authored target"),
        ("start", "same entity"),
        ("e*", "exact authored"),
    ] {
        let mut vmf = fixture();
        vmf.entities[0].properties.set("NextKey", next);
        let preview = prepare(&vmf, |_| true);
        assert!(preview.links.is_empty());
        assert!(preview.issues[0].message.contains(expected));
    }
    let mut vmf = fixture();
    vmf.entities
        .push(node(4, "info_target", "256 32 128", "END", ""));
    assert!(
        prepare(&vmf, |_| true).issues[0]
            .message
            .contains("multiple")
    );
    vmf.entities.pop();
    vmf.entities[1].hidden = true;
    assert!(prepare(&vmf, |_| true).issues[0].message.contains("hidden"));
    vmf.entities[1].hidden = false;
    assert!(
        prepare(&vmf, |id| id != ObjectId::Entity(3))
            .links
            .is_empty()
    );
    vmf.entities[1].properties.set("NextKey", "start");
    let cycle = prepare(&vmf, |_| true);
    assert!(cycle.links.is_empty());
    assert_eq!(cycle.issues.len(), 2);
    assert!(
        cycle
            .issues
            .iter()
            .all(|issue| issue.message.contains("cyclic"))
    );
}

#[test]
fn unsupported_parameters_duplicate_keys_and_nonfinite_or_out_of_bounds_inputs_are_visible() {
    for (key, value, expected) in [
        ("Slack", "NaN", "finite"),
        ("Slack", "-1", "Slack"),
        ("Width", "65", "Width"),
        ("Subdiv", "1.5", "Subdiv"),
        ("Type", "2", "Type"),
        ("Dangling", "1", "Dangling"),
        ("PositionInterpolator", "0", "PositionInterpolator"),
        ("parentname", "moving_prop,hook", "Parented"),
        ("StartAttachment", "1", "attachment"),
        ("origin", "0 0 inf", "origin"),
        ("origin", "0 0 -65536", "bounds"),
    ] {
        let mut vmf = fixture();
        vmf.entities[0].properties.set(key, value);
        let preview = prepare(&vmf, |_| true);
        assert!(preview.links.is_empty(), "{key}={value}");
        assert!(
            preview.issues[0].message.contains(expected),
            "{key}={value}: {:?}",
            preview.issues
        );
    }
    let mut vmf = fixture();
    vmf.entities[0]
        .properties
        .0
        .push(("nextkey".into(), "other".into()));
    assert!(
        prepare(&vmf, |_| true).issues[0]
            .message
            .contains("duplicate")
    );
    let mut vmf = fixture();
    vmf.entities[1].properties.set("origin", "0 0 256");
    assert!(
        prepare(&vmf, |_| true).issues[0]
            .message
            .contains("physics pose")
    );
}

#[test]
fn preview_node_budget_is_explicit_and_does_not_grow_geometry_past_the_limit() {
    let mut vmf = fixture();
    for id in 4..MAX_LINKS as u32 + 10 {
        vmf.entities.push(node(
            id,
            "move_rope",
            "0 0 128",
            &format!("node_{id}"),
            "end",
        ));
    }
    let preview = prepare(&vmf, |_| true);
    assert_eq!(preview.omitted, vmf.entities.len() - MAX_LINKS);
    assert!(preview.links.len() <= MAX_LINKS);
    vmf.entities[0]
        .properties
        .set("NextKey", format!("node_{}", MAX_LINKS + 5));
    let preview = prepare(&vmf, |_| true);
    assert!(
        preview
            .issues
            .iter()
            .any(|issue| issue.entity == 2 && issue.message.contains("inspection limit"))
    );
}

#[test]
fn oversized_identifiers_reject_without_truncating_exact_matches_or_expanding_diagnostics() {
    let mut vmf = fixture();
    let accepted = "a".repeat(MAX_IDENTIFIER_BYTES);
    vmf.entities[0]
        .properties
        .set("NextKey", accepted.to_ascii_uppercase());
    vmf.entities[1].properties.set("targetname", &accepted);
    assert_eq!(prepare(&vmf, |_| true).links.len(), 1);
    let oversized = accepted + "b";
    vmf.entities[0].properties.set("NextKey", &oversized);
    let preview = prepare(&vmf, |_| true);
    assert!(preview.links.is_empty());
    assert!(preview.issues[0].message.contains("identifier exceeds"));
    assert!(preview.issues[0].message.len() < MAX_IDENTIFIER_BYTES);
    assert_eq!(
        vmf.entities[0].properties.get("NextKey"),
        Some(oversized.as_str())
    );

    let oversized = "é".repeat(MAX_IDENTIFIER_BYTES * 32);
    vmf.entities[0].properties.set("NextKey", &oversized);
    vmf.entities[1].properties.set("targetname", &oversized);
    let preview = prepare(&vmf, |_| true);
    assert!(preview.links.is_empty());
    assert_eq!(preview.issues.len(), 2);
    assert!(
        preview
            .issues
            .iter()
            .all(|issue| issue.message.len() < MAX_IDENTIFIER_BYTES)
    );
    assert!(
        preview
            .issues
            .iter()
            .all(|issue| issue.message.contains('…'))
    );

    vmf.entities[1]
        .properties
        .set("targetname", "z".repeat(MAX_IDENTIFIER_BYTES));
    vmf.entities[0]
        .properties
        .set("NextKey", "z".repeat(MAX_IDENTIFIER_BYTES - 1));
    let preview = prepare(&vmf, |_| true);
    assert!(preview.links.is_empty());
    assert!(preview.issues[0].message.contains("no authored target"));
    assert!(preview.issues[0].message.len() < MAX_IDENTIFIER_BYTES);
}

#[test]
fn ambiguous_target_identity_and_excess_origin_coordinates_never_generate_a_span() {
    let mut vmf = fixture();
    vmf.entities[1]
        .properties
        .0
        .push(("TARGETNAME".into(), "different_name".into()));
    let preview = prepare(&vmf, |_| true);
    assert!(preview.links.is_empty());
    assert!(
        preview
            .issues
            .iter()
            .any(|issue| issue.entity == 2 && issue.message.contains("targetname has ambiguous"))
    );

    let mut vmf = fixture();
    vmf.entities[0]
        .properties
        .set("origin", "0 0 128 ".repeat(32_768));
    let preview = prepare(&vmf, |_| true);
    assert!(preview.links.is_empty());
    assert_eq!(
        preview.issues[0].message,
        "Rope endpoint needs exactly three finite origin coordinates"
    );
}
