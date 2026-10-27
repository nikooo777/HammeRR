mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use compiler::cubemap_assets::{
    CubemapBindingQuery, CubemapBindingReport, CubemapHelperSource, MANIFEST_PATH,
    finalize_reflections_with_capture, inspect_cubemap_bindings, inspect_root_cubemap_bindings,
};
use compiler::instances::{self, PreparedInstances, SourceContext};
use compiler::{
    CompileContext, Control, CubemapAssetManifest, CubemapAssetPhase, LightingMode, Options,
};
use glam::DVec3;
use map_core::{BrushGeometry, Document, ObjectId, edit};
use native_runtime::CaptureEnvironment;
use source_formats::bsp::{self, Cubemap, PakLimits};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::keyvalues::Node;
use source_formats::vmf::{Entity, UvAxis, Vmf};

fn next_id(source: &Vmf) -> u32 {
    source.object_ids().chain(source.side_ids()).max().unwrap() + 1
}

fn sample(source: &mut Vmf, origin: &str, sides: Option<&str>) -> u32 {
    let id = next_id(source);
    let mut entity = Entity::new(id, "env_cubemap");
    entity.properties.set("origin", origin);
    entity.properties.set("cubemapsize", "1");
    if let Some(sides) = sides {
        entity.properties.set("sides", sides);
    }
    source.entities.push(entity);
    id
}

fn instance(source: &mut Vmf, file: &str, origin: &str, angles: &str) -> u32 {
    let id = next_id(source);
    let mut entity = Entity::new(id, "func_instance");
    entity.properties.set("file", file);
    entity.properties.set("origin", origin);
    entity.properties.set("angles", angles);
    source.entities.push(entity);
    id
}

fn write_source(directory: &Path, name: &str, source: &Vmf) -> Vec<u8> {
    let bytes = source.write(&Default::default()).into_bytes();
    std::fs::write(directory.join(name), &bytes).unwrap();
    bytes
}

fn saved_parent(directory: &Path, parent: &Vmf) -> SourceContext {
    write_source(directory, "parent.vmf", parent);
    SourceContext::capture(&directory.join("parent.vmf"), &Control::default()).unwrap()
}

fn game(directory: &Path) -> GameFs {
    for folder in ["dev", "tools", "skybox"] {
        std::fs::create_dir_all(directory.join("materials").join(folder)).unwrap();
    }
    for (name, text) in [
        (
            "dev/dev_measuregeneric01",
            "LightmappedGeneric { $envmap env_cubemap }",
        ),
        ("tools/toolsblack", "UnlitGeneric { }"),
        (
            "tools/toolsnodraw",
            "LightmappedGeneric { %compileNodraw 1 }",
        ),
    ] {
        std::fs::write(directory.join(format!("materials/{name}.vmt")), text).unwrap();
    }
    for face in ["rt", "lf", "bk", "ft", "up", "dn"] {
        std::fs::write(
            directory.join(format!("materials/skybox/sky_day01_01{face}.vmt")),
            "UnlitGeneric { }",
        )
        .unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.to_owned()),
    });
    game
}

fn room() -> Vmf {
    let mut source = common::room();
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 128;
    }
    source
}

fn nested_room() -> Vmf {
    let mut document = Document::empty();
    for (mins, maxs) in [
        ([752.0, 752.0, 1008.0], [1296.0, 1296.0, 1024.0]),
        ([752.0, 752.0, 1280.0], [1296.0, 1296.0, 1296.0]),
        ([752.0, 752.0, 1024.0], [768.0, 1296.0, 1280.0]),
        ([1280.0, 752.0, 1024.0], [1296.0, 1296.0, 1280.0]),
        ([768.0, 752.0, 1024.0], [1280.0, 768.0, 1280.0]),
        ([768.0, 1280.0, 1024.0], [1280.0, 1296.0, 1280.0]),
    ] {
        block(&mut document, mins, maxs, false);
    }
    document
        .edit("nested fixture players", |tx| {
            for (classname, origin) in [
                ("info_player_terrorist", [960.0, 1024.0, 1032.0]),
                ("info_player_counterterrorist", [1088.0, 1024.0, 1032.0]),
            ] {
                edit::create_entity(
                    tx,
                    classname,
                    Some(DVec3::from_array(origin)),
                    &[],
                    Vec::new(),
                )?;
            }
            Ok(())
        })
        .unwrap();
    document.into_vmf()
}

fn block(document: &mut Document, mins: [f64; 3], maxs: [f64; 3], detail: bool) -> u32 {
    document
        .edit("reflective block", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::from_array(mins),
                DVec3::from_array(maxs),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            for side in &mut tx.solid_mut(id)?.sides {
                side.lightmap_scale = 128;
            }
            if detail {
                edit::tie_to_entity(tx, &[id], "func_detail", &[])?;
            }
            Ok(id)
        })
        .unwrap()
}

struct Leaf {
    vmf: Vmf,
    world: u32,
    detail: u32,
    targets: [u32; 2],
    near: u32,
    far: u32,
}

