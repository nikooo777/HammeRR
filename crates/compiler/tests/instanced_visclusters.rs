mod common;

use std::collections::HashMap;
use std::path::Path;

use compiler::instances::{self, SourceContext};
use compiler::{Control, LightingMode, Options, VisibilityMode};
use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::bsp::{
    self, Bsp, Leaf, LeafAmbient, LightingKind, Model, Node, VisibilityKind,
};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::keyvalues::Node as KeyValue;
use source_formats::vmf::{Entity, Vmf};

fn write_source(directory: &Path, name: &str, source: &Vmf) -> Vec<u8> {
    let bytes = source.write(&Default::default()).into_bytes();
    std::fs::write(directory.join(name), &bytes).unwrap();
    bytes
}

fn saved_parent(directory: &Path, source: &Vmf) -> SourceContext {
    write_source(directory, "parent.vmf", source);
    SourceContext::capture(&directory.join("parent.vmf"), &Control::default()).unwrap()
}

fn instance(source: &mut Vmf, file: &str, origin: &str, angles: &str) -> u32 {
    let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
    let mut entity = Entity::new(id, "func_instance");
    entity.properties.set("file", file);
    entity.properties.set("origin", origin);
    entity.properties.set("angles", angles);
    source.entities.push(entity);
    id
}

fn helper(document: &mut Document, mins: [f64; 3], maxs: [f64; 3]) -> u32 {
    let object = document
        .edit("visibility helper", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::from_array(mins),
                DVec3::from_array(maxs),
                "tools/toolstrigger",
            )?
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

fn helper_file(mins: [f64; 3], maxs: [f64; 3]) -> Vmf {
    let mut document = Document::empty();
    helper(&mut document, mins, maxs);
    document.into_vmf()
}

fn region(source: &mut Vmf, mins: &str, maxs: &str) {
    source.cordon = Some(KeyValue::block(
        "cordon",
        vec![
            KeyValue::string("active", "1"),
            KeyValue::string("mins", mins),
            KeyValue::string("maxs", maxs),
        ],
    ));
}

fn region_game(directory: &Path) -> GameFs {
    let materials = directory.join("materials/tools");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(materials.join("toolsblack.vmt"), "UnlitGeneric { }").unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.to_owned()),
    });
    game
}

fn compile(source: &Vmf, game: &GameFs, options: Options) -> compiler::Output {
    let output = compiler::compile_with_options(source, game, options);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    output
}

fn compile_saved(
    source: &Vmf,
    game: &GameFs,
    options: Options,
    context: &SourceContext,
) -> compiler::Output {
    compiler::compile_with_source_controlled(
        source,
        game,
        options,
        None,
        Some(context),
        &Control::default(),
    )
}

