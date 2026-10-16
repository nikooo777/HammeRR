use super::*;
use source_formats::fgd::Catalog;

const FGD: &str = "@BaseClass line(12 34 56,targetname,aim) line(65 43 21,targetname,start,targetname,finish) = shared [ aim(target_destination) : \"Target\" : \"End\" ] @PointClass base(shared) = sample [ start(string) : \"Start\" : \"Start\" finish(string) : \"End\" : \"End\" ]";

fn entity(id: u32, name: &str, origin: &str) -> Entity {
    let mut entity = Entity::new(id, "sample");
    entity.properties.set("origin", origin);
    entity.properties.set("targetname", name);
    entity
}

fn rows(source: &Entity, targets: &[Entity]) -> Vec<Line> {
    let catalog = Catalog::parse(FGD).unwrap();
    resolve(
        catalog.class("sample").unwrap(),
        source,
        &Targets::new(targets, &catalog),
    )
}

#[test]
fn single_pair_uses_self_and_double_pair_uses_named_endpoints_with_literal_colors() {
    let mut source = entity(1, "owner", "10 20 30");
    source.properties.set("rendercolor", "255 0 0");
    let targets = [
        entity(2, "Start", "-20 40 60"),
        entity(3, "End", "80 100 120"),
    ];
    let resolved = rows(&source, &targets);
    assert_eq!(resolved.len(), 2);
    assert!(resolved.iter().all(|line| line.state == "ready"));
    assert_eq!(resolved[0].color, Some([12, 34, 56]));
    assert_eq!(resolved[0].endpoints[0].position, Some([10.0, 20.0, 30.0]));
    assert_eq!(resolved[0].endpoints[0].value_source, "self_origin");
    assert_eq!(resolved[0].endpoints[1].value_source, "fgd_default");
    assert_eq!(resolved[0].endpoints[1].entity, Some(3));
    assert_eq!(resolved[1].color, Some([65, 43, 21]));
    assert_eq!(
        resolved[1].endpoints.each_ref().map(|point| point.entity),
        [Some(2), Some(3)]
    );
    source.properties.set("AIM", "Start");
    let changed = rows(&source, &targets);
    assert_eq!(changed[0].endpoints[1].value_source, "authored");
    assert_eq!(changed[0].endpoints[1].entity, Some(2));
    assert_eq!(changed[1].endpoints[1].entity, Some(3));
}

#[test]
fn ambiguous_hidden_case_and_duplicate_fields_never_choose_a_substitute_endpoint() {
    let mut source = entity(1, "owner", "0 0 0");
    let mut targets = vec![entity(2, "End", "10 0 0")];
    for (value, expected) in [
        ("Missing", "missing"),
        ("end", "unsupported_case_match"),
        ("End*", "unsupported_reference"),
        ("!self", "unsupported_reference"),
    ] {
        source.properties.set("aim", value);
        let row = rows(&source, &targets).remove(0);
        assert_eq!(row.state, expected, "{value}");
        assert!(row.endpoints[1].position.is_none());
    }
    source.properties.set("aim", "End");
    let mut duplicate = entity(3, "end", "20 0 0");
    duplicate.hidden = true;
    targets.push(duplicate);
    let row = rows(&source, &targets).remove(0);
    assert_eq!(row.state, "ambiguous");
    assert_eq!(row.endpoints[1].matches, 2);
    targets.pop();
    source.properties.0.push(("AIM".into(), "End".into()));
    assert_eq!(rows(&source, &targets)[0].state, "duplicate_property");
    source.properties.0.pop();
    targets[0]
        .properties
        .0
        .push(("TARGETNAME".into(), "End".into()));
    assert_eq!(rows(&source, &targets)[0].state, "duplicate_targetname");
}