fn leaf(helpers_first: bool) -> Leaf {
    let mut document = Document::empty();
    let world = block(
        &mut document,
        [-40.0, -24.0, 16.0],
        [-8.0, 24.0, 48.0],
        false,
    );
    let detail = block(&mut document, [8.0, -16.0, 16.0], [40.0, 16.0, 32.0], true);
    let targets = [
        document.solid(world).unwrap().sides[0].id,
        map_core::surface::solid(&document, detail).unwrap().sides[0].id,
    ];
    let mut vmf = document.into_vmf();
    for side in vmf
        .world
        .solids
        .iter_mut()
        .chain(
            vmf.entities
                .iter_mut()
                .flat_map(|entity| &mut entity.solids),
        )
        .flat_map(|solid| &mut solid.sides)
    {
        side.u_axis = UvAxis {
            axis: DVec3::ONE.normalize(),
            offset: 0.0,
            scale: 1.0,
        };
        side.v_axis = UvAxis {
            axis: DVec3::new(1.0, 2.0, 3.0).normalize(),
            offset: 0.0,
            scale: 1.0,
        };
    }
    let near = sample(&mut vmf, "0 0 64", None);
    let far = sample(
        &mut vmf,
        "0 0 -96",
        Some(&format!("{}\t {}", targets[0], targets[1])),
    );
    if helpers_first {
        let detail = vmf.entities.remove(0);
        vmf.entities.push(detail);
    }
    Leaf {
        vmf,
        world,
        detail,
        targets,
        near,
        far,
    }
}

fn nested_parent(directory: &Path, child: &Vmf) -> (Vmf, u32) {
    write_source(directory, "leaf.vmf", child);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "16 0 0", "90 0 0");
    instance(&mut middle, "leaf.vmf", "-128 128 0", "90 0 0");
    write_source(directory, "middle.vmf", &middle);
    let mut parent = nested_room();
    let owner = instance(&mut parent, "middle.vmf", "1040 1048 1152", "0 90 0");
    (parent, owner)
}

fn context() -> CompileContext {
    CompileContext::new("instanced_cubemap_bindings").unwrap()
}

fn capture_options(lighting: LightingMode) -> Options {
    Options {
        lighting,
        generate_reflections: true,
        ..Default::default()
    }
}

fn compile_saved(
    parent: &Vmf,
    game: &GameFs,
    source: &SourceContext,
    options: Options,
) -> compiler::Output {
    compiler::compile_with_source_controlled(
        parent,
        game,
        options,
        Some(&context()),
        Some(source),
        &Control::default(),
    )
}

fn pending(output: &compiler::Output) {
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some() && output.leak.is_none());
    assert_eq!(
        output.cubemap_assets.as_ref().unwrap().phase,
        CubemapAssetPhase::Pending
    );
}

fn child_error(output: &compiler::Output, owner: u32, original: u32, reason: &str) {
    assert!(output.report.has_errors() && output.bsp.is_none() && output.leak.is_none());
    assert!(output.cubemap_assets.is_none());
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.object == Some(format!("entity:{owner}"))
                && diagnostic.message.contains("leaf.vmf")
                && diagnostic.message.contains(reason)
                && (diagnostic
                    .message
                    .contains(&format!("Child cubemap {original}"))
                    || diagnostic
                        .message
                        .contains(&format!("child object {original}")))
        }),
        "{:?}",
        output.report
    );
}

struct LiteralOccurrence {
    bounds: [([f64; 3], [f64; 3]); 2],
    sides: [[u32; 6]; 2],
    samples: [[i32; 3]; 2],
    helpers: [u32; 2],
}

fn literal_flat() -> (Vmf, Vec<LiteralOccurrence>) {
    let mut document = Document::new(nested_room());
    let mut occurrences = Vec::new();
    for (bounds, samples, offsets) in [
        (
            [
                ([1016.0, 1080.0, 1160.0], [1064.0, 1112.0, 1192.0]),
                ([1024.0, 1080.0, 1112.0], [1056.0, 1096.0, 1144.0]),
            ],
            [[1040, 1128, 1152], [1040, 968, 1152]],
            [1128.0 / 3.0_f64.sqrt(), 40.0 / 14.0_f64.sqrt()],
        ),
        (
            [
                ([888.0, 936.0, 1160.0], [936.0, 968.0, 1192.0]),
                ([896.0, 936.0, 1112.0], [928.0, 952.0, 1144.0]),
            ],
            [[912, 984, 1152], [912, 824, 1152]],
            [1144.0 / 3.0_f64.sqrt(), 216.0 / 14.0_f64.sqrt()],
        ),
    ] {
        let mut sides = [[0; 6]; 2];
        for (index, (mins, maxs)) in bounds.into_iter().enumerate() {
            let id = block(&mut document, mins, maxs, index == 1);
            document
                .edit("literal face order and texture coordinates", |tx| {
                    let owner = tx.document().owner_of_solid(id).unwrap();
                    let solid = match owner {
                        ObjectId::Solid(id) => tx.solid_mut(id)?,
                        ObjectId::Entity(owner) => tx
                            .entity_mut(owner)?
                            .solids
                            .iter_mut()
                            .find(|solid| solid.id == id)
                            .unwrap(),
                    };
                    assert_eq!(solid.id, id);
                    let original = solid.sides.clone();
                    solid.sides = [4, 5, 0, 1, 2, 3].map(|side| original[side].clone()).into();
                    for (side, original) in solid.sides.iter_mut().zip(&original) {
                        side.id = original.id;
                        side.u_axis = UvAxis {
                            axis: DVec3::new(-1.0, 1.0, -1.0).normalize(),
                            offset: offsets[0],
                            scale: 1.0,
                        };
                        side.v_axis = UvAxis {
                            axis: DVec3::new(-2.0, 3.0, -1.0).normalize(),
                            offset: offsets[1],
                            scale: 1.0,
                        };
                    }
                    sides[index] = solid
                        .sides
                        .iter()
                        .map(|side| side.id)
                        .collect::<Vec<_>>()
                        .try_into()
                        .unwrap();
                    Ok(())
                })
                .unwrap();
        }
        occurrences.push(LiteralOccurrence {
            bounds,
            sides,
            samples,
            helpers: [0; 2],
        });
    }
    let mut flat = document.into_vmf();
    for occurrence in &mut occurrences {
        for index in 0..2 {
            let [x, y, z] = occurrence.samples[index];
            let manual = format!("{} {}", occurrence.sides[0][0], occurrence.sides[1][0]);
            occurrence.helpers[index] = sample(
                &mut flat,
                &format!("{x} {y} {z}"),
                (index == 1).then_some(manual.as_str()),
            );
        }
    }
    (flat, occurrences)
}