fn fixture() -> Vmf {
    let mut source = common::room();
    source.entities[0].properties.set("origin", "-200 0 8");
    source.entities[1].properties.set("origin", "200 0 8");
    let mut document = Document::new(source);
    document
        .edit("partition rooms and light probes", |tx| {
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
    let mut source = document.into_vmf();
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 64;
    }
    source
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

fn assert_physical_leaves_visibility_and_probes(baseline: &Bsp, grouped: &Bsp) {
    for (index, lump) in baseline.lumps.iter().enumerate() {
        if ![
            bsp::lump::LEAFS,
            bsp::lump::VISIBILITY,
            bsp::lump::WORLDLIGHTS,
            bsp::lump::WORLDLIGHTS_HDR,
        ]
        .contains(&index)
        {
            assert_eq!(lump, &grouped.lumps[index], "physical lump {index}");
        }
    }
    let old_leaves = baseline.records::<Leaf>().unwrap();
    let leaves = grouped.records::<Leaf>().unwrap();
    assert_eq!(old_leaves.len(), leaves.len());
    let old_visibility = baseline.visibility().unwrap().unwrap();
    let visibility = grouped.visibility().unwrap().unwrap();
    let mut cluster_mapping = HashMap::new();
    for (a, old_a) in old_leaves
        .iter()
        .enumerate()
        .filter(|(_, leaf)| leaf.cluster >= 0)
    {
        let mut normalized = leaves[a];
        assert!(normalized.cluster >= 0);
        normalized.cluster = old_a.cluster;
        assert_eq!(normalized, *old_a);
        if let Some(previous) = cluster_mapping.insert(old_a.cluster, leaves[a].cluster) {
            assert_eq!(previous, leaves[a].cluster);
        }
        for kind in [VisibilityKind::Visible, VisibilityKind::Audible] {
            let old_row = old_visibility.row(old_a.cluster as usize, kind).unwrap();
            let row = visibility.row(leaves[a].cluster as usize, kind).unwrap();
            for (b, old_b) in old_leaves
                .iter()
                .enumerate()
                .filter(|(_, leaf)| leaf.cluster >= 0)
            {
                if bit(&old_row, old_b.cluster as usize) {
                    assert!(
                        bit(&row, leaves[b].cluster as usize),
                        "lost {a} -> {b} {kind:?}"
                    );
                }
            }
        }
    }
    let left = leaves[leaf_at(grouped, DVec3::new(-200.0, 0.0, 64.0))].cluster as usize;
    let middle = leaves[leaf_at(grouped, DVec3::new(0.0, 0.0, 64.0))].cluster as usize;
    let right = leaves[leaf_at(grouped, DVec3::new(200.0, 0.0, 64.0))].cluster as usize;
    assert_eq!(left, middle);
    assert_ne!(left, right);
    for kind in [VisibilityKind::Visible, VisibilityKind::Audible] {
        assert!(!bit(&visibility.row(left, kind).unwrap(), right));
        assert!(!bit(&visibility.row(right, kind).unwrap(), left));
    }
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let old_lights = baseline.world_lights(kind).unwrap();
        let lights = grouped.world_lights(kind).unwrap();
        assert!(!old_lights.is_empty());
        assert_eq!(old_lights.len(), lights.len());
        for (old_light, light) in old_lights.iter().zip(lights) {
            let old_leaf = leaf_at(baseline, DVec3::from_array(old_light.origin.map(f64::from)));
            let leaf = leaf_at(grouped, DVec3::from_array(light.origin.map(f64::from)));
            assert_eq!(old_leaf, leaf);
            assert_eq!(old_light.cluster, i32::from(old_leaves[old_leaf].cluster));
            assert_eq!(light.cluster, i32::from(leaves[leaf].cluster));
            assert_eq!(
                light.cluster,
                i32::from(cluster_mapping[&old_leaves[old_leaf].cluster])
            );
            let mut normalized = light;
            normalized.cluster = old_light.cluster;
            assert_eq!(normalized, *old_light, "world-light {kind:?} changed");
        }
        let world_lump = match kind {
            LightingKind::Ldr => bsp::lump::WORLDLIGHTS,
            LightingKind::Hdr => bsp::lump::WORLDLIGHTS_HDR,
        };
        let old_lump = &baseline.lumps[world_lump];
        let lump = &grouped.lumps[world_lump];
        assert_eq!(lump.version, old_lump.version);
        assert_eq!(lump.fourcc, old_lump.fourcc);
        assert_eq!(lump.data.len(), old_lump.data.len());
        let old_ambient = baseline.leaf_ambient(kind).unwrap().unwrap();
        let ambient = grouped.leaf_ambient(kind).unwrap().unwrap();
        assert_eq!(ambient, old_ambient, "per-leaf {kind:?} changed");
        let LeafAmbient::Indexed { indices, samples } = ambient else {
            unreachable!()
        };
        let mut sampled_by_cluster = HashMap::<i16, usize>::new();
        for (owner, index) in indices.iter().enumerate() {
            for sample in &samples[index.first_sample as usize
                ..index.first_sample as usize + index.sample_count as usize]
            {
                let point = DVec3::from_array(sample.world_position(&leaves[owner]).unwrap());
                assert_eq!(leaf_at(grouped, point), owner);
            }
            if index.sample_count > 0 {
                *sampled_by_cluster.entry(leaves[owner].cluster).or_default() += 1;
            }
        }
        assert!(sampled_by_cluster.values().any(|&members| members >= 2));
    }
}

fn assert_child_error(
    output: &compiler::Output,
    owner: u32,
    file: &str,
    helper: u32,
    reason: &str,
) {
    assert!(output.report.has_errors() && output.bsp.is_none() && output.leak.is_none());
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.object == Some(format!("entity:{owner}"))
                && diagnostic.message.contains(file)
                && diagnostic.message.contains(reason)
                && (diagnostic
                    .message
                    .contains(&format!("child object {helper}"))
                    || diagnostic
                        .message
                        .contains(&format!("Child viscluster {helper}")))
        }),
        "{:?}",
        output.report
    );
}

