use super::*;
use crate::Document;

fn region(id: u32) -> Region {
    Region {
        id,
        name: format!("Region {id}"),
        material: "test/checker".into(),
        image_identity: "a".repeat(64),
        dimensions: [64, 32],
        region: FitRegion {
            rectangle: Rectangle {
                x: 16,
                y: 8,
                width: 32,
                height: 16,
            },
            padding: 2,
        },
        extra: vec![Node::string("future", "keep")],
    }
}

#[test]
fn named_regions_roundtrip_unicode_extensions_and_undo_without_changing_geometry() {
    let mut document = Document::empty();
    let before = document.vmf().clone();
    let mut first = region(1);
    first.name = "Panneau \"été\" \\ rouge".into();
    let saved = Regions {
        regions: vec![first, region(2)],
        extra: vec![Node::block(
            "future_root",
            vec![Node::string("opaque", "yes")],
        )],
    };
    document
        .edit("save regions", |tx| saved.clone().write(tx))
        .unwrap();
    let text = document.vmf().write(&Default::default());
    let reopened = Vmf::parse(&text).unwrap();
    assert_eq!(Regions::parse(&reopened).unwrap(), saved);
    assert_eq!(reopened.world, before.world);
    assert_eq!(reopened.entities, before.entities);
    let untouched = document
        .vmf()
        .extra
        .iter()
        .find(|node| node.key == KEY)
        .unwrap()
        .children()
        .unwrap()[2]
        .clone();
    let mut renamed = saved.clone();
    renamed.regions[0].name = "Renamed".into();
    document.edit("rename", |tx| renamed.write(tx)).unwrap();
    assert_eq!(
        document
            .vmf()
            .extra
            .iter()
            .find(|node| node.key == KEY)
            .unwrap()
            .children()
            .unwrap()[2],
        untouched
    );
    document.undo().unwrap();
    assert_eq!(Regions::parse(document.vmf()).unwrap(), saved);
    document.undo().unwrap();
    assert_eq!(document.vmf(), &before);
    document.redo().unwrap();
    assert_eq!(Regions::parse(document.vmf()).unwrap(), saved);
}

#[test]
fn named_regions_reject_duplicate_names_bad_sources_and_catalog_overflow() {
    let first = region(1);
    for invalid in [
        Region {
            name: "\n".into(),
            ..first.clone()
        },
        Region {
            name: "a".repeat(MAX_NAME_CHARS + 1),
            ..first.clone()
        },
        Region {
            material: "Materials/test/checker.vmt".into(),
            ..first.clone()
        },
        Region {
            material: "../checker".into(),
            ..first.clone()
        },
        Region {
            image_identity: "f".repeat(63),
            ..first.clone()
        },
        Region {
            dimensions: [8193, 32],
            ..first.clone()
        },
        Region {
            region: FitRegion {
                padding: 8,
                ..first.region
            },
            ..first.clone()
        },
    ] {
        assert!(invalid.validate().is_err());
    }
    let mut catalog = Regions {
        regions: vec![
            first.clone(),
            Region {
                id: 2,
                name: " REGION 1 ".into(),
                ..first.clone()
            },
        ],
        extra: Vec::new(),
    };
    assert!(catalog.validate().is_err());
    catalog.regions[1].material = "test/another".into();
    catalog.validate().unwrap();
    catalog.regions = (1..=MAX_REGIONS as u32).map(region).collect();
    catalog.validate().unwrap();
    catalog.regions.push(region(129));
    assert!(catalog.validate().is_err());
}

#[test]
fn malformed_or_future_catalogs_are_preserved_instead_of_overwritten() {
    let valid = Regions {
        regions: vec![region(1)],
        extra: Vec::new(),
    };
    for node in [
        Node::string(KEY, "opaque"),
        Node::block(KEY, vec![Node::string("version", "2")]),
        Node::block(
            KEY,
            vec![Node::string("version", "1"), Node::string("version", "1")],
        ),
        Node::block(
            KEY,
            vec![
                Node::string("version", "1"),
                Node::block(
                    "region",
                    vec![Node::string(
                        "name_base64",
                        "a".repeat(MAX_ENCODED_NAME_BYTES + 1),
                    )],
                ),
            ],
        ),
    ] {
        let mut vmf = Document::empty().vmf().clone();
        vmf.extra.push(node);
        let mut document = Document::new(vmf.clone());
        assert!(Regions::parse(document.vmf()).is_err());
        assert!(
            document
                .edit("overwrite", |tx| valid.clone().write(tx))
                .is_err()
        );
        assert_eq!(document.vmf(), &vmf);
        assert_eq!(document.undo_labels().count(), 0);
    }
}

fn metadata_bytes(node: &Node) -> usize {
    node.key.len()
        + node.as_str().map_or_else(
            || node.children().unwrap().iter().map(metadata_bytes).sum(),
            str::len,
        )
}

#[test]
fn full_catalog_budget_rejects_near_limit_writes_before_mutating_preserved_metadata() {
    for lexical_padding in [0, 64] {
        let mut first = region(1).to_node();
        let source_formats::keyvalues::Value::Block(fields) = &mut first.value else {
            unreachable!()
        };
        *fields.iter_mut().find(|field| field.key == "id").unwrap() =
            Node::string("id", format!("{}1", "0".repeat(lexical_padding)));
        let mut node = Node::block(
            KEY,
            vec![
                Node::string("version", "1"),
                first,
                Node::string("future", ""),
            ],
        );
        let addition = metadata_bytes(&region(2).to_node());
        let payload = crate::metadata::MAX_BYTES - metadata_bytes(&node) - addition + KEY.len() - 1;
        let source_formats::keyvalues::Value::Block(fields) = &mut node.value else {
            unreachable!()
        };
        *fields.last_mut().unwrap() = Node::string("future", "x".repeat(payload));
        let mut vmf = Document::empty().vmf().clone();
        vmf.extra.push(node);
        let mut document = Document::new(vmf.clone());
        let mut catalog = Regions::parse(document.vmf()).unwrap();
        catalog.regions.push(region(2));
        assert_eq!(catalog.validate().is_ok(), lexical_padding != 0);
        assert!(
            document
                .edit("over budget", |tx| catalog.write(tx))
                .is_err()
        );
        assert_eq!(document.vmf(), &vmf);
        assert_eq!(document.undo_labels().count(), 0);
        Regions::parse(document.vmf()).unwrap();
    }
}