struct IdMap {
    sides: BTreeMap<u32, u32>,
    helpers: BTreeMap<u32, u32>,
}

fn literal_ids(
    prepared: &PreparedInstances<'_>,
    parent: &Vmf,
    child: &Leaf,
    owner: u32,
    occurrences: &[LiteralOccurrence],
) -> IdMap {
    let mut ids = IdMap {
        sides: parent.side_ids().map(|id| (id, id)).collect(),
        helpers: BTreeMap::new(),
    };
    let mut seen = BTreeSet::new();
    for brush in prepared.generated_brushes() {
        assert_eq!((brush.owner, brush.file), (owner, "leaf.vmf"));
        let index = usize::from(brush.detail);
        assert_eq!(brush.original, [child.world, child.detail][index]);
        let geometry = BrushGeometry::from_solid(brush.solid).unwrap();
        let occurrence = occurrences
            .iter()
            .position(|literal| {
                geometry
                    .mins
                    .abs_diff_eq(DVec3::from_array(literal.bounds[index].0), 1e-8)
                    && geometry
                        .maxs
                        .abs_diff_eq(DVec3::from_array(literal.bounds[index].1), 1e-8)
            })
            .unwrap();
        assert!(seen.insert((occurrence, index)));
        let original = child
            .vmf
            .solids()
            .find(|solid| solid.id == brush.original)
            .unwrap();
        for ((side, original_side), literal) in brush
            .solid
            .sides
            .iter()
            .zip(&original.sides)
            .zip(occurrences[occurrence].sides[index])
        {
            assert_eq!(
                prepared.face_origin(side.id),
                Some((owner, "leaf.vmf", brush.original, original_side.id))
            );
            assert!(ids.sides.insert(side.id, literal).is_none());
        }
    }
    assert_eq!(seen.len(), 4);
    for helper in prepared
        .vmf()
        .entities
        .iter()
        .filter(|entity| entity.classname() == Some("env_cubemap"))
    {
        assert_eq!(prepared.root_entity(helper.id), Some(owner));
        assert_eq!(helper.properties.get("angles"), None);
        let (occurrence, index) = occurrences
            .iter()
            .enumerate()
            .find_map(|(occurrence, literal)| {
                literal
                    .samples
                    .iter()
                    .position(|origin| {
                        helper
                            .origin()
                            .unwrap()
                            .abs_diff_eq(DVec3::from_array(origin.map(f64::from)), 1e-8)
                    })
                    .map(|index| (occurrence, index))
            })
            .unwrap();
        assert!(
            ids.helpers
                .insert(helper.id, occurrences[occurrence].helpers[index])
                .is_none()
        );
        if index == 0 {
            assert_eq!(helper.properties.get("sides"), None);
        } else {
            let bound = helper
                .properties
                .get("sides")
                .unwrap()
                .split_whitespace()
                .map(|value| ids.sides[&value.parse::<u32>().unwrap()])
                .collect::<Vec<_>>();
            assert_eq!(
                bound,
                [
                    occurrences[occurrence].sides[0][0],
                    occurrences[occurrence].sides[1][0]
                ]
            );
        }
    }
    assert_eq!(ids.helpers.len(), 4);
    ids
}

fn normalize_manifest(manifest: &CubemapAssetManifest, ids: &IdMap) -> CubemapAssetManifest {
    let mut normalized = manifest.clone();
    for sample in normalized
        .samples
        .iter_mut()
        .chain(&mut normalized.hdr_samples)
    {
        sample.helper_id = ids.helpers[&sample.helper_id];
    }
    for assignment in &mut normalized.assignments {
        assignment.face_id = ids.sides[&assignment.face_id];
        assignment.helper_id = ids.helpers[&assignment.helper_id];
    }
    normalized
}

