use glam::DMat3;

use super::*;
use crate::map::Map;

const PIVOT: DVec3 = DVec3::new(16.125, -32.25, 24.375);
const YAW: DMat3 = DMat3::from_cols_array(&[0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
const MIXED: DMat3 = DMat3::from_cols_array(&[
    0.7697511313200572,
    0.5389855446957562,
    -0.3420201433256687,
    -0.5162121193659353,
    0.8407726623973335,
    0.16317591116653482,
    0.37551064385876115,
    0.050950100827363065,
    0.9254165783983234,
]);

fn fixture(pivot: DVec3, angles: Option<&str>, samples: [DVec3; 2]) -> (Vmf, [u32; 2], u32) {
    let (mut vmf, helpers) = source();
    for (helper, position) in helpers.into_iter().zip(samples) {
        vmf.entities
            .iter_mut()
            .find(|entity| entity.id == helper)
            .unwrap()
            .properties
            .set("origin", source_formats::vmf::format_vec3(position));
    }
    let mut document = Document::new(vmf);
    let model = document
        .edit("posed reflection model", |tx| {
            let ObjectId::Solid(solid) = edit::create_block(
                tx,
                pivot + DVec3::new(48.0, -12.0, -10.0),
                pivot + DVec3::new(80.0, 12.0, 10.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            let ObjectId::Entity(model) = edit::tie_to_entity(
                tx,
                &[solid],
                "func_brush",
                &[("origin".into(), source_formats::vmf::format_vec3(pivot))],
            )?
            else {
                unreachable!()
            };
            let ObjectId::Solid(terrain) = edit::create_block(
                tx,
                DVec3::new(-160.0, -160.0, 4.0),
                DVec3::new(-128.0, -128.0, 8.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            let solid = tx.solid_mut(terrain)?;
            let geometry = map_core::BrushGeometry::from_solid(solid).unwrap();
            let face = geometry
                .faces
                .iter()
                .find(|face| face.plane.normal.z > 0.99)
                .unwrap();
            solid.sides[face.side].displacement = Some(source_formats::vmf::DispInfo {
                power: 2,
                start_position: face.vertices[0],
                flags: Some(0),
                elevation: 0.0,
                subdivided: false,
                normals: None,
                distances: None,
                offsets: None,
                offset_normals: None,
                alphas: None,
                triangle_tags: None,
                allowed_verts: None,
                extra_properties: Default::default(),
                extra: Vec::new(),
            });
            Ok(model)
        })
        .unwrap();
    let mut vmf = document.vmf().clone();
    if let Some(angles) = angles {
        vmf.entities
            .iter_mut()
            .find(|entity| entity.id == model)
            .unwrap()
            .properties
            .set("angles", angles);
    }
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
        side.lightmap_scale = 64;
    }
    (vmf, helpers, model)
}

fn local_sides(vmf: &Vmf, game: &GameFs, model: u32) -> Vec<(u32, DVec3, DVec3)> {
    let mut report = crate::Report::default();
    let map = Map::load(vmf, game, &mut report).unwrap();
    assert!(!report.has_errors(), "{report:?}");
    assert_eq!(map.terrains.len(), 1);
    let mut sides = Vec::new();
    for brush in &map.brushes {
        if map.entities[brush.entity].id != model {
            continue;
        }
        for side in &brush.sides {
            if !side.visible || side.bevel {
                continue;
            }
            let (Some(id), Some(winding)) = (side.source_id, &side.winding) else {
                continue;
            };
            let center = winding.0.iter().copied().sum::<DVec3>() / winding.0.len() as f64;
            sides.push((id, center, map.planes.get(side.plane).normal));
        }
    }
    assert_eq!(sides.len(), 6);
    sides
}

fn nearest_reference(center: DVec3, normal: DVec3, samples: [DVec3; 2]) -> usize {
    let front = samples
        .iter()
        .any(|sample| (*sample - center).dot(normal) >= 0.0);
    samples
        .into_iter()
        .enumerate()
        .filter(|(_, sample)| !front || (*sample - center).dot(normal) >= 0.0)
        .min_by(|(a, left), (b, right)| {
            left.distance_squared(center)
                .total_cmp(&right.distance_squared(center))
                .then(a.cmp(b))
        })
        .unwrap()
        .0
}

fn inspect(vmf: &Vmf, game: &GameFs, namespace: &str) -> CubemapBindingReport {
    inspect_cubemap_bindings(
        vmf,
        game,
        &CubemapBindingQuery {
            namespace: Some(namespace.into()),
            limit: 1024,
            ..Default::default()
        },
        &Control::default(),
    )
    .unwrap()
}

fn expected_manual(
    vmf: &Vmf,
    game: &GameFs,
    model: u32,
    helpers: [u32; 2],
    samples: [DVec3; 2],
    matrix: DMat3,
    offset: DVec3,
) -> Vmf {
    let mut source = vmf.clone();
    let mut selected = [Vec::new(), Vec::new()];
    for (id, center, normal) in local_sides(vmf, game, model) {
        selected[nearest_reference(matrix * center + offset, matrix * normal, samples)]
            .push(id.to_string());
    }
    for (helper, sides) in helpers.into_iter().zip(selected) {
        source
            .entities
            .iter_mut()
            .find(|entity| entity.id == helper)
            .unwrap()
            .properties
            .set("sides", sides.join(" "));
    }
    source
}

#[test]
fn literal_world_centers_and_normals_choose_samples_and_reject_partial_transform_controls() {
    let (_directory, game) = game();
    let cases = [
        (
            "0 90 0",
            YAW,
            [
                DVec3::new(128.0, -16.0, 24.0),
                DVec3::new(16.0, 128.0, 24.0),
            ],
        ),
        (
            "-90 0 0",
            DMat3::from_cols_array(&[0.0, 0.0, 1.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0]),
            [
                DVec3::new(128.0, -16.0, 25.0),
                DVec3::new(16.0, -32.0, 192.0),
            ],
        ),
        (
            "20 35 10",
            MIXED,
            [
                DVec3::new(112.0, -40.0, 24.0),
                DVec3::new(155.0, 65.0, 80.0),
            ],
        ),
    ];
    for (angles, matrix, samples) in cases {
        let (vmf, helpers, model) = fixture(PIVOT, Some(angles), samples);
        let before = vmf.clone();
        let local = local_sides(&vmf, &game, model);
        let (_, center, normal) = local
            .iter()
            .find(|(_, _, normal)| normal.x > 0.99)
            .copied()
            .unwrap();
        assert!(
            center.distance(DVec3::new(80.0, 0.0, 0.0)) < 1e-8,
            "{center}"
        );
        assert_eq!(
            nearest_reference(matrix * center + PIVOT, matrix * normal, samples),
            1
        );
        for (point, direction) in [
            (center + PIVOT, normal),
            (matrix * center + PIVOT, normal),
            (center + PIVOT, matrix * normal),
        ] {
            assert_eq!(nearest_reference(point, direction, samples), 0, "{angles}");
        }
        if angles == "0 90 0" {
            assert_eq!(matrix * center + PIVOT, DVec3::new(16.125, 47.75, 24.375));
            assert_eq!(matrix * normal, DVec3::Y);
        }
        if angles == "20 35 10" {
            let no_roll = DMat3::from_cols_array(&[
                0.7697511313200572,
                0.5389855446957562,
                -0.3420201433256687,
                -0.573576436351046,
                0.8191520442889918,
                0.0,
                0.2801664995932355,
                0.1961746949690111,
                0.9396926207859084,
            ]);
            let (_, center, normal) = local
                .iter()
                .find(|(_, _, normal)| normal.y > 0.99)
                .copied()
                .unwrap();
            assert_eq!(
                nearest_reference(matrix * center + PIVOT, matrix * normal, samples),
                1
            );
            assert_eq!(
                nearest_reference(no_roll * center + PIVOT, no_roll * normal, samples),
                0
            );
        }
        let report = inspect(&vmf, &game, "rotated_reference");
        assert_eq!(
            report
                .samples
                .iter()
                .map(|sample| sample.origin)
                .collect::<Vec<_>>(),
            samples.map(|point| point.as_ivec3().to_array())
        );
        for &(id, center, normal) in &local {
            let assignment = report
                .assignments
                .iter()
                .find(|assignment| assignment.face_id == id)
                .unwrap();
            assert_eq!(
                assignment.helper_id,
                helpers[nearest_reference(matrix * center + PIVOT, matrix * normal, samples)],
                "{angles} side:{id}"
            );
            assert!(!assignment.manual);
        }
        let mut zero = vmf.clone();
        zero.entities
            .iter_mut()
            .find(|entity| entity.id == model)
            .unwrap()
            .properties
            .set("angles", "0 0 0");
        let baseline = inspect(&zero, &game, "rotated_reference");
        let terrain = vmf
            .world
            .solids
            .iter()
            .flat_map(|solid| &solid.sides)
            .find(|side| side.displacement.is_some())
            .unwrap()
            .id;
        assert!(
            report
                .assignments
                .iter()
                .any(|assignment| assignment.face_id == terrain)
        );
        for assignment in &baseline.assignments {
            if !local.iter().any(|(id, _, _)| *id == assignment.face_id) {
                assert_eq!(
                    report
                        .assignments
                        .iter()
                        .find(|other| other.face_id == assignment.face_id),
                    Some(assignment)
                );
            }
        }
        let namespaced = inspect(&vmf, &game, "other_namespace");
        assert_eq!(report.assignments.len(), namespaced.assignments.len());
        for (left, right) in report.assignments.iter().zip(&namespaced.assignments) {
            assert_eq!(
                (left.face_id, left.helper_id, left.manual),
                (right.face_id, right.helper_id, right.manual)
            );
            assert_ne!(left.alias, right.alias);
        }
        assert_eq!(vmf, before);
    }
}

#[test]
fn pending_compiler_aliases_match_independently_chosen_manual_samples_in_both_channels() {
    let (_directory, game) = game();
    let samples = [
        DVec3::new(128.0, -16.0, 24.0),
        DVec3::new(16.0, 128.0, 24.0),
    ];
    let (vmf, helpers, model) = fixture(PIVOT, Some("0 90 0"), samples);
    let manual = expected_manual(&vmf, &game, model, helpers, samples, YAW, PIVOT);
    for lighting in [LightingMode::DirectLdr, LightingMode::DirectLdrHdr] {
        let outputs = [&vmf, &manual].map(|source| {
            let output = compile_with_context_controlled(
                source,
                &game,
                Options {
                    lighting,
                    generate_reflections: true,
                    ..Default::default()
                },
                Some(&CompileContext::new("rotated_pending").unwrap()),
                &Control::default(),
            );
            assert!(!output.report.has_errors(), "{:?}", output.report);
            assert!(output.bsp.is_some());
            output
        });
        let [actual, reference] = outputs;
        let actual_manifest = actual.cubemap_assets.as_ref().unwrap();
        let reference_manifest = reference.cubemap_assets.as_ref().unwrap();
        assert_eq!(actual_manifest.phase, CubemapAssetPhase::Pending);
        assert_eq!(actual_manifest.aliases, reference_manifest.aliases);
        assert_eq!(actual_manifest.samples, reference_manifest.samples);
        assert_eq!(actual_manifest.hdr_samples, reference_manifest.hdr_samples);
        assert_eq!(
            actual_manifest.texinfo_bindings,
            reference_manifest.texinfo_bindings
        );
        assert_eq!(
            actual_manifest.face_bindings,
            reference_manifest.face_bindings
        );
        assert_eq!(
            actual_manifest.assignments.len(),
            reference_manifest.assignments.len()
        );
        assert_eq!(
            actual_manifest.hdr_samples.len(),
            if lighting == LightingMode::DirectLdrHdr {
                2
            } else {
                0
            }
        );
        for (left, right) in actual_manifest
            .assignments
            .iter()
            .zip(&reference_manifest.assignments)
        {
            assert_eq!(
                (left.face_id, left.helper_id, &left.alias),
                (right.face_id, right.helper_id, &right.alias)
            );
        }
        let actual_bsp = actual.bsp.as_ref().unwrap();
        let reference_bsp = reference.bsp.as_ref().unwrap();
        assert_eq!(
            actual_bsp.records::<bsp::TexInfo>().unwrap(),
            reference_bsp.records::<bsp::TexInfo>().unwrap()
        );
        assert_eq!(
            actual_bsp.texdata_strings().unwrap(),
            reference_bsp.texdata_strings().unwrap()
        );
        verify_generated_cubemaps(actual_bsp, actual_manifest, &Control::default()).unwrap();
    }
}

#[test]
fn absent_exact_zero_and_signed_zero_keep_identical_assignment_alias_plans() {
    let (_directory, game) = game();
    let samples = [
        DVec3::new(128.0, -16.0, 24.0),
        DVec3::new(16.0, 128.0, 24.0),
    ];
    let (mut vmf, _, model) = fixture(PIVOT, None, samples);
    let baseline = inspect(&vmf, &game, "identity_plan");
    let context = CompileContext::new("identity_plan").unwrap();
    let mut map = Map::load(&vmf, &game, &mut crate::Report::default()).unwrap();
    let baseline_plan = binding::prepare(
        &vmf,
        &game,
        &mut map,
        &context,
        binding::PrepareOptions::default(),
        &mut crate::Report::default(),
        &Control::default(),
    )
    .unwrap();
    for angles in ["0 0 0", "-0 +0 -0e10"] {
        vmf.entities
            .iter_mut()
            .find(|entity| entity.id == model)
            .unwrap()
            .properties
            .set("angles", angles);
        let actual = inspect(&vmf, &game, "identity_plan");
        assert_eq!(actual.assignments, baseline.assignments);
        assert_eq!(actual.samples, baseline.samples);
        let mut map = Map::load(&vmf, &game, &mut crate::Report::default()).unwrap();
        let actual_plan = binding::prepare(
            &vmf,
            &game,
            &mut map,
            &context,
            binding::PrepareOptions::default(),
            &mut crate::Report::default(),
            &Control::default(),
        )
        .unwrap();
        assert_eq!(actual_plan.manifest, baseline_plan.manifest);
        assert_eq!(actual_plan.entries, baseline_plan.entries);
    }
}

#[test]
fn identity_keeps_front_plane_inclusion_and_first_helper_distance_ties() {
    let (_directory, game) = game();
    for samples in [
        [
            DVec3::new(96.0, -32.0, 24.0),
            DVec3::new(128.0, -32.0, 24.0),
        ],
        [
            DVec3::new(128.0, -16.0, 24.0),
            DVec3::new(128.0, -48.0, 24.0),
        ],
    ] {
        let pivot = DVec3::new(16.0, -32.0, 24.0);
        let (vmf, helpers, model) = fixture(pivot, None, samples);
        let local = local_sides(&vmf, &game, model);
        let (id, center, normal) = local
            .iter()
            .find(|(_, _, normal)| normal.x > 0.99)
            .copied()
            .unwrap();
        assert_eq!(nearest_reference(center + pivot, normal, samples), 0);
        let report = inspect(&vmf, &game, "stable_assignment");
        assert_eq!(
            report
                .assignments
                .iter()
                .find(|assignment| assignment.face_id == id)
                .unwrap()
                .helper_id,
            helpers[0]
        );
    }
}

#[test]
fn reflection_in_a_material_dependency_still_uses_the_rotated_source_side() {
    let (directory, game) = game();
    std::fs::write(
        directory
            .path()
            .join("materials/dev/dev_measuregeneric01.vmt"),
        "LightmappedGeneric { $fallbackmaterial dev/reflection_dependency }",
    )
    .unwrap();
    std::fs::write(
        directory
            .path()
            .join("materials/dev/reflection_dependency.vmt"),
        "LightmappedGeneric { $envmap env_cubemap }",
    )
    .unwrap();
    let samples = [
        DVec3::new(128.0, -16.0, 24.0),
        DVec3::new(16.0, 128.0, 24.0),
    ];
    let (vmf, helpers, model) = fixture(PIVOT, Some("0 90 0"), samples);
    let local = local_sides(&vmf, &game, model);
    let (id, _, _) = local
        .iter()
        .find(|(_, _, normal)| normal.x > 0.99)
        .copied()
        .unwrap();
    let report = inspect(&vmf, &game, "dependent_reflection");
    assert_eq!(
        report
            .assignments
            .iter()
            .find(|assignment| assignment.face_id == id)
            .unwrap()
            .helper_id,
        helpers[1]
    );
}

#[test]
fn origin_volume_keeps_authored_local_subtraction_and_uses_native_float_world_offset() {
    let (directory, game) = game();
    std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsorigin.vmt"),
        "LightmappedGeneric { %compileorigin 1 }",
    )
    .unwrap();
    let pivot = DVec3::new(101.500001, -67.250001, 32.125001);
    let native = DVec3::new(101.5, -67.25, 32.125);
    let samples = [
        DVec3::new(100.0, 128.0, 32.0),
        DVec3::new(103.0, 128.0, 32.0),
    ];
    let (vmf, helpers, model) = fixture(pivot, Some("0 90 0"), samples);
    let mut document = Document::new(vmf);
    let ObjectId::Solid(origin) = document
        .edit("origin volume", |tx| {
            edit::create_block(
                tx,
                pivot - DVec3::splat(2.0),
                pivot + DVec3::splat(2.0),
                "tools/toolsorigin",
            )
        })
        .unwrap()
    else {
        unreachable!()
    };
    let mut vmf = document.vmf().clone();
    let index = vmf
        .world
        .solids
        .iter()
        .position(|solid| solid.id == origin)
        .unwrap();
    let origin = vmf.world.solids.remove(index);
    let owner = vmf
        .entities
        .iter_mut()
        .find(|entity| entity.id == model)
        .unwrap();
    owner.properties.set("origin", "0 0 0");
    for side in &mut owner.solids[0].sides {
        for point in &mut side.plane.0 {
            point.y += 0.00000025;
        }
    }
    owner.solids.push(origin);
    let before = vmf.clone();
    let mut report = crate::Report::default();
    let map = Map::load(&vmf, &game, &mut report).unwrap();
    assert!(!report.has_errors(), "{report:?}");
    let entity = map
        .entities
        .iter()
        .find(|entity| entity.id == model)
        .unwrap();
    let authored = entity.origin.unwrap();
    assert!(authored.distance(pivot) < 1e-8 && authored != native);
    let stored: Vec<f64> = entity
        .properties
        .iter()
        .find(|(key, _)| key == "origin")
        .unwrap()
        .1
        .split_ascii_whitespace()
        .map(|number| number.parse().unwrap())
        .collect();
    assert_eq!(
        DVec3::from_array([
            stored[0] as f32 as f64,
            stored[1] as f32 as f64,
            stored[2] as f32 as f64
        ]),
        native
    );
    let local = local_sides(&vmf, &game, model);
    let (_, center, normal) = local
        .iter()
        .find(|(_, _, normal)| normal.x > 0.99)
        .copied()
        .unwrap();
    assert_eq!(
        nearest_reference(YAW * center + native, YAW * normal, samples),
        0
    );
    assert_eq!(
        nearest_reference(YAW * center + authored, YAW * normal, samples),
        1
    );
    let actual = inspect(&vmf, &game, "volume_origin");
    for (id, center, normal) in local {
        let assignment = actual
            .assignments
            .iter()
            .find(|assignment| assignment.face_id == id)
            .unwrap();
        assert_eq!(
            assignment.helper_id,
            helpers[nearest_reference(YAW * center + native, YAW * normal, samples)]
        );
    }
    assert_eq!(vmf, before);
}

#[test]
fn unsupported_automatic_class_parent_origin_and_bounds_preserve_previous_bsp() {
    let (directory, game) = game();
    let samples = [
        DVec3::new(128.0, -16.0, 24.0),
        DVec3::new(16.0, 128.0, 24.0),
    ];
    let (baseline, _, model) = fixture(PIVOT, Some("0 90 0"), samples);
    let previous = crate::compile_with_options(
        &baseline,
        &game,
        Options {
            lighting: LightingMode::DirectLdr,
            ..Default::default()
        },
    );
    assert!(!previous.report.has_errors(), "{:?}", previous.report);
    let destination = directory.path().join("previous.bsp");
    assert!(previous.write(&destination).unwrap());
    let previous_bytes = std::fs::read(&destination).unwrap();
    for case in ["class", "parent", "duplicate-origin", "bounds"] {
        let mut vmf = baseline.clone();
        let owner = vmf
            .entities
            .iter_mut()
            .find(|entity| entity.id == model)
            .unwrap();
        match case {
            "class" => owner.properties.set("classname", "func_door"),
            "parent" => owner.properties.set("parentname", "moving_parent"),
            "duplicate-origin" => owner
                .properties
                .0
                .push(("OrIgIn".into(), "16.125 -32.25 24.375".into())),
            "bounds" => owner.properties.set("origin", "16380 0 32"),
            _ => unreachable!(),
        }
        let sides: Vec<_> = owner.solids[0].sides.iter().map(|side| side.id).collect();
        let before = vmf.clone();
        let error = inspect_cubemap_bindings(&vmf, &game, &Default::default(), &Control::default())
            .unwrap_err();
        assert!(
            error.contains("automatic cubemap assignment")
                && error.contains(&format!("entity:{model}")),
            "{case}: {error}"
        );
        assert!(
            sides
                .iter()
                .any(|side| error.contains(&format!("side:{side}"))),
            "{error}"
        );
        if case == "bounds" {
            assert!(error.contains("bounds"), "{error}");
        }
        let output = compile_with_context_controlled(
            &vmf,
            &game,
            Options {
                lighting: LightingMode::DirectLdr,
                generate_reflections: true,
                ..Default::default()
            },
            Some(&CompileContext::new("refusal").unwrap()),
            &Control::default(),
        );
        assert!(
            output.bsp.is_none() && output.report.has_errors(),
            "{case}: {:?}",
            output.report
        );
        assert!(
            output.report.diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains("automatic cubemap assignment")
                    && diagnostic.message.contains(&format!("entity:{model}"))
            }),
            "{case}: {:?}",
            output.report
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), previous_bytes);
        assert_eq!(vmf, before);
    }
}

#[test]
fn manual_and_nonreflective_inspection_do_not_require_an_automatic_pose() {
    for manual in [false, true] {
        let (directory, game) = game();
        std::fs::write(
            directory.path().join("materials/dev/nonreflective.vmt"),
            "LightmappedGeneric { $surfaceprop concrete }",
        )
        .unwrap();
        let (mut vmf, helpers, model) = model_source();
        let owner = vmf
            .entities
            .iter_mut()
            .find(|entity| entity.id == model)
            .unwrap();
        owner.properties.set("angles", "bad");
        owner.properties.set("parentname", "moving_parent");
        let sides: Vec<_> = owner.solids[0].sides.iter().map(|side| side.id).collect();
        if !manual {
            for side in &mut owner.solids[0].sides {
                side.material = "dev/nonreflective".into();
            }
        } else {
            vmf.entities
                .iter_mut()
                .find(|entity| entity.id == helpers[0])
                .unwrap()
                .properties
                .set(
                    "sides",
                    sides
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(" "),
                );
        }
        let before = vmf.clone();
        let report = inspect(&vmf, &game, "explicit_only");
        for id in sides {
            let assignment = report
                .assignments
                .iter()
                .find(|assignment| assignment.face_id == id);
            if manual {
                let assignment = assignment.unwrap();
                assert!(assignment.manual);
                assert_eq!(assignment.helper_id, helpers[0]);
            } else {
                assert!(assignment.is_none());
            }
        }
        assert_eq!(vmf, before);
    }
}
