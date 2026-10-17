mod common;

use compiler::{LightingMode, Options, VisibilityMode};
use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::bsp::{
    self, Bsp, Leaf, LeafAmbient, LightingKind, Model, Node, VisibilityKind,
};
use source_formats::vmf::{Entity, Vmf};

fn fixture() -> Document {
    let mut source = common::room();
    source.entities[0].properties.set("origin", "-200 0 8");
    source.entities[1].properties.set("origin", "200 0 8");
    let mut document = Document::new(source);
    document
        .edit("partition rooms", |tx| {
            for (mins, maxs) in [
                ([-72.0, -256.0, 0.0], [-56.0, -64.0, 256.0]),
                ([-72.0, 64.0, 0.0], [-56.0, 256.0, 256.0]),
                ([120.0, -256.0, 0.0], [136.0, 256.0, 256.0]),
            ] {
                edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "dev/dev_measuregeneric01",
                )?;
            }
            Ok(())
        })
        .unwrap();
    document
}

fn helper(document: &mut Document, mins: DVec3, maxs: DVec3) -> u32 {
    let object = document
        .edit("author viscluster", |tx| {
            let ObjectId::Solid(id) = edit::create_block(tx, mins, maxs, "tools/toolstrigger")?
            else {
                unreachable!()
            };
            edit::tie_to_entity(tx, &[id], "func_viscluster", &[])
        })
        .unwrap();
    let ObjectId::Entity(id) = object else {
        unreachable!()
    };
    id
}

fn grouped(document: &mut Document) -> u32 {
    helper(
        document,
        DVec3::new(-250.0, -250.0, 8.0),
        DVec3::new(110.0, 250.0, 248.0),
    )
}

fn compile(source: &Vmf, options: Options) -> compiler::Output {
    let output = compiler::compile_with_options(source, &common::empty_game(), options);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    output
}

fn leaf_at(bsp: &Bsp, point: DVec3) -> usize {
    let nodes = bsp.records::<Node>().unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let mut node = bsp.records::<Model>().unwrap()[0].head_node;
    while node >= 0 {
        let current = nodes[node as usize];
        let plane = planes[current.plane as usize];
        let distance =
            point.dot(DVec3::from_array(plane.normal.map(f64::from))) - f64::from(plane.dist);
        node = current.children[usize::from(distance < 0.0)];
    }
    (-1 - node) as usize
}

fn bit(row: &[u8], index: usize) -> bool {
    row[index / 8] & (1 << (index % 8)) != 0
}

