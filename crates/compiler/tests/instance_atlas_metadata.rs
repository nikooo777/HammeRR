mod common;

use std::path::Path;

use compiler::instances::{self, SourceContext};
use compiler::{Control, Options, Report};
use glam::DVec3;
use map_core::texture_atlas::regions::{KEY, Region, Regions};
use map_core::texture_atlas::{FitRegion, Rectangle};
use map_core::{Document, ObjectId, edit};
use sha2::{Digest, Sha256};
use source_formats::keyvalues::Node;
use source_formats::vmf::{Entity, Vmf};

fn write(directory: &Path, name: &str, vmf: &Vmf) -> Vec<u8> {
    let bytes = vmf.write(&Default::default()).into_bytes();
    std::fs::write(directory.join(name), &bytes).unwrap();
    bytes
}

fn reference(vmf: &mut Vmf, id: u32, file: &str, origin: &str) {
    let mut helper = Entity::new(id, "func_instance");
    helper.properties.set("file", file);
    helper.properties.set("origin", origin);
    vmf.entities.push(helper);
}

fn with_catalog(vmf: Vmf, name: &str) -> Vmf {
    let mut document = Document::new(vmf);
    let catalog = Regions {
        regions: vec![Region {
            id: 1,
            name: name.into(),
            material: "unmounted/atlas_metadata_only".into(),
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
            extra: vec![
                Node::block(
                    "entry_extension",
                    vec![Node::string("opaque", "$unresolved")],
                ),
                Node::block("dispinfo", Vec::new()),
            ],
        }],
        extra: vec![
            Node::string("catalog_extension", "preserved original bytes"),
            Node::block(
                "hidden",
                vec![Node::block(
                    "dispinfo",
                    vec![Node::string("power", "opaque")],
                )],
            ),
        ],
    };
    document
        .edit("saved atlas region", |tx| catalog.write(tx))
        .unwrap();
    document.into_vmf()
}

fn fixture() -> (tempfile::TempDir, Vmf, Vmf, Vmf) {
    let retained = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/validation/846-child-atlas-metadata");
    std::fs::create_dir_all(&retained).unwrap();
    let directory = tempfile::tempdir_in(retained).unwrap();
    let mut leaf = Document::empty();
    leaf.edit("child world and detail", |tx| {
        edit::create_block(
            tx,
            DVec3::new(-32.0, -24.0, 16.0),
            DVec3::new(16.0, 24.0, 48.0),
            "dev/dev_measuregeneric01",
        )?;
        let ObjectId::Solid(detail) = edit::create_block(
            tx,
            DVec3::new(24.0, 16.0, 16.0),
            DVec3::new(56.0, 40.0, 32.0),
            "dev/dev_measuregeneric01",
        )?
        else {
            unreachable!()
        };
        edit::tie_to_entity(tx, &[detail], "func_detail", &[])?;
        Ok(())
    })
    .unwrap();
    let leaf = leaf.into_vmf();
    let mut middle = Document::empty().into_vmf();
    reference(&mut middle, 100, "leaf.vmf", "0 0 0");
    reference(&mut middle, 101, "leaf.vmf", "96 0 0");
    let mut parent = common::room();
    let id = parent.object_ids().max().unwrap() + 1;
    reference(&mut parent, id, "middle.vmf", "0 0 0");
    write(directory.path(), "leaf.vmf", &leaf);
    write(directory.path(), "middle.vmf", &middle);
    write(directory.path(), "parent.vmf", &parent);
    (directory, parent, middle, leaf)
}