fn pending_matches_literal(actual: &compiler::Output, literal: &compiler::Output, ids: &IdMap) {
    pending(actual);
    pending(literal);
    let normalized = normalize_manifest(actual.cubemap_assets.as_ref().unwrap(), ids);
    assert_eq!(&normalized, literal.cubemap_assets.as_ref().unwrap());
    let actual_bsp = actual.bsp.as_ref().unwrap();
    let literal_bsp = literal.bsp.as_ref().unwrap();
    assert_eq!(actual_bsp.version, literal_bsp.version);
    assert_eq!(actual_bsp.map_revision, literal_bsp.map_revision);
    for (index, lump) in literal_bsp.lumps.iter().enumerate() {
        if index != bsp::lump::PAKFILE {
            assert_eq!(&actual_bsp.lumps[index], lump, "pending lump {index}");
        }
    }
    assert_eq!(actual_bsp.game_lumps, literal_bsp.game_lumps);
    let actual_pak = actual_bsp.pak_data(PakLimits::default()).unwrap();
    let expected_pak = literal_bsp.pak_data(PakLimits::default()).unwrap();
    let actual_entries = actual_pak
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.data.as_slice()))
        .collect::<BTreeMap<_, _>>();
    let expected_entries = expected_pak
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.data.as_slice()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(actual_entries.len(), actual_pak.entries.len());
    assert_eq!(expected_entries.len(), expected_pak.entries.len());
    assert_eq!(
        actual_entries.keys().collect::<Vec<_>>(),
        expected_entries.keys().collect::<Vec<_>>()
    );
    for (name, data) in actual_entries {
        if name == MANIFEST_PATH {
            let manifest = serde_json::from_slice(data).unwrap();
            let expected_manifest: CubemapAssetManifest =
                serde_json::from_slice(expected_entries[name]).unwrap();
            assert_eq!(normalize_manifest(&manifest, ids), expected_manifest);
        } else {
            assert_eq!(data, expected_entries[name], "packed asset {name}");
        }
    }
}

fn root_choices(
    report: &CubemapBindingReport,
    root_sides: &BTreeSet<u32>,
) -> Vec<serde_json::Value> {
    report
        .assignments
        .iter()
        .filter(|assignment| root_sides.contains(&assignment.face_id))
        .map(|assignment| {
            let sample = report
                .samples
                .iter()
                .find(|sample| sample.helper_id == assignment.helper_id)
                .unwrap();
            serde_json::json!({
                "side":assignment.face_id,"position":sample.origin,"manual":assignment.manual,
                "material":assignment.material,"alias":assignment.alias,
            })
        })
        .collect()
}

#[test]
fn root_binding_inspection_matches_independent_literal_assembly_and_keeps_original_sources() {
    for helpers_first in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let child = leaf(helpers_first);
        let (parent, owner) = nested_parent(directory.path(), &child.vmf);
        let source = saved_parent(directory.path(), &parent);
        let (flat, _) = literal_flat();
        let root_sides: BTreeSet<_> = parent.side_ids().collect();
        let query = CubemapBindingQuery {
            namespace: Some(context().namespace().into()),
            limit: 1024,
            ..Default::default()
        };
        let inspection = inspect_root_cubemap_bindings(
            &parent,
            &game,
            Some(&source),
            &query,
            &Control::default(),
        )
        .unwrap();
        let literal = inspect_cubemap_bindings(&flat, &game, &query, &Control::default()).unwrap();
        assert!(!inspection.report.assignments.is_empty());
        assert!(
            inspection
                .report
                .assignments
                .iter()
                .all(|assignment| root_sides.contains(&assignment.face_id))
        );
        assert_eq!(inspection.report.total, inspection.report.assignments.len());
        assert!(literal.total > inspection.report.total);
        assert_eq!(
            root_choices(&inspection.report, &root_sides),
            root_choices(&literal, &root_sides)
        );
        assert!(inspection.report.helpers.is_empty());
        let mut occurrences = BTreeSet::new();
        for origin in inspection.helper_sources.values() {
            let CubemapHelperSource::Instance {
                owner: actual_owner,
                file,
                helper,
                occurrence,
            } = origin
            else {
                panic!("The root fixture has no authored cubemap helpers");
            };
            assert_eq!(*actual_owner, owner);
            assert_eq!(file, "leaf.vmf");
            assert!([child.near, child.far].contains(helper));
            assert_eq!(occurrence.steps().count(), 2);
            occurrences.insert(occurrence.label());
        }
        assert_eq!(occurrences.len(), 2);
        inspection
            .sources
            .verify(&game, &Control::default())
            .unwrap();
        let path = directory.path().join("leaf.vmf");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.push(b'\n');
        std::fs::write(path, bytes).unwrap();
        assert!(
            inspection
                .sources
                .verify(&game, &Control::default())
                .is_err()
        );
        let fresh = inspect_root_cubemap_bindings(
            &parent,
            &game,
            Some(&source),
            &query,
            &Control::default(),
        )
        .unwrap();
        assert_eq!(
            root_choices(&fresh.report, &root_sides),
            root_choices(&inspection.report, &root_sides)
        );
        assert!(
            inspection
                .sources
                .verify(&game, &Control::default())
                .is_err()
        );
    }
}