#[test]
fn translated_and_noncommuting_helpers_match_literal_controls_without_physical_geometry() {
    for nested in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let game = common::empty_game();
        let mut child = if nested {
            helper_file([-298.0, -104.0, -330.0], [202.0, 136.0, 30.0])
        } else {
            helper_file([-330.0, -266.0, -8.0], [30.0, 234.0, 232.0])
        };
        if !nested {
            child.entities[0].properties.set("origin", "4096 0 64");
        }
        for side in &mut child.entities[0].solids[0].sides {
            side.u_axis.offset = 3.25;
            side.v_axis.offset = -5.5;
        }
        let original = &child.entities[0];
        let child_bytes = write_source(directory.path(), "leaf.vmf", &child);
        let mut parent = fixture();
        let owner = if nested {
            let mut middle = Document::empty().into_vmf();
            instance(&mut middle, "leaf.vmf", "16 32 0", "0 90 0");
            write_source(directory.path(), "middle.vmf", &middle);
            instance(&mut parent, "middle.vmf", "80 16 128", "90 0 0")
        } else {
            instance(&mut parent, "leaf.vmf", "80 16 16", "0 0 0")
        };
        let before = parent.clone();
        let context = saved_parent(directory.path(), &parent);
        let mut report = compiler::Report::default();
        let prepared =
            instances::resolve(&parent, Some(&context), &Control::default(), &mut report).unwrap();
        assert!(!report.has_errors(), "{report:?}");
        let stats = prepared.stats().unwrap();
        assert_eq!(
            (stats.world_brushes, stats.detail_brushes, stats.visclusters),
            (0, 0, 1)
        );
        assert_eq!(stats.max_expansion_depth, if nested { 2 } else { 1 });
        assert_eq!(prepared.generated_brushes().count(), 0);
        let generated = prepared
            .vmf()
            .entities
            .iter()
            .find(|entity| entity.classname() == Some("func_viscluster"))
            .unwrap();
        assert_eq!(prepared.root_entity(generated.id), Some(owner));
        assert_eq!(generated.properties.get("angles"), None);
        if nested {
            assert_eq!(generated.properties.get("origin"), None);
        } else {
            assert_eq!(generated.origin(), Some(DVec3::new(4176.0, 16.0, 80.0)));
        }
        for (actual, source) in generated.solids[0]
            .sides
            .iter()
            .zip(&original.solids[0].sides)
        {
            for (point, original_point) in actual.plane.0.iter().zip(source.plane.0) {
                let expected = if nested {
                    DVec3::new(
                        original_point.z + 80.0,
                        original_point.x + 48.0,
                        original_point.y + 112.0,
                    )
                } else {
                    original_point + DVec3::new(80.0, 16.0, 16.0)
                };
                assert!(
                    point.abs_diff_eq(expected, 1e-8),
                    "{point:?} != {expected:?}"
                );
                assert!(
                    map_core::texel_uv(actual, expected)
                        .abs_diff_eq(map_core::texel_uv(source, original_point), 1e-8)
                );
            }
            let normal = map_core::Plane::from_points(source.plane.0).unwrap().normal;
            let expected = if nested {
                DVec3::new(normal.z, normal.x, normal.y)
            } else {
                normal
            };
            assert!(
                map_core::Plane::from_points(actual.plane.0)
                    .unwrap()
                    .normal
                    .abs_diff_eq(expected, 1e-8)
            );
            assert_eq!(
                prepared.face_origin(actual.id),
                Some((owner, "leaf.vmf", original.solids[0].id, source.id))
            );
        }
        let mut flat = Document::new(fixture());
        helper(&mut flat, [-250.0, -250.0, 8.0], [110.0, 250.0, 248.0]);
        for visibility in [VisibilityMode::Fast, VisibilityMode::Detailed] {
            let options = Options {
                visibility,
                lighting: LightingMode::DirectLdrHdr,
                ..Default::default()
            };
            let baseline = compile(&fixture(), &game, options);
            let expected = compile(flat.vmf(), &game, options);
            let output = compile_saved(&parent, &game, options, &context);
            assert!(!output.report.has_errors(), "{:?}", output.report);
            assert_eq!(output.stats.brushes, baseline.stats.brushes);
            assert_eq!(output.stats.models, baseline.stats.models);
            assert_eq!(
                output.stats.visibility_portals,
                baseline.stats.visibility_portals
            );
            assert_eq!(
                output.stats.visibility_solved_clusters,
                baseline.stats.clusters
            );
            assert!(output.stats.clusters < baseline.stats.clusters);
            assert_eq!(
                (
                    output.stats.viscluster_helpers,
                    output.stats.viscluster_groups
                ),
                (1, 1)
            );
            assert!(output.stats.viscluster_grouped_leaves >= 2);
            let result = output.bsp.unwrap();
            assert_eq!(result.to_bytes(), expected.bsp.unwrap().to_bytes());
            assert_physical_leaves_visibility_and_probes(&baseline.bsp.unwrap(), &result);
            let entities = String::from_utf8_lossy(&result.lumps[bsp::lump::ENTITIES].data);
            assert!(
                !entities.contains("func_viscluster")
                    && !entities.contains("func_instance")
                    && !entities.contains("func_detail")
            );
        }
        assert_eq!(parent, before);
        assert_eq!(
            std::fs::read(directory.path().join("leaf.vmf")).unwrap(),
            child_bytes
        );
        assert_eq!(
            std::fs::read(directory.path().join("parent.vmf")).unwrap(),
            parent.write(&Default::default()).into_bytes()
        );
    }
}

