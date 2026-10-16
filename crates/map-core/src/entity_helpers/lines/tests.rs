use super::*;
use source_formats::fgd::Catalog;

const FGD: &str = "@BaseClass line(12 34 56,targetname,aim) = shared [ aim(target_destination) : \"Target\" : \"End\" ] @PointClass base(shared) line(65 43 21,targetname,start,targetname,finish) = sample [ start(string) : \"Start\" : \"Start\" finish(string) : \"End\" : \"End\" ]";

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
        &Targets::new(targets),
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
            &Targets::new(&[]),
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
        &Targets::new(&[]),
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