#[test]
fn repeated_nested_occurrences_bind_their_own_world_and_detail_faces_before_and_after_helpers() {
    for helpers_first in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let child = leaf(helpers_first);
        let (parent, owner) = nested_parent(directory.path(), &child.vmf);
        assert!(
            child
                .targets
                .iter()
                .all(|id| parent.side_ids().any(|root| root == *id))
        );
        let source = saved_parent(directory.path(), &parent);
        let originals = ["parent.vmf", "middle.vmf", "leaf.vmf"]
            .map(|name| std::fs::read(directory.path().join(name)).unwrap());
        let before = parent.clone();
        let mut report = compiler::Report::default();
        let prepared =
            instances::resolve(&parent, Some(&source), &Control::default(), &mut report).unwrap();
        assert!(!report.has_errors(), "{report:?}");
        let stats = prepared.stats().unwrap();
        assert_eq!(
            (
                stats.instances,
                stats.files,
                stats.world_brushes,
                stats.detail_brushes,
                stats.cubemaps,
                stats.max_expansion_depth
            ),
            (3, 2, 2, 2, 4, 2)
        );
        prepared.vmf().validate_ids().unwrap();
        let (flat, occurrences) = literal_flat();
        let ids = literal_ids(&prepared, &parent, &child, owner, &occurrences);
        let ordinary = compile_saved(&parent, &game, &source, Options::default());
        let ordinary_flat = compiler::compile(&flat, &game);
        assert!(!ordinary.report.has_errors(), "{:?}", ordinary.report);
        assert!(
            !ordinary_flat.report.has_errors(),
            "{:?}",
            ordinary_flat.report
        );
        assert_eq!(
            ordinary.bsp.as_ref().unwrap().to_bytes(),
            ordinary_flat.bsp.as_ref().unwrap().to_bytes()
        );
        assert_eq!(
            ordinary
                .bsp
                .as_ref()
                .unwrap()
                .records::<Cubemap>()
                .unwrap()
                .iter()
                .map(|sample| sample.origin)
                .collect::<Vec<_>>(),
            [
                [1040, 1128, 1152],
                [1040, 968, 1152],
                [912, 984, 1152],
                [912, 824, 1152]
            ]
        );
        for lighting in [LightingMode::DirectLdr, LightingMode::DirectLdrHdr] {
            let actual = compile_saved(&parent, &game, &source, capture_options(lighting));
            let literal = compiler::compile_with_context_controlled(
                &flat,
                &game,
                capture_options(lighting),
                Some(&context()),
                &Control::default(),
            );
            pending_matches_literal(&actual, &literal, &ids);
            let manifest = normalize_manifest(actual.cubemap_assets.as_ref().unwrap(), &ids);
            assert_eq!(manifest.samples.len(), 4);
            assert_eq!(
                manifest.hdr_samples.len(),
                if lighting == LightingMode::DirectLdrHdr {
                    4
                } else {
                    0
                }
            );
            assert!(
                !manifest.aliases.is_empty()
                    && !manifest.texinfo_bindings.is_empty()
                    && !manifest.face_bindings.is_empty()
            );
            let manual = manifest
                .assignments
                .iter()
                .filter(|assignment| assignment.manual)
                .map(|assignment| (assignment.face_id, assignment.helper_id))
                .collect::<BTreeSet<_>>();
            assert_eq!(
                manual,
                occurrences
                    .iter()
                    .flat_map(|literal| [literal.sides[0][0], literal.sides[1][0]]
                        .map(|side| (side, literal.helpers[1])))
                    .collect()
            );
        }
        let mut automatic = flat.clone();
        for entity in &mut automatic.entities {
            if entity.classname() == Some("env_cubemap") {
                entity.properties.remove("sides");
            }
        }
        let automatic = compiler::compile_with_context_controlled(
            &automatic,
            &game,
            capture_options(LightingMode::DirectLdr),
            Some(&context()),
            &Control::default(),
        );
        pending(&automatic);
        for occurrence in &occurrences {
            for side in [occurrence.sides[0][0], occurrence.sides[1][0]] {
                let assignment = automatic
                    .cubemap_assets
                    .as_ref()
                    .unwrap()
                    .assignments
                    .iter()
                    .find(|assignment| assignment.face_id == side)
                    .unwrap();
                assert_eq!(assignment.helper_id, occurrence.helpers[0]);
                assert!(!assignment.manual);
            }
        }
        assert_eq!(parent, before);
        for (name, bytes) in ["parent.vmf", "middle.vmf", "leaf.vmf"]
            .into_iter()
            .zip(originals)
        {
            assert_eq!(std::fs::read(directory.path().join(name)).unwrap(), bytes);
        }
    }
}

