mod common;

use source_formats::{keyvalues::Node, vmf::Vmf};

fn imported(nodes: &[Node]) -> Vmf {
    let mut source = common::room().to_nodes();
    source.extend_from_slice(nodes);
    let text = source_formats::keyvalues::write(&source, &Default::default());
    Vmf::parse(&text).unwrap()
}

#[test]
fn invalid_or_ambiguous_imported_cordons_fail_before_publishing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, b"previous successful BSP").unwrap();
    let active = Node::string("active", "1");
    for nodes in [
        vec![Node::block("cordon", vec![active.clone()])],
        vec![Node::block("cordons", vec![Node::string("active", "true")])],
        vec![Node::block("CORDONS", vec![Node::string("Active", "TRUE")])],
        vec![
            Node::block("cordon", vec![Node::string("active", "0")]),
            Node::block("cordon", vec![active.clone()]),
        ],
        vec![Node::block("cordon", vec![Node::string("active", "maybe")])],
        vec![Node::block("cordon", vec![Node::block("active", vec![])])],
        vec![Node::block(
            "cordon",
            vec![Node::string("active", "0"), active],
        )],
        vec![Node::string("cordons", "1")],
    ] {
        let vmf = imported(&nodes);
        let before = vmf.clone();
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(output.report.has_errors(), "{:?}", output.report);
        assert!(output.bsp.is_none());
        assert_eq!(output.stats.brushes, 0);
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains("cordon"))
        );
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous successful BSP");
        assert_eq!(vmf, before);
    }
}

#[test]
fn disabled_cordons_preserve_full_map_bytes_and_metadata() {
    let fs = common::empty_game();
    let baseline = compiler::compile(&common::room(), &fs)
        .bsp
        .unwrap()
        .to_bytes();
    for nodes in [
        vec![Node::block("cordon", vec![Node::string("active", "0")])],
        vec![Node::block(
            "cordons",
            vec![
                Node::string("active", "false"),
                Node::block("cordon", vec![Node::string("active", "1")]),
            ],
        )],
        vec![Node::block(
            "cordon",
            vec![Node::string("mins", "(-64 -64 0)")],
        )],
        vec![Node::block("cordons", vec![])],
        vec![Node::block("custom", vec![Node::string("active", "1")])],
    ] {
        let vmf = imported(&nodes);
        let before = vmf.clone();
        let output = compiler::compile(&vmf, &fs);
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.bsp.unwrap().to_bytes(), baseline);
        assert_eq!(vmf, before);
    }
}