#[test]
fn root_repeated_and_nested_occurrences_share_the_post_region_sixteen_helper_limit() {
    let directory = tempfile::tempdir().unwrap();
    let game = region_game(directory.path());
    let mut leaf = helper_file([600.0, 600.0, 64.0], [608.0, 608.0, 72.0]);
    let mut hidden = Entity::new(100, "func_viscluster");
    hidden.hidden = true;
    hidden.properties.set("origin", "NaN 0 0");
    leaf.entities.push(hidden);
    let mut hidden_solid = leaf.entities[0].clone();
    hidden_solid.id = 101;
    hidden_solid.solids[0].id = 102;
    hidden_solid.solids[0].hidden = true;
    for (index, side) in hidden_solid.solids[0].sides.iter_mut().enumerate() {
        side.id = 103 + index as u32;
    }
    leaf.entities.push(hidden_solid);
    write_source(directory.path(), "leaf.vmf", &leaf);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "0 0 0", "0 0 0");
    instance(&mut middle, "leaf.vmf", "0 0 0", "0 0 0");
    write_source(directory.path(), "middle.vmf", &middle);
    let mut exterior = Document::empty();
    for _ in 0..17 {
        helper(&mut exterior, [1200.0, 600.0, 64.0], [1208.0, 608.0, 72.0]);
    }
    write_source(directory.path(), "exterior.vmf", exterior.vmf());
    let mut source = common::room();
    region(&mut source, "-1024 -1024 -32", "1024 1024 288");
    let baseline = compile(&source, &game, Options::default())
        .bsp
        .unwrap()
        .to_bytes();
    let mut document = Document::new(source);
    helper(&mut document, [600.0, 600.0, 64.0], [608.0, 608.0, 72.0]);
    let mut source = document.into_vmf();
    for _ in 0..7 {
        instance(&mut source, "middle.vmf", "0 0 0", "0 0 0");
    }
    instance(&mut source, "leaf.vmf", "0 0 0", "0 0 0");
    instance(&mut source, "exterior.vmf", "0 0 0", "0 0 0");
    let context = saved_parent(directory.path(), &source);
    let output = compile_saved(&source, &game, Options::default(), &context);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.instances.as_ref().unwrap().visclusters, 32);
    assert_eq!(output.stats.viscluster_helpers, 16);
    assert_eq!(output.stats.viscluster_groups, 0);
    assert_eq!(output.bsp.unwrap().to_bytes(), baseline);
    let seventeenth = instance(&mut source, "leaf.vmf", "0 0 0", "0 0 0");
    let before = source.clone();
    let context = saved_parent(directory.path(), &source);
    let output = compile_saved(&source, &game, Options::default(), &context);
    assert_child_error(
        &output,
        seventeenth,
        "leaf.vmf",
        leaf.entities[0].id,
        "16-helper",
    );
    assert_eq!(output.stats.instances.as_ref().unwrap().visclusters, 33);
    assert_eq!(source, before);
}