#[test]
fn manual_targets_cannot_escape_direct_visible_ordinary_geometry_in_their_source_occurrence() {
    for case in [
        "missing",
        "root",
        "hidden world",
        "hidden detail entity",
        "hidden detail solid",
        "terrain top",
        "terrain ordinary side",
        "runtime model",
        "descendant",
        "ancestor",
        "sibling",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut child = leaf(false);
        let mut parent = room();
        match case {
            "missing" => child
                .vmf
                .entities
                .iter_mut()
                .find(|entity| entity.id == child.far)
                .unwrap()
                .properties
                .set("sides", u32::MAX.to_string()),
            "root" => child
                .vmf
                .entities
                .iter_mut()
                .find(|entity| entity.id == child.far)
                .unwrap()
                .properties
                .set("sides", parent.side_ids().max().unwrap().to_string()),
            "hidden world" => child.vmf.world.solids[0].hidden = true,
            "hidden detail entity" => child.vmf.entities[0].hidden = true,
            "hidden detail solid" => child.vmf.entities[0].solids[0].hidden = true,
            "terrain top" | "terrain ordinary side" => {
                let mut document = Document::new(child.vmf);
                document
                    .edit("terrain target", |tx| {
                        map_core::displacement::edit::create(
                            tx,
                            &[map_core::surface::FaceId {
                                solid: child.world,
                                side: child.targets[0],
                            }],
                            2,
                            0,
                        )
                    })
                    .unwrap();
                child.vmf = document.into_vmf();
                if case == "terrain ordinary side" {
                    let bottom = child.vmf.world.solids[0].sides[1].id;
                    child
                        .vmf
                        .entities
                        .iter_mut()
                        .find(|entity| entity.id == child.far)
                        .unwrap()
                        .properties
                        .set("sides", bottom.to_string());
                }
            }
            "runtime model" => {
                child.vmf.entities[0]
                    .properties
                    .set("classname", "func_brush");
            }
            "descendant" | "ancestor" | "sibling" => {
                let mut geometry = child.vmf.clone();
                geometry
                    .entities
                    .retain(|entity| entity.classname() == Some("func_detail"));
                write_source(directory.path(), "geometry.vmf", &geometry);
                child.vmf.world.solids.clear();
                child
                    .vmf
                    .entities
                    .retain(|entity| entity.classname() == Some("env_cubemap"));
                if case == "descendant" {
                    instance(&mut child.vmf, "geometry.vmf", "0 0 0", "0 0 0");
                } else {
                    let mut middle = if case == "ancestor" {
                        geometry
                    } else {
                        Document::empty().into_vmf()
                    };
                    if case == "sibling" {
                        instance(&mut middle, "geometry.vmf", "0 0 0", "0 0 0");
                    }
                    instance(&mut middle, "leaf.vmf", "0 0 0", "0 0 0");
                    write_source(directory.path(), "middle.vmf", &middle);
                }
            }
            _ => unreachable!(),
        }
        write_source(directory.path(), "leaf.vmf", &child.vmf);
        let owner = instance(
            &mut parent,
            if matches!(case, "ancestor" | "sibling") {
                "middle.vmf"
            } else {
                "leaf.vmf"
            },
            "0 0 0",
            "0 0 0",
        );
        let source = saved_parent(directory.path(), &parent);
        let before = parent.clone();
        let error = instances::resolve(
            &parent,
            Some(&source),
            &Control::default(),
            &mut compiler::Report::default(),
        )
        .err()
        .unwrap();
        assert_eq!(error.object, Some(format!("entity:{owner}")), "{case}");
        assert!(
            error.message.contains("leaf.vmf") && error.message.contains("same child"),
            "{case}: {error:?}"
        );
        assert!(
            error
                .message
                .contains(&format!("Child cubemap {}", child.far)),
            "{case}: {error:?}"
        );
        assert!(error.message.contains("side:"), "{case}: {error:?}");
        assert_eq!(parent, before);
    }
}

#[test]
fn retained_manual_bindings_use_the_existing_visible_face_and_duplicate_owner_planner() {
    for case in [
        "region target omitted",
        "nodraw target",
        "duplicate owner",
        "hidden helper",
        "helper and target omitted",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let mut child = leaf(false);
        let mut parent = room();
        let translated = matches!(case, "region target omitted" | "helper and target omitted");
        if case == "region target omitted" {
            child
                .vmf
                .entities
                .iter_mut()
                .find(|entity| entity.id == child.far)
                .unwrap()
                .properties
                .set("origin", "-192 0 64");
        } else if case == "nodraw target" {
            child.vmf.world.solids[0].sides[0].material = "tools/toolsnodraw".into();
        } else if case == "duplicate owner" {
            child
                .vmf
                .entities
                .iter_mut()
                .find(|entity| entity.id == child.near)
                .unwrap()
                .properties
                .set("sides", child.targets[0].to_string());
        } else if case == "hidden helper" {
            let helper = child
                .vmf
                .entities
                .iter_mut()
                .find(|entity| entity.id == child.far)
                .unwrap();
            helper.hidden = true;
            helper.properties.set("sides", "not a face ID");
        } else {
            sample(&mut parent, "-128 0 64", None);
        }
        if translated {
            parent.cordon = Some(Node::block(
                "cordon",
                vec![
                    Node::string("active", "1"),
                    Node::string("mins", "-200 -200 4"),
                    Node::string("maxs", "0 200 252"),
                ],
            ));
        }
        write_source(directory.path(), "leaf.vmf", &child.vmf);
        let owner = instance(
            &mut parent,
            "leaf.vmf",
            if translated { "128 0 0" } else { "0 0 128" },
            "0 0 0",
        );
        let source = saved_parent(directory.path(), &parent);
        let output = compile_saved(
            &parent,
            &game,
            &source,
            capture_options(LightingMode::DirectLdr),
        );
        if matches!(case, "hidden helper" | "helper and target omitted") {
            pending(&output);
            let manifest = output.cubemap_assets.as_ref().unwrap();
            assert_eq!(manifest.samples.len(), 1);
            assert!(
                manifest
                    .assignments
                    .iter()
                    .all(|assignment| !assignment.manual)
            );
            assert_eq!(
                output.stats.instances.as_ref().unwrap().cubemaps,
                if case == "hidden helper" { 1 } else { 2 }
            );
        } else {
            child_error(
                &output,
                owner,
                child.far,
                if case == "duplicate owner" {
                    "duplicate cubemap ownership"
                } else {
                    "visible compiled side"
                },
            );
        }
    }
}

