mod common;

use glam::DVec3;
use map_core::{Document, edit};
use source_formats::keyvalues::Node;
use source_formats::vmf::{Entity, Vmf};

#[test]
fn visible_instance_helpers_fail_before_full_map_or_region_construction() {
    let directory = tempfile::tempdir().unwrap();
    let child = directory.path().join("referenced.vmf");
    let mut child_document = Document::empty();
    child_document
        .edit("referenced geometry", |tx| {
            edit::create_block(
                tx,
                DVec3::new(8.0, 16.0, 24.0),
                DVec3::new(40.0, 64.0, 88.0),
                "dev/instance-only",
            )?;
            Ok(())
        })
        .unwrap();
    let child_bytes = child_document.vmf().write(&Default::default());
    std::fs::write(&child, &child_bytes).unwrap();
    let game = common::empty_game();
    let baseline = compiler::compile(&common::room(), &game);
    assert!(baseline.bsp.is_some(), "{:?}", baseline.report);
    let bsp = directory.path().join("previous.bsp");
    assert!(baseline.write(&bsp).unwrap());
    let previous = std::fs::read(&bsp).unwrap();
    let lin = bsp.with_extension("lin");
    std::fs::write(&lin, b"previous pointfile").unwrap();

    for (classname, region) in [
        ("FuNc_InStAnCe", false),
        ("func_instance_parms", false),
        ("func_instance", true),
        ("FUNC_INSTANCE_PARMS", true),
    ] {
        let mut source = common::room();
        let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
        let mut entity = Entity::new(id, classname);
        entity.properties.set("origin", "4096 0 64");
        entity.properties.set("file", "referenced.vmf");
        entity.properties.set("replace01", "$name original");
        source.entities.push(entity);
        if region {
            source.cordon = Some(Node::block(
                "cordon",
                vec![
                    Node::string("active", "1"),
                    Node::string("mins", "-128 -128 0"),
                    Node::string("maxs", "128 128 256"),
                ],
            ));
        }
        let source_path = directory.path().join("parent.vmf");
        let source_bytes = source.write(&Default::default());
        std::fs::write(&source_path, &source_bytes).unwrap();
        let source = Vmf::parse(&std::fs::read_to_string(&source_path).unwrap()).unwrap();
        let before = source.clone();
        let output = compiler::compile(&source, &game);
        assert!(output.report.has_errors(), "{:?}", output.report);
        assert!(output.bsp.is_none());
        assert!(output.leak.is_none());
        assert_eq!(output.stats.brushes, 0);
        assert!(output.stats.compile_region.is_none());
        assert_eq!(output.report.diagnostics.len(), 1);
        let diagnostic = &output.report.diagnostics[0];
        assert_eq!(diagnostic.object, Some(format!("entity:{id}")));
        assert!(diagnostic.message.contains(classname));
        assert!(diagnostic.message.contains("instance expansion"));
        assert!(!output.write(&bsp).unwrap());
        assert_eq!(std::fs::read(&bsp).unwrap(), previous);
        assert_eq!(std::fs::read(&lin).unwrap(), b"previous pointfile");
        assert_eq!(std::fs::read_to_string(&child).unwrap(), child_bytes);
        assert_eq!(std::fs::read_to_string(&source_path).unwrap(), source_bytes);
        assert_eq!(source, before);
    }
}

#[test]
fn hidden_instances_keep_baseline_bsp_bytes_and_visclusters_remain_warnings() {
    let game = common::empty_game();
    let mut source = common::room();
    let baseline = compiler::compile(&source, &game).bsp.unwrap().to_bytes();
    for classname in ["func_instance", "FuNc_InStAnCe_PaRmS"] {
        let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
        let mut entity = Entity::new(id, classname);
        entity.hidden = true;
        entity.properties.set("file", "missing.vmf");
        entity.properties.set("origin", "NaN 0 0");
        source.entities.push(entity);
    }
    let source = Vmf::parse(&source.write(&Default::default())).unwrap();
    let before = source.clone();
    let output = compiler::compile(&source, &game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.bsp.unwrap().to_bytes(), baseline);
    assert_eq!(source, before);

    let mut source = common::room();
    let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
    source.entities.push(Entity::new(id, "FUNC_VISCLUSTER"));
    let output = compiler::compile(&source, &game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.bsp.unwrap().to_bytes(), baseline);
    assert!(output.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.level == compiler::Level::Warning
            && diagnostic.object == Some(format!("entity:{id}"))
            && diagnostic.message.contains("was left out")
    }));
}