#[test]
fn regional_helper_selection_uses_transformed_hulls_and_preserves_child_identity() {
    let directory = tempfile::tempdir().unwrap();
    let game = region_game(directory.path());
    let mut base = fixture();
    region(&mut base, "-254 -254 4", "254 254 252");
    let baseline = compile(&base, &game, Options::default())
        .bsp
        .unwrap()
        .to_bytes();
    for state in ["contained", "crossing", "hidden", "all hidden", "exterior"] {
        let mut child = helper_file([-330.0, -266.0, -8.0], [30.0, 234.0, 232.0]);
        let original = child.entities[0].id;
        child.entities[0].properties.set("origin", "4096 0 64");
        if state == "hidden" {
            child.entities[0].hidden = true;
        } else if state == "all hidden" {
            child.entities[0].solids[0].hidden = true;
        }
        write_source(directory.path(), "leaf.vmf", &child);
        let mut parent = base.clone();
        if state == "crossing" {
            region(&mut parent, "-128 -128 4", "128 128 252");
        }
        let origin = if state == "exterior" {
            "1200 1200 0"
        } else {
            "80 16 16"
        };
        let owner = instance(&mut parent, "leaf.vmf", origin, "0 0 0");
        let before = parent.clone();
        let context = saved_parent(directory.path(), &parent);
        let output = compile_saved(&parent, &game, Options::default(), &context);
        if state == "crossing" {
            assert_child_error(
                &output,
                owner,
                "leaf.vmf",
                original,
                "compile-region boundary",
            );
        } else {
            assert!(!output.report.has_errors(), "{state}: {:?}", output.report);
            let expanded = usize::from(!["hidden", "all hidden"].contains(&state));
            assert_eq!(
                output.stats.instances.as_ref().unwrap().visclusters,
                expanded
            );
            if expanded == 0 {
                let stats = serde_json::to_value(output.stats.instances.as_ref().unwrap()).unwrap();
                assert!(stats.get("visclusters").is_none());
            }
            assert_eq!(
                output.stats.viscluster_helpers,
                usize::from(state == "contained")
            );
            if state == "contained" {
                let mut flat = Document::new(base.clone());
                helper(&mut flat, [-250.0, -250.0, 8.0], [110.0, 250.0, 248.0]);
                assert_eq!(
                    output.bsp.unwrap().to_bytes(),
                    compile(flat.vmf(), &game, Options::default())
                        .bsp
                        .unwrap()
                        .to_bytes()
                );
            } else {
                assert_eq!(output.bsp.unwrap().to_bytes(), baseline);
            }
        }
        assert_eq!(parent, before);
    }
}