#[test]
fn bounded_manual_sources_and_automatic_empty_lists_keep_prior_output_and_root_id_custody() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut child = leaf(false);
    let mut parent = room();
    let root_helper = sample(&mut parent, "-128 0 64", None);
    let owner = instance(&mut parent, "leaf.vmf", "0 0 128", "0 0 0");
    let original_list = child
        .vmf
        .entities
        .iter()
        .find(|entity| entity.id == child.far)
        .unwrap()
        .properties
        .get("sides")
        .unwrap()
        .to_owned();
    for bytes in [instances::MAX_FIELD_BYTES, instances::MAX_FIELD_BYTES + 1] {
        let mut padded = original_list.clone();
        padded.push_str(&" ".repeat(bytes - padded.len()));
        child
            .vmf
            .entities
            .iter_mut()
            .find(|entity| entity.id == child.far)
            .unwrap()
            .properties
            .set("sides", padded);
        write_source(directory.path(), "leaf.vmf", &child.vmf);
        let source = saved_parent(directory.path(), &parent);
        let result = instances::resolve(
            &parent,
            Some(&source),
            &Control::default(),
            &mut compiler::Report::default(),
        );
        if bytes == instances::MAX_FIELD_BYTES {
            assert!(result.is_ok());
        } else {
            let error = result.err().unwrap();
            assert_eq!(error.object, Some(format!("entity:{owner}")));
            assert!(
                error.message.contains("512")
                    && error
                        .message
                        .contains(&format!("Child cubemap {}", child.far))
            );
        }
    }
    child
        .vmf
        .entities
        .iter_mut()
        .find(|entity| entity.id == child.far)
        .unwrap()
        .properties
        .set("sides", original_list);
    write_source(directory.path(), "leaf.vmf", &child.vmf);
    let floor = parent.world.solids[0].sides[0].id;
    assert_eq!(floor, child.targets[0]);
    parent
        .entities
        .iter_mut()
        .find(|entity| entity.id == root_helper)
        .unwrap()
        .properties
        .set("sides", floor.to_string());
    let source = saved_parent(directory.path(), &parent);
    let output = compile_saved(
        &parent,
        &game,
        &source,
        capture_options(LightingMode::DirectLdr),
    );
    pending(&output);
    let assignments = &output.cubemap_assets.as_ref().unwrap().assignments;
    assert!(
        assignments
            .iter()
            .any(|assignment| assignment.face_id == floor
                && assignment.helper_id == root_helper
                && assignment.manual)
    );
    let child_manual = assignments
        .iter()
        .filter(|assignment| assignment.manual && assignment.helper_id != root_helper)
        .collect::<Vec<_>>();
    assert_eq!(child_manual.len(), 2);
    assert!(
        child_manual
            .iter()
            .all(|assignment| !parent.side_ids().any(|id| id == assignment.face_id))
    );
    parent
        .entities
        .iter_mut()
        .find(|entity| entity.id == root_helper)
        .unwrap()
        .properties
        .remove("sides");
    let mut baseline = None;
    for sides in [None, Some(""), Some(" \t ")] {
        for entity in child
            .vmf
            .entities
            .iter_mut()
            .filter(|entity| entity.classname() == Some("env_cubemap"))
        {
            entity.properties.remove("sides");
            if let Some(sides) = sides {
                entity.properties.set("sides", sides);
            }
        }
        write_source(directory.path(), "leaf.vmf", &child.vmf);
        let source = saved_parent(directory.path(), &parent);
        let ordinary = compile_saved(&parent, &game, &source, Options::default());
        assert!(!ordinary.report.has_errors(), "{:?}", ordinary.report);
        let pending = compile_saved(
            &parent,
            &game,
            &source,
            capture_options(LightingMode::DirectLdrHdr),
        );
        pending_matches_automatic_baseline(&mut baseline, &ordinary, &pending);
        if sides.is_none() {
            let generated = pending
                .cubemap_assets
                .as_ref()
                .unwrap()
                .assignments
                .iter()
                .find(|assignment| !parent.side_ids().any(|id| id == assignment.face_id))
                .unwrap()
                .face_id;
            let mut invalid = parent.clone();
            invalid
                .entities
                .iter_mut()
                .find(|entity| entity.id == root_helper)
                .unwrap()
                .properties
                .set("sides", generated.to_string());
            let invalid_source = saved_parent(directory.path(), &invalid);
            let output = compile_saved(
                &invalid,
                &game,
                &invalid_source,
                capture_options(LightingMode::DirectLdr),
            );
            assert!(
                output.bsp.is_none()
                    && output.cubemap_assets.is_none()
                    && output.stats.instances.is_none()
            );
            assert!(
                output.report.diagnostics.iter().any(|diagnostic| diagnostic
                    .message
                    .contains("original root map")
                    && diagnostic.message.contains(&format!("side:{generated}"))),
                "{:?}",
                output.report
            );
        }
    }
}