#[test]
fn native_bsp_quotient_preserves_geometry_visibility_and_unaffected_occlusion() {
    for visibility in [VisibilityMode::Fast, VisibilityMode::Detailed] {
        let mut document = fixture();
        let options = Options {
            visibility,
            ..Default::default()
        };
        let baseline = compile(document.vmf(), options);
        let helper_id = grouped(&mut document);
        document
            .edit("irrelevant helper marker", |tx| {
                tx.entity_mut(helper_id)?
                    .properties
                    .set("origin", "4096 0 64");
                Ok(())
            })
            .unwrap();
        let source = document.vmf().clone();
        let output = compile(&source, options);
        assert_eq!(document.vmf(), &source);
        assert_eq!(
            output.stats.visibility_solved_clusters,
            baseline.stats.clusters
        );
        assert!(output.stats.clusters < baseline.stats.clusters);
        assert_eq!(output.stats.viscluster_helpers, 1);
        assert_eq!(output.stats.viscluster_groups, 1);
        assert!(output.stats.viscluster_grouped_leaves >= 2);
        assert_eq!(
            output.stats.visibility_portals,
            baseline.stats.visibility_portals
        );
        let baseline = baseline.bsp.unwrap();
        let result = output.bsp.unwrap();
        for (index, lump) in baseline.lumps.iter().enumerate() {
            if ![bsp::lump::LEAFS, bsp::lump::VISIBILITY].contains(&index) {
                assert_eq!(lump, &result.lumps[index], "lump {index}");
            }
        }
        assert!(
            !String::from_utf8_lossy(&result.lumps[bsp::lump::ENTITIES].data)
                .contains("func_viscluster")
        );
        let old_leaves = baseline.records::<Leaf>().unwrap();
        let new_leaves = result.records::<Leaf>().unwrap();
        assert_eq!(old_leaves.len(), new_leaves.len());
        let old_vis = baseline.visibility().unwrap().unwrap();
        let new_vis = result.visibility().unwrap().unwrap();
        assert_eq!(new_vis.clusters(), output.stats.clusters);
        for (a, old_a) in old_leaves
            .iter()
            .enumerate()
            .filter(|(_, leaf)| leaf.cluster >= 0)
        {
            let new_a = &new_leaves[a];
            assert!(new_a.cluster >= 0 && (new_a.cluster as usize) < new_vis.clusters());
            let mut normalized = *new_a;
            normalized.cluster = old_a.cluster;
            assert_eq!(normalized, *old_a);
            for kind in [VisibilityKind::Visible, VisibilityKind::Audible] {
                let old = old_vis.row(old_a.cluster as usize, kind).unwrap();
                let new = new_vis.row(new_a.cluster as usize, kind).unwrap();
                for (b, old_b) in old_leaves
                    .iter()
                    .enumerate()
                    .filter(|(_, leaf)| leaf.cluster >= 0)
                {
                    if bit(&old, old_b.cluster as usize) {
                        assert!(
                            bit(&new, new_leaves[b].cluster as usize),
                            "lost {a} -> {b} {kind:?}"
                        );
                    }
                }
            }
        }
        let left = new_leaves[leaf_at(&result, DVec3::new(-200.0, 0.0, 64.0))].cluster as usize;
        let middle = new_leaves[leaf_at(&result, DVec3::new(0.0, 0.0, 64.0))].cluster as usize;
        let right = new_leaves[leaf_at(&result, DVec3::new(200.0, 0.0, 64.0))].cluster as usize;
        assert_eq!(left, middle);
        assert_ne!(left, right);
        for kind in [VisibilityKind::Visible, VisibilityKind::Audible] {
            assert!(!bit(&new_vis.row(left, kind).unwrap(), right));
            assert!(!bit(&new_vis.row(right, kind).unwrap(), left));
        }
    }
}

#[test]
fn hidden_and_no_effect_helpers_preserve_native_bytes() {
    let game = common::empty_game();
    for lighting in [LightingMode::Fullbright, LightingMode::DirectLdrHdr] {
        let options = Options {
            lighting,
            ..Default::default()
        };
        let mut document = Document::new(common::room());
        let baseline = compile(document.vmf(), options).bsp.unwrap().to_bytes();
        let id = helper(
            &mut document,
            DVec3::new(1000.0, 1000.0, 1000.0),
            DVec3::new(1100.0, 1100.0, 1100.0),
        );
        let output = compile(document.vmf(), options);
        assert_eq!(output.bsp.unwrap().to_bytes(), baseline);
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains("does not reduce"))
        );
        document
            .edit("hide helper", |tx| {
                tx.entity_mut(id)?.hidden = true;
                Ok(())
            })
            .unwrap();
        let mut source = document.vmf().clone();
        let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
        let mut instance = Entity::new(id, "func_instance");
        instance.hidden = true;
        instance.properties.set("file", "unavailable.vmf");
        source.entities.push(instance);
        let output = compiler::compile_with_options(&source, &game, options);
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.bsp.unwrap().to_bytes(), baseline);
    }
}