#[test]
fn malformed_exterior_sources_and_intermediate_poses_fail_before_regional_omission() {
    let directory = tempfile::tempdir().unwrap();
    let game = region_game(directory.path());
    for (key, value, reason) in [
        ("origin", "NaN 0 0", "origin"),
        ("angles", "bad", "angles"),
        ("parentname", "moving_parent", "parent"),
    ] {
        let mut child = helper_file([600.0, 600.0, 64.0], [608.0, 608.0, 72.0]);
        child.entities[0].properties.set(key, value);
        let original = child.entities[0].id;
        write_source(directory.path(), "leaf.vmf", &child);
        let mut middle = Document::empty().into_vmf();
        instance(&mut middle, "leaf.vmf", "0 0 0", "0 0 0");
        write_source(directory.path(), "middle.vmf", &middle);
        let mut parent = common::room();
        region(&mut parent, "-254 -254 4", "254 254 252");
        let owner = instance(&mut parent, "middle.vmf", "0 0 0", "0 0 0");
        let before = parent.clone();
        let context = saved_parent(directory.path(), &parent);
        let output = compile_saved(&parent, &game, Options::default(), &context);
        assert_child_error(&output, owner, "leaf.vmf", original, reason);
        assert!(output.stats.compile_region.is_none());
        assert_eq!(parent, before);
    }
    let child = helper_file([600.0, 600.0, 64.0], [608.0, 608.0, 72.0]);
    let original = child.entities[0].id;
    write_source(directory.path(), "leaf.vmf", &child);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "16360 0 0", "0 0 0");
    write_source(directory.path(), "middle.vmf", &middle);
    let mut parent = common::room();
    region(&mut parent, "-254 -254 4", "254 254 252");
    let owner = instance(&mut parent, "middle.vmf", "-16360 0 0", "0 0 0");
    let context = saved_parent(directory.path(), &parent);
    let output = compile_saved(&parent, &game, Options::default(), &context);
    assert_child_error(&output, owner, "leaf.vmf", original, "compile bounds");
    assert!(output.stats.compile_region.is_none());
}