fn pending_matches_automatic_baseline(
    baseline: &mut Option<(Vec<u8>, Vec<u8>, CubemapAssetManifest)>,
    ordinary: &compiler::Output,
    output: &compiler::Output,
) {
    pending(output);
    assert!(
        output
            .cubemap_assets
            .as_ref()
            .unwrap()
            .assignments
            .iter()
            .all(|assignment| !assignment.manual)
    );
    let current = (
        ordinary.bsp.as_ref().unwrap().to_bytes(),
        output.bsp.as_ref().unwrap().to_bytes(),
        output.cubemap_assets.as_ref().unwrap().clone(),
    );
    if let Some(baseline) = baseline {
        assert_eq!(&current, &*baseline);
    } else {
        *baseline = Some(current);
    }
}

#[test]
fn manual_binding_dependencies_cancellation_and_failed_publication_preserve_authored_state() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let child = leaf(true);
    let (mut parent, owner) = nested_parent(directory.path(), &child.vmf);
    parent.cameras = Some(Node::block(
        "cameras",
        vec![
            Node::string("activecamera", "0"),
            Node::block(
                "camera",
                vec![
                    Node::string("position", "[-128 -192 160]"),
                    Node::string("look", "[0 0 64]"),
                ],
            ),
        ],
    ));
    let mut document = Document::new(parent);
    document
        .edit("authored assembly name", |tx| {
            tx.entity_mut(owner)?
                .properties
                .set("targetname", "reflection_assembly");
            Ok(())
        })
        .unwrap();
    let before = document.vmf().clone();
    let history = document
        .undo_labels()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let source = saved_parent(directory.path(), document.vmf());
    let originals = ["parent.vmf", "middle.vmf", "leaf.vmf"]
        .map(|name| std::fs::read(directory.path().join(name)).unwrap());
    let output = compile_saved(
        document.vmf(),
        &game,
        &source,
        capture_options(LightingMode::DirectLdrHdr),
    );
    pending(&output);
    assert_eq!(
        output
            .stats
            .instances
            .as_ref()
            .unwrap()
            .dependencies
            .iter()
            .map(|dependency| dependency.path.as_str())
            .collect::<Vec<_>>(),
        ["leaf.vmf", "middle.vmf"]
    );
    let path = directory.path().join("instanced_cubemap_bindings.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
    assert!(
        output
            .write(&path)
            .unwrap_err()
            .to_string()
            .contains("pending native capture")
    );
    let environment = CaptureEnvironment {
        engine_root: "/unused/engine".into(),
        steam_root: "/unused/steam".into(),
        mounts: Vec::new(),
    };
    for (index, name) in ["middle.vmf", "leaf.vmf"].into_iter().enumerate() {
        let reservation = compiler::WriteReservation::new(&path, &Control::default()).unwrap();
        let source = SourceContext::capture(source.path(), &Control::default()).unwrap();
        let ordinary = compile_saved(document.vmf(), &game, &source, Options::default());
        assert!(!ordinary.report.has_errors(), "{:?}", ordinary.report);
        let stale_output = compile_saved(
            document.vmf(),
            &game,
            &source,
            capture_options(LightingMode::DirectLdr),
        );
        pending(&stale_output);
        let staged = ordinary
            .prepare_write_reserved(reservation, &Control::default())
            .unwrap()
            .unwrap();
        let mut changed = originals[if index == 0 { 1 } else { 2 }].clone();
        changed.push(b'\n');
        std::fs::write(directory.path().join(name), changed).unwrap();
        assert!(staged.commit(&Control::default()).is_err());
        assert!(ordinary.write(&path).is_err());
        assert!(
            finalize_reflections_with_capture(
                stale_output,
                &context(),
                &game,
                &environment,
                &Control::default(),
                |_request, _observer| panic!("changed instance source reached native capture")
            )
            .is_err()
        );
        std::fs::write(
            directory.path().join(name),
            &originals[if index == 0 { 1 } else { 2 }],
        )
        .unwrap();
    }
    let output = compile_saved(
        document.vmf(),
        &game,
        &source,
        capture_options(LightingMode::DirectLdrHdr),
    );
    pending(&output);
    let cancelled = Control::default();
    cancelled.cancel().unwrap();
    let error = finalize_reflections_with_capture(
        output,
        &context(),
        &game,
        &environment,
        &cancelled,
        |_request, _observer| panic!("cancelled manual bindings reached native capture"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    let cancelled_output = compiler::compile_with_source_controlled(
        document.vmf(),
        &game,
        capture_options(LightingMode::DirectLdr),
        Some(&context()),
        Some(&source),
        &cancelled,
    );
    assert!(cancelled_output.bsp.is_none() && cancelled_output.cubemap_assets.is_none());
    assert!(!cancelled_output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(path.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
    assert_eq!(document.vmf(), &before);
    assert_eq!(
        document.undo_labels().collect::<Vec<_>>(),
        history.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(document.redo_labels().count(), 0);
    for (name, bytes) in ["parent.vmf", "middle.vmf", "leaf.vmf"]
        .into_iter()
        .zip(originals)
    {
        assert_eq!(std::fs::read(directory.path().join(name)).unwrap(), bytes);
    }
}