#[test]
fn unsupported_syntax_limits_and_invalid_native_origins_remain_diagnostic() {
    let source = entity(1, "owner", "0 0 0");
    let mut target = entity(2, "End", "10 0 0");
    for raw in ["NaN 0 0", "0 0", "65537 0 0"] {
        target.properties.set("origin", raw);
        assert_eq!(rows(&source, &[target.clone()])[0].state, "invalid_origin");
    }
    target.properties.set("origin", "0 0 0");
    assert_eq!(rows(&source, &[target.clone()])[0].state, "coincident");
    target.properties.set("origin", "1e-50 0 0");
    assert_eq!(rows(&source, &[target.clone()])[0].state, "coincident");
    target.properties.0.push(("ORIGIN".into(), "10 0 0".into()));
    assert_eq!(rows(&source, &[target.clone()])[0].state, "invalid_origin");
    for declaration in [
        "line()",
        "line(256 0 0,targetname,aim)",
        "line(1.5 2 3,targetname,aim)",
        "line(1 2 3,nodeid,aim)",
        "line(1 2 3,targetname,absent)",
    ] {
        let catalog = Catalog::parse(&format!(
            "@PointClass {declaration} = sample [ aim(string) : \"Target\" ]"
        ))
        .unwrap();
        let row = resolve(
            catalog.class("sample").unwrap(),
            &source,
            &Targets::new(&[], &catalog),
        )
        .remove(0);
        assert_eq!(row.state, "unsupported", "{declaration}");
        assert!(row.color.is_none());
    }
    let mut oversized = source.clone();
    oversized.properties.set("aim", "é".repeat(1024));
    let row = rows(&oversized, &[]).remove(0);
    assert!(row.endpoints[1].raw_truncated);
    assert_eq!(
        row.endpoints[1].raw.as_ref().unwrap().len(),
        MAX_VALUE_BYTES
    );
    assert_eq!(row.state, "unsupported_reference");
    let mut many = entity(2, "End", "10 0 0");
    many.properties
        .0
        .extend((0..MAX_TARGET_ENTITIES).map(|_| ("targetname".into(), "End".into())));
    assert_eq!(rows(&source, &[many])[0].state, "entity_limit");
    let helpers = (0..20)
        .map(|i| format!("line({i} 0 0,targetname,aim)"))
        .collect::<Vec<_>>()
        .join(" ");
    let catalog = Catalog::parse(&format!(
        "@PointClass {helpers} = sample [ aim(string) : \"Target\" ]"
    ))
    .unwrap();
    let rows = resolve(
        catalog.class("sample").unwrap(),
        &source,
        &Targets::new(&[], &catalog),
    );
    assert_eq!(rows.len(), MAX_LINES_PER_ENTITY + 1);
    assert!(
        rows.last()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("omitted")
    );
}

const NODE_FGD: &str = "@PointClass line(12 34 56,nodeid,aim) line(65 43 21,nodeid,start,nodeid,finish) = link [ aim(node_dest) : \"Target\" : 42 start(node_dest) : \"Start\" : 0 finish(node_dest) : \"End\" : 42 ] @PointClass = node [ nodeid(integer) readonly : \"Node ID\" ]";

fn node_rows(source: &Entity, targets: &[Entity]) -> Vec<Line> {
    let catalog = Catalog::parse(NODE_FGD).unwrap();
    resolve(
        catalog.class("link").unwrap(),
        source,
        &Targets::new(targets, &catalog),
    )
}

fn node(id: u32, node_id: &str, origin: &str) -> Entity {
    let mut result = Entity::new(id, "node");
    result.properties.set("nodeid", node_id);
    result.properties.set("origin", origin);
    result
}

#[test]
fn authored_nodeid_zero_and_defaults_resolve_without_object_or_runtime_ids() {
    let mut source = entity(10, "owner", "0 0 0");
    let targets = [node(500, "0", "-10 0 0"), node(600, "42", "10 0 0")];
    let rows = node_rows(&source, &targets);
    assert!(rows.iter().all(|row| row.state == "ready"));
    assert_eq!(rows[0].endpoints[0].lookup_property, None);
    assert_eq!(rows[0].endpoints[1].lookup_property, Some("nodeid"));
    assert_eq!(rows[0].endpoints[1].value_source, "fgd_default");
    assert_eq!(
        rows[1].endpoints.each_ref().map(|endpoint| endpoint.entity),
        [Some(500), Some(600)]
    );
    source.properties.set("aim", "0");
    let row = node_rows(&source, &targets).remove(0);
    assert_eq!(row.endpoints[1].entity, Some(500));
    assert_eq!(row.endpoints[1].value_source, "authored");
    for (raw, expected) in [
        ("-1", "no_association"),
        ("500", "missing"),
        ("01", "unsupported_reference"),
        ("+0", "unsupported_reference"),
        ("-2", "unsupported_reference"),
        ("2147483648", "unsupported_reference"),
    ] {
        source.properties.set("aim", raw);
        let row = node_rows(&source, &targets).remove(0);
        assert_eq!(row.state, expected, "{raw}");
        assert!(row.endpoints[1].position.is_none());
    }
    source.properties.set("aim", "2147483647");
    assert_eq!(
        node_rows(&source, &[node(2, "2147483647", "1 2 3")])[0].state,
        "ready"
    );
}