#[test]
fn ambiguous_child_claims_report_both_final_source_owners_and_preserve_publication() {
    let directory = tempfile::tempdir().unwrap();
    let mut parent = common::room();
    let mut expected = Vec::new();
    for (file, middle_file, mins, maxs) in [
        (
            "left.vmf",
            "middle-left.vmf",
            [-256.0, -256.0, 0.0],
            [-128.0, 256.0, 256.0],
        ),
        (
            "right.vmf",
            "middle-right.vmf",
            [128.0, -256.0, 0.0],
            [256.0, 256.0, 256.0],
        ),
    ] {
        let child = helper_file(mins, maxs);
        let original = child.entities[0].id;
        write_source(directory.path(), file, &child);
        let mut middle = Document::empty().into_vmf();
        instance(&mut middle, file, "0 0 0", "0 0 0");
        write_source(directory.path(), middle_file, &middle);
        let owner = instance(&mut parent, middle_file, "0 0 0", "0 0 0");
        expected.push((owner, file, original));
    }
    let before = parent.clone();
    let context = saved_parent(directory.path(), &parent);
    let output = compile_saved(&parent, &common::empty_game(), Options::default(), &context);
    for (owner, file, original) in expected {
        assert_child_error(&output, owner, file, original, "both qualify");
    }
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(path.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
    assert_eq!(parent, before);
}

#[test]
fn child_threshold_uncertainty_retains_final_leaf_source_identity() {
    let directory = tempfile::tempdir().unwrap();
    let child = helper_file([-256.0, -256.0, 0.0], [-204.8, 256.0, 256.0]);
    let original = child.entities[0].id;
    write_source(directory.path(), "leaf.vmf", &child);
    let mut parent = common::room();
    let owner = instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0");
    let context = saved_parent(directory.path(), &parent);
    let output = compile_saved(&parent, &common::empty_game(), Options::default(), &context);
    assert_child_error(&output, owner, "leaf.vmf", original, "strict 10%");
}

#[test]
fn helper_only_transitive_dependencies_and_cancellation_guard_publication_and_history() {
    let directory = tempfile::tempdir().unwrap();
    let child = helper_file([-250.0, -250.0, 8.0], [110.0, 250.0, 248.0]);
    let child_bytes = write_source(directory.path(), "leaf.vmf", &child);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "0 0 0", "0 0 0");
    let middle_bytes = write_source(directory.path(), "middle.vmf", &middle);
    let mut parent = fixture();
    let owner = instance(&mut parent, "middle.vmf", "0 0 0", "0 0 0");
    let mut document = Document::new(parent);
    document
        .edit("authored instance name", |tx| {
            tx.entity_mut(owner)?
                .properties
                .set("targetname", "assembly");
            Ok(())
        })
        .unwrap();
    let before = document.vmf().clone();
    let history = document
        .undo_labels()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let context = saved_parent(directory.path(), document.vmf());
    let parent_bytes = std::fs::read(directory.path().join("parent.vmf")).unwrap();
    let output = compile_saved(
        document.vmf(),
        &common::empty_game(),
        Options::default(),
        &context,
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let stats = output.stats.instances.as_ref().unwrap();
    assert_eq!(
        (
            stats.files,
            stats.visclusters,
            stats.world_brushes,
            stats.detail_brushes
        ),
        (2, 1, 0, 0)
    );
    assert_eq!(
        stats
            .dependencies
            .iter()
            .map(|dependency| dependency.path.as_str())
            .collect::<Vec<_>>(),
        ["leaf.vmf", "middle.vmf"]
    );
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
    let cancelled = Control::default();
    cancelled.cancel().unwrap();
    assert!(output.write_controlled(&path, &cancelled).is_err());
    let staged = output
        .prepare_write_controlled(&path, &Control::default())
        .unwrap()
        .unwrap();
    let mut changed = child_bytes.clone();
    changed.extend_from_slice(b"\n");
    std::fs::write(directory.path().join("leaf.vmf"), &changed).unwrap();
    assert!(staged.commit(&Control::default()).is_err());
    assert!(output.write(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(path.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
    std::fs::write(directory.path().join("leaf.vmf"), &child_bytes).unwrap();
    let cancelled_output = compiler::compile_with_source_controlled(
        document.vmf(),
        &common::empty_game(),
        Options::default(),
        None,
        Some(&context),
        &cancelled,
    );
    assert!(cancelled_output.bsp.is_none() && cancelled_output.leak.is_none());
    assert!(
        cancelled_output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("cancelled"))
    );
    assert_eq!(document.vmf(), &before);
    assert_eq!(
        document.undo_labels().collect::<Vec<_>>(),
        history.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(document.redo_labels().count(), 0);
    assert_eq!(
        std::fs::read(directory.path().join("parent.vmf")).unwrap(),
        parent_bytes
    );
    assert_eq!(
        std::fs::read(directory.path().join("middle.vmf")).unwrap(),
        middle_bytes
    );
    assert_eq!(
        std::fs::read(directory.path().join("leaf.vmf")).unwrap(),
        child_bytes
    );
}