#[test]
fn grouped_ldr_and_hdr_probes_belong_to_each_exact_emitted_leaf() {
    let mut document = fixture();
    document
        .edit("ambient lights", |tx| {
            for (origin, color) in [
                (DVec3::new(-180.0, 0.0, 128.0), "255 0 0 200"),
                (DVec3::new(0.0, 0.0, 128.0), "0 0 255 200"),
            ] {
                edit::create_entity(
                    tx,
                    "light",
                    Some(origin),
                    &[
                        ("_light".into(), color.into()),
                        ("_lightHDR".into(), color.into()),
                        ("_constant_attn".into(), "1".into()),
                        ("_quadratic_attn".into(), "0".into()),
                    ],
                    Vec::new(),
                )?;
            }
            Ok(())
        })
        .unwrap();
    let mut source = document.vmf().clone();
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 64;
    }
    document = Document::new(source);
    let options = Options {
        lighting: LightingMode::DirectLdrHdr,
        ..Default::default()
    };
    let baseline = compile(document.vmf(), options).bsp.unwrap();
    grouped(&mut document);
    let result = compile(document.vmf(), options).bsp.unwrap();
    let leaves = result.records::<Leaf>().unwrap();
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let LeafAmbient::Indexed {
            indices: old_indices,
            ..
        } = baseline.leaf_ambient(kind).unwrap().unwrap()
        else {
            unreachable!()
        };
        let LeafAmbient::Indexed { indices, samples } = result.leaf_ambient(kind).unwrap().unwrap()
        else {
            unreachable!()
        };
        let mut sampled_by_cluster = std::collections::HashMap::<i16, usize>::new();
        for (owner, index) in indices.iter().enumerate() {
            if old_indices[owner].sample_count > 0 {
                assert!(index.sample_count > 0, "lost ambient leaf {owner}");
            }
            for sample in &samples[index.first_sample as usize
                ..index.first_sample as usize + index.sample_count as usize]
            {
                let point = DVec3::from_array(sample.world_position(&leaves[owner]).unwrap());
                assert_eq!(
                    leaf_at(&result, point),
                    owner,
                    "probe assigned to another leaf"
                );
            }
            if index.sample_count > 0 {
                *sampled_by_cluster.entry(leaves[owner].cluster).or_default() += 1;
            }
        }
        assert!(sampled_by_cluster.values().any(|&members| members >= 2));
    }
}

#[test]
fn ambiguity_invalid_geometry_regions_and_cancellation_preserve_publication() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("visclusters.bsp");
    let lin = path.with_extension("lin");
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(&lin, b"previous pointfile").unwrap();
    let mut document = Document::new(common::room());
    helper(
        &mut document,
        DVec3::new(-256.0, -256.0, 0.0),
        DVec3::new(-128.0, 256.0, 256.0),
    );
    helper(
        &mut document,
        DVec3::new(128.0, -256.0, 0.0),
        DVec3::new(256.0, 256.0, 256.0),
    );
    let output = compiler::compile(document.vmf(), &common::empty_game());
    assert!(output.report.has_errors());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|item| item.message.contains("both qualify"))
    );
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(std::fs::read(&lin).unwrap(), b"previous pointfile");

    let mut uncertain = Document::new(common::room());
    helper(
        &mut uncertain,
        DVec3::new(-256.0, -256.0, 0.0),
        DVec3::new(-204.8, 256.0, 256.0),
    );
    let output = compiler::compile(uncertain.vmf(), &common::empty_game());
    assert!(output.report.has_errors());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|item| item.message.contains("strict 10%"))
    );
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(std::fs::read(&lin).unwrap(), b"previous pointfile");

    let mut source = common::room();
    let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
    source.entities.push(Entity::new(id, "func_viscluster"));
    let output = compiler::compile(&source, &common::empty_game());
    assert!(output.report.has_errors());
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");

    let mut recovered = fixture();
    grouped(&mut recovered);
    let output = compile(recovered.vmf(), Options::default());
    let control = compiler::Control::default();
    control.cancel().unwrap();
    assert!(output.write_controlled(&path, &control).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(std::fs::read(&lin).unwrap(), b"previous pointfile");
    assert!(output.write(&path).unwrap());
    assert_ne!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(std::fs::read(&lin).unwrap(), b"previous pointfile");

    let mut region = recovered.vmf().clone();
    region.cordon = Some(source_formats::keyvalues::Node::block(
        "cordon",
        vec![
            source_formats::keyvalues::Node::string("active", "1"),
            source_formats::keyvalues::Node::string("mins", "-128 -128 0"),
            source_formats::keyvalues::Node::string("maxs", "128 128 256"),
        ],
    ));
    let materials = directory.path().join("materials/tools");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(materials.join("toolsblack.vmt"), "UnlitGeneric { }").unwrap();
    let mut game = common::empty_game();
    game.mounts.push(source_formats::gamefs::MountedPath {
        ids: vec!["game".into()],
        mount: source_formats::gamefs::Mount::Directory(directory.path().to_owned()),
    });
    let output = compiler::compile(&region, &game);
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|item| item.message.contains("do not support func_viscluster")),
        "{:?}",
        output.report
    );
    assert!(output.bsp.is_none());
}