#[test]
fn nodeid_aliases_duplicates_invalid_ownership_and_raw_lookup_budget_are_explicit() {
    let mut source = entity(10, "owner", "0 0 0");
    source.properties.set("aim", "42");
    let mut targets = vec![node(500, "42", "10 0 0")];
    let mut alias = node(600, "+42", "20 0 0");
    alias.hidden = true;
    targets.push(alias);
    let row = node_rows(&source, &targets).remove(0);
    assert_eq!(row.state, "ambiguous");
    assert_eq!(row.endpoints[1].matches, 2);
    targets.remove(0);
    assert_eq!(node_rows(&source, &targets)[0].state, "invalid_nodeid");
    targets[0].properties.set("nodeid", "42");
    targets[0].properties.0.push(("NODEID".into(), "42".into()));
    assert_eq!(node_rows(&source, &targets)[0].state, "duplicate_nodeid");
    targets[0].properties.0.pop();
    targets[0].properties.set("classname", "undeclared_node");
    assert_eq!(node_rows(&source, &targets)[0].state, "missing");
    targets[0].properties.set("classname", "node");
    targets[0].properties.set("origin", "NaN 0 0");
    assert_eq!(node_rows(&source, &targets)[0].state, "invalid_origin");
    targets[0]
        .properties
        .0
        .extend((0..MAX_TARGET_ENTITIES).map(|_| ("nodeid".into(), "invalid".into())));
    assert_eq!(node_rows(&source, &targets)[0].state, "entity_limit");
    source.properties.0.push(("AIM".into(), "42".into()));
    assert_eq!(node_rows(&source, &[])[0].state, "duplicate_property");
}

#[test]
fn nodeid_binding_requires_declared_ownership_and_can_mix_with_named_endpoints() {
    let source = entity(10, "owner", "0 0 0");
    let target = node(600, "42", "10 0 0");
    for definition in [
        NODE_FGD.replace("nodeid(integer) readonly", "nodeid(integer)"),
        NODE_FGD.replace("nodeid(integer) readonly", "nodeid(string) readonly"),
        NODE_FGD.replace("@PointClass = node", "@SolidClass = node"),
    ] {
        let catalog = Catalog::parse(&definition).unwrap();
        assert_eq!(
            resolve(
                catalog.class("link").unwrap(),
                &source,
                &Targets::new(&[target.clone()], &catalog)
            )[0]
            .state,
            "missing"
        );
    }
    let catalog = Catalog::parse("@PointClass line(1 2 3,targetname,start,nodeid,finish) = link [ start(string) : \"Start\" : \"Named\" finish(node_dest) : \"Finish\" : 42 ] @PointClass = node [ nodeid(integer) readonly : \"Node ID\" ]").unwrap();
    let targets = [entity(500, "Named", "-10 0 0"), target];
    let rows = resolve(
        catalog.class("link").unwrap(),
        &source,
        &Targets::new(&targets, &catalog),
    );
    assert_eq!(rows[0].state, "ready");
    assert_eq!(
        rows[0]
            .endpoints
            .each_ref()
            .map(|endpoint| endpoint.lookup_property),
        [Some("targetname"), Some("nodeid")]
    );
    assert_eq!(
        rows[0].endpoints.each_ref().map(|endpoint| endpoint.entity),
        [Some(500), Some(600)]
    );
}
