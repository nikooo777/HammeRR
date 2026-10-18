use source_formats::keyvalues::Node;
use source_formats::vmf::Vmf;

#[test]
fn quoted_unknown_block_names_roundtrip_at_root_and_world_scope() {
    for key in [
        "vendor extension",
        "//vendor",
        "/*vendor",
        "vendor{extension",
        "",
        "tab\tkey",
    ] {
        let source = format!(
            "world {{ id 1 \"{key}\" {{ data keep }} }} \"{key}\" {{ nested {{ data keep }} }}"
        );
        let vmf = Vmf::parse(&source).unwrap();
        let saved = vmf.try_write(&Default::default()).unwrap();
        assert_eq!(Vmf::parse(&saved).unwrap(), vmf, "{key:?}");
    }
}

#[test]
fn vmf_rejects_excessive_nesting_before_recursive_conversion_or_cleanup() {
    use source_formats::keyvalues::{self, ParseErrorKind};
    for suffix in ["data keep", "incomplete", ""] {
        let text = format!(
            "world {{ id 1 }} {}{suffix}{}",
            "vendor { ".repeat(65_536),
            "} ".repeat(65_536)
        );
        let error = Vmf::parse(&text).unwrap_err();
        assert!(
            matches!(error, source_formats::vmf::Error::Syntax(error) if error.kind == ParseErrorKind::NestingLimit)
        );
    }
    let text = format!(
        "world {{ id 1 }} {}data keep{}",
        "vendor { ".repeat(keyvalues::MAX_NESTING),
        "} ".repeat(keyvalues::MAX_NESTING)
    );
    let vmf = Vmf::parse(&text).unwrap();
    assert_eq!(
        Vmf::parse(&vmf.try_write(&Default::default()).unwrap()).unwrap(),
        vmf
    );
    let nodes = [Node::block(
        "world",
        vec![
            Node::string("id", "1"),
            Node::string("message", "hello \"world\""),
        ],
    )];
    assert!(Vmf::from_nodes(&nodes).is_err());
    let mut vmf = Vmf::parse("world { id 1 }").unwrap();
    vmf.world.properties.set("message", "hello \"world\"");
    assert_eq!(
        vmf.try_write(&Default::default()),
        Err(keyvalues::WriteError::Quote)
    );
}

#[test]
fn unknown_root_visgroup_children_survive_edits_and_repeated_round_trips() {
    let source = r#"
        world { id 1 }
        visgroups {
            vendor_extension { data keep_me nested { value "with spaces" } }
            vendor_key preserve_me
            visgroup "unknown scalar"
            visgroup {
                name Existing visgroupid 7 color "100 120 140"
                custom_property keep_too
                custom_block { data keep_nested }
            }
        }
    "#;
    let mut vmf = Vmf::parse(source).unwrap();
    let expected = vmf.visgroups_extra.clone();
    assert_eq!(expected.len(), 3);
    assert_eq!(expected[1], Node::string("vendor_key", "preserve_me"));
    assert_eq!(expected[2], Node::string("visgroup", "unknown scalar"));
    vmf.visgroups.as_mut().unwrap()[0].name = "Renamed".into();
    for _ in 0..3 {
        let saved = vmf.write(&Default::default());
        let reopened = Vmf::parse(&saved).unwrap();
        assert_eq!(reopened, vmf);
        assert_eq!(reopened.visgroups_extra, expected);
        assert!(saved.contains("keep_me") && saved.contains("keep_nested"));
        vmf = reopened;
    }
    vmf.visgroups = None;
    let reopened = Vmf::parse(&vmf.write(&Default::default())).unwrap();
    assert_eq!(reopened.visgroups_extra, expected);
    assert!(reopened.visgroups.unwrap().is_empty());
}
