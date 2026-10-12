use source_formats::keyvalues::Node;
use source_formats::vmf::Vmf;

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