fn compile(parent: &Vmf, source: &SourceContext) -> compiler::Output {
    let output = compiler::compile_with_source_controlled(
        parent,
        &common::empty_game(),
        Options::default(),
        None,
        Some(source),
        &Control::default(),
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(
        output.bsp.is_some() && output.leak.is_none(),
        "{:?}",
        output.report
    );
    output
}

#[test]
fn nested_child_atlas_catalogs_preserve_original_custody_and_exact_plain_child_bsp() {
    let (directory, parent, middle, leaf) = fixture();
    let context =
        SourceContext::capture(&directory.path().join("parent.vmf"), &Control::default()).unwrap();
    let plain = instances::resolve(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    let baseline = compile(&parent, &context);
    let original_parent = std::fs::read(directory.path().join("parent.vmf")).unwrap();
    let leaf = with_catalog(leaf, "Leaf \"été\" \\ region");
    let middle = with_catalog(middle, "Middle region");
    let source_bytes = [
        ("leaf.vmf", write(directory.path(), "leaf.vmf", &leaf)),
        ("middle.vmf", write(directory.path(), "middle.vmf", &middle)),
    ];
    let prepared = instances::resolve(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    assert_eq!(prepared.vmf(), plain.vmf());
    assert_eq!(prepared.vmf().extra, parent.extra);
    let stats = prepared.stats().unwrap();
    assert_eq!(
        (stats.instances, stats.files, stats.max_expansion_depth),
        (3, 2, 2)
    );
    assert_eq!((stats.world_brushes, stats.detail_brushes), (2, 2));
    for (name, bytes) in &source_bytes {
        let identity = stats
            .dependencies
            .iter()
            .find(|dependency| dependency.path == *name)
            .unwrap();
        assert_eq!(identity.bytes, bytes.len());
        assert_eq!(identity.sha256, format!("{:x}", Sha256::digest(bytes)));
        assert_eq!(std::fs::read(directory.path().join(name)).unwrap(), *bytes);
    }
    prepared
        .dependencies()
        .unwrap()
        .verify(&Control::default())
        .unwrap();
    let output = compile(&parent, &context);
    assert_eq!(
        output.bsp.as_ref().unwrap().to_bytes(),
        baseline.bsp.as_ref().unwrap().to_bytes()
    );
    assert!(
        !output
            .bsp
            .as_ref()
            .unwrap()
            .entities()
            .unwrap()
            .contains(KEY)
    );
    assert_eq!(
        std::fs::read(directory.path().join("parent.vmf")).unwrap(),
        original_parent
    );

    let destination = directory.path().join("previous.bsp");
    std::fs::write(&destination, b"previous BSP").unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous pointfile").unwrap();
    let staged = output
        .prepare_write_controlled(&destination, &Control::default())
        .unwrap()
        .unwrap();
    drop(output);
    let mut changed = leaf;
    let node = changed
        .extra
        .iter_mut()
        .find(|node| node.key == KEY)
        .unwrap();
    let source_formats::keyvalues::Value::Block(fields) = &mut node.value else {
        unreachable!()
    };
    *fields
        .iter_mut()
        .find(|field| field.key == "catalog_extension")
        .unwrap() = Node::string("catalog_extension", "metadata-only mutation");
    write(directory.path(), "leaf.vmf", &changed);
    assert!(
        prepared
            .dependencies()
            .unwrap()
            .verify(&Control::default())
            .is_err()
    );
    assert!(staged.commit(&Control::default()).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(destination.with_extension("lin")).unwrap(),
        b"previous pointfile"
    );
}

#[test]
fn child_atlas_catalogs_reject_malformed_duplicate_future_and_unknown_top_level_metadata() {
    let (directory, parent, _middle, leaf) = fixture();
    let context =
        SourceContext::capture(&directory.path().join("parent.vmf"), &Control::default()).unwrap();
    let mut valid = with_catalog(leaf, "Panel");
    let catalog = valid.extra.pop().unwrap();
    let mut duplicate_fields = catalog.children().unwrap().to_vec();
    duplicate_fields.push(Node::string("version", "1"));
    let mut invalid_padding = catalog.clone();
    let source_formats::keyvalues::Value::Block(fields) = &mut invalid_padding.value else {
        unreachable!()
    };
    let entry = fields.iter_mut().find(|node| node.key == "region").unwrap();
    let source_formats::keyvalues::Value::Block(fields) = &mut entry.value else {
        unreachable!()
    };
    *fields
        .iter_mut()
        .find(|node| node.key == "padding")
        .unwrap() = Node::string("padding", "8");
    for extras in [
        vec![Node::string(KEY, "not a block")],
        vec![Node::block(KEY, vec![Node::string("version", "2")])],
        vec![catalog.clone(), catalog.clone()],
        vec![Node::block(KEY, duplicate_fields)],
        vec![invalid_padding],
        vec![
            catalog.clone(),
            Node::block("unknown_map_metadata", Vec::new()),
        ],
        vec![Node::block(
            "HAMMERR_ATLAS_REGIONS",
            vec![Node::string("version", "1")],
        )],
    ] {
        let mut child = valid.clone();
        child.extra = extras;
        let bytes = write(directory.path(), "leaf.vmf", &child);
        let error = instances::resolve(
            &parent,
            Some(&context),
            &Control::default(),
            &mut Report::default(),
        )
        .err()
        .unwrap();
        assert!(
            error.message.contains("atlas region metadata")
                || error
                    .message
                    .contains("unknown or compiler-owned map metadata"),
            "{error:?}"
        );
        assert!(error.message.contains("leaf.vmf"), "{error:?}");
        assert_eq!(
            std::fs::read(directory.path().join("leaf.vmf")).unwrap(),
            bytes
        );
        assert!(parent.extra.is_empty());
    }
}
