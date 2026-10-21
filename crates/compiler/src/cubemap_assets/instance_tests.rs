use std::path::Path;

use native_runtime::{CaptureEnvironment, CaptureRequest};
use source_formats::keyvalues::Node;
use source_formats::vmf::Entity;

use super::*;
use crate::instances::SourceContext;
use crate::{Report, compile_with_source_controlled};

fn write_vmf(directory: &Path, name: &str, vmf: &Vmf) {
    std::fs::write(directory.join(name), vmf.write(&Default::default())).unwrap();
}

fn instance(id: u32, file: &str, origin: &str, angles: &str) -> Entity {
    let mut entity = Entity::new(id, "func_instance");
    entity.properties.set("file", file);
    entity.properties.set("origin", origin);
    entity.properties.set("angles", angles);
    entity
}

fn fixture() -> (tempfile::TempDir, GameFs, Vmf, [u32; 2], u32) {
    let (directory, game) = game();
    std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsblack.vmt"),
        "UnlitGeneric {}",
    )
    .unwrap();
    let mut child = Document::empty();
    child
        .edit("reflective child", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-40.0, -24.0, 16.0),
                DVec3::new(-8.0, 24.0, 48.0),
                "dev/dev_measuregeneric01",
            )?;
            let ObjectId::Solid(detail) = edit::create_block(
                tx,
                DVec3::new(8.0, -16.0, 16.0),
                DVec3::new(40.0, 16.0, 32.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            edit::tie_to_entity(tx, &[detail], "func_detail", &[])?;
            Ok(())
        })
        .unwrap();
    write_vmf(directory.path(), "child.vmf", child.vmf());
    let mut middle = Document::empty().into_vmf();
    middle
        .entities
        .push(instance(2, "child.vmf", "16 0 0", "90 0 0"));
    write_vmf(directory.path(), "middle.vmf", &middle);
    let (mut parent, helpers) = source();
    let owner = parent.object_ids().max().unwrap() + 1;
    parent
        .entities
        .push(instance(owner, "middle.vmf", "16 24 128", "0 90 0"));
    write_vmf(directory.path(), "parent.vmf", &parent);
    (directory, game, parent, helpers, owner)
}

fn compile(parent: &Vmf, game: &GameFs, directory: &Path, control: &Control) -> Output {
    compile_lighting(parent, game, directory, control, LightingMode::DirectLdr)
}

fn compile_lighting(
    parent: &Vmf,
    game: &GameFs,
    directory: &Path,
    control: &Control,
    lighting: LightingMode,
) -> Output {
    let source = SourceContext::capture(&directory.join("parent.vmf"), control).unwrap();
    compile_with_source_controlled(
        parent,
        game,
        Options {
            lighting,
            generate_reflections: true,
            ..Default::default()
        },
        Some(&CompileContext::new("reflections").unwrap()),
        Some(&source),
        control,
    )
}

fn ready(parent: &Vmf, game: &GameFs, directory: &Path) -> Output {
    let output = compile(parent, game, directory, &Control::default());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    output
}

fn environment() -> CaptureEnvironment {
    CaptureEnvironment {
        engine_root: "/unused/fake-engine".into(),
        steam_root: "/unused/fake-steam".into(),
        mounts: Vec::new(),
    }
}

fn receipt(request: CaptureRequest) -> CaptureReceipt {
    let staged =
        native_runtime::capture_staging_bsp_for_channel(&request.bsp, request.channel).unwrap();
    let mut captured = Bsp::parse(&staged).unwrap();
    let mut archive = captured.pak_data(PakLimits::default()).unwrap();
    for sample in request.samples {
        archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == sample.texture)
            .unwrap()
            .data = native_black_cube(sample.dimension);
    }
    captured
        .set_pak_data(&archive, PakLimits::default())
        .unwrap();
    CaptureReceipt {
        channel: request.channel,
        bsp: captured.to_bytes(),
        staged_bsp: staged,
        artifacts: "/unused/fake-native-evidence".into(),
        elapsed_ms: 1,
        verified_controls: true,
        preservation_passed: true,
    }
}

#[test]
fn automatic_child_world_and_detail_assignments_keep_transformed_source_provenance() {
    let (directory, game, parent, helpers, owner) = fixture();
    let parent_before = parent.clone();
    let original_files = ["parent.vmf", "middle.vmf", "child.vmf"]
        .map(|name| std::fs::read(directory.path().join(name)).unwrap());
    let context =
        SourceContext::capture(&directory.path().join("parent.vmf"), &Control::default()).unwrap();
    let prepared = crate::instances::prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    let output = ready(&parent, &game, directory.path());
    let assignments = &output.cubemap_assets.as_ref().unwrap().assignments;
    let generated = prepared.generated_brushes().collect::<Vec<_>>();
    assert_eq!(generated.len(), 2);
    for brush in generated {
        assert_eq!((brush.owner, brush.file), (owner, "child.vmf"));
        let geometry = map_core::BrushGeometry::from_solid(brush.solid).unwrap();
        let (mins, maxs) = if brush.detail {
            ([0.0, 56.0, 88.0], [32.0, 72.0, 120.0])
        } else {
            ([-8.0, 56.0, 136.0], [40.0, 88.0, 168.0])
        };
        assert!(geometry.mins.abs_diff_eq(DVec3::from_array(mins), 1e-8));
        assert!(geometry.maxs.abs_diff_eq(DVec3::from_array(maxs), 1e-8));
        for side in &brush.solid.sides {
            assert!(
                assignments
                    .iter()
                    .any(|assignment| { assignment.face_id == side.id && !assignment.manual })
            );
        }
        if !brush.detail {
            for (normal, helper) in [(-1.0, helpers[0]), (1.0, helpers[1])] {
                let face = geometry
                    .faces
                    .iter()
                    .find(|face| face.plane.normal.x * normal > 0.99)
                    .unwrap();
                let id = brush.solid.sides[face.side].id;
                assert!(assignments.iter().any(|assignment| {
                    assignment.face_id == id && assignment.helper_id == helper
                }));
            }
        }
    }
    let origins = output.face_origins.as_ref().unwrap();
    assert!((0..origins.len()).any(|face| {
        origins.contributors(face).unwrap().any(|origin| {
            matches!(origin, crate::face_origins::Origin::Instance {
                instance, file, ..
            } if *instance == owner && file == "child.vmf")
        })
    }));
    let finalized = finalize_reflections_with_capture(
        output,
        &CompileContext::new("reflections").unwrap(),
        &game,
        &environment(),
        &Control::default(),
        |request, observer| {
            observer.checkpoint()?;
            Ok(receipt(request))
        },
    )
    .unwrap();
    assert_eq!(
        finalized.cubemap_assets.as_ref().unwrap().phase,
        CubemapAssetPhase::Verified
    );
    finalized
        .face_origins
        .as_ref()
        .unwrap()
        .validate_channel(finalized.bsp.as_ref().unwrap(), bsp::LightingKind::Ldr)
        .unwrap();
    finalized
        .instance_dependencies
        .as_ref()
        .unwrap()
        .verify(&Control::default())
        .unwrap();
    assert_eq!(parent, parent_before);
    for (name, bytes) in ["parent.vmf", "middle.vmf", "child.vmf"]
        .into_iter()
        .zip(original_files)
    {
        assert_eq!(std::fs::read(directory.path().join(name)).unwrap(), bytes);
    }
}

#[test]
fn original_root_manual_binding_remains_legal_and_duplicate_ownership_stays_rejected() {
    let (directory, game, mut parent, helpers, _) = fixture();
    let output = ready(&parent, &game, directory.path());
    let side = output
        .cubemap_assets
        .as_ref()
        .unwrap()
        .assignments
        .iter()
        .find(|assignment| parent.side_ids().any(|id| id == assignment.face_id))
        .unwrap()
        .face_id;
    parent
        .entities
        .iter_mut()
        .find(|entity| entity.id == helpers[0])
        .unwrap()
        .properties
        .set("sides", side.to_string());
    write_vmf(directory.path(), "parent.vmf", &parent);
    let output = ready(&parent, &game, directory.path());
    assert!(
        output
            .cubemap_assets
            .as_ref()
            .unwrap()
            .assignments
            .iter()
            .any(|assignment| assignment.face_id == side
                && assignment.helper_id == helpers[0]
                && assignment.manual)
    );
    parent
        .entities
        .iter_mut()
        .find(|entity| entity.id == helpers[1])
        .unwrap()
        .properties
        .set("sides", side.to_string());
    write_vmf(directory.path(), "parent.vmf", &parent);
    let rejected = compile(&parent, &game, directory.path(), &Control::default());
    assert!(rejected.bsp.is_none());
    assert!(
        rejected
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message.contains("duplicate cubemap ownership") })
    );
}

#[test]
fn missing_root_manual_id_cannot_bind_a_real_generated_child_side() {
    let (directory, game, mut parent, helpers, _) = fixture();
    let automatic = ready(&parent, &game, directory.path());
    let generated = automatic
        .cubemap_assets
        .as_ref()
        .unwrap()
        .assignments
        .iter()
        .find(|assignment| !parent.side_ids().any(|id| id == assignment.face_id))
        .unwrap()
        .face_id;
    parent
        .entities
        .iter_mut()
        .find(|entity| entity.id == helpers[0])
        .unwrap()
        .properties
        .set("sides", generated.to_string());
    write_vmf(directory.path(), "parent.vmf", &parent);
    let output = compile(&parent, &game, directory.path(), &Control::default());
    assert!(output.bsp.is_none());
    assert!(output.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.message.contains("original root map")
            && diagnostic.message.contains(&format!("side:{generated}"))
    }));
    let destination = tempfile::tempdir().unwrap();
    let path = destination.path().join("reflections.bsp");
    std::fs::write(&path, b"previous complete BSP").unwrap();
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(path).unwrap(), b"previous complete BSP");
}

#[test]
fn original_root_manual_binding_still_requires_a_visible_included_compiled_side() {
    for hidden in [false, true] {
        let (directory, game, mut parent, helpers, _) = fixture();
        let floor = parent.world.solids[0]
            .sides
            .iter()
            .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.9)
            .unwrap()
            .id;
        parent
            .entities
            .iter_mut()
            .find(|entity| entity.id == helpers[0])
            .unwrap()
            .properties
            .set("sides", floor.to_string());
        if hidden {
            parent.world.solids[0].hidden = true;
        } else {
            parent.cordon = Some(Node::block(
                "cordon",
                vec![
                    Node::string("active", "1"),
                    Node::string("mins", "-128 -128 16"),
                    Node::string("maxs", "128 128 192"),
                ],
            ));
        }
        write_vmf(directory.path(), "parent.vmf", &parent);
        let output = compile(&parent, &game, directory.path(), &Control::default());
        assert!(output.bsp.is_none());
        assert!(
            output.report.diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains(&format!("side:{floor}"))
                    && diagnostic.message.contains("visible compiled side")
            }),
            "{:?}",
            output.report
        );
    }
}

#[test]
fn unsafe_or_unsupported_child_entities_remain_rejected_before_capture() {
    let (directory, game, parent, _, _) = fixture();
    let child_bytes = std::fs::read(directory.path().join("child.vmf")).unwrap();
    for class in ["point_servercommand", "env_cubemap"] {
        let mut child = Vmf::parse(std::str::from_utf8(&child_bytes).unwrap()).unwrap();
        child.entities.push(Entity::new(1000, class));
        write_vmf(directory.path(), "child.vmf", &child);
        let output = compile(&parent, &game, directory.path(), &Control::default());
        assert!(output.bsp.is_none(), "{class}");
        assert!(output.report.has_errors(), "{class}");
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("child.vmf") })
        );
    }
    std::fs::write(directory.path().join("child.vmf"), child_bytes).unwrap();
    let mut unsafe_root = parent.clone();
    unsafe_root
        .entities
        .push(Entity::new(1000, "point_servercommand"));
    write_vmf(directory.path(), "parent.vmf", &unsafe_root);
    let output = compile(&unsafe_root, &game, directory.path(), &Control::default());
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message.contains("private reflection capture") })
    );
}

#[test]
fn original_child_drift_is_refused_before_capture_and_after_a_receipt() {
    for (lighting, during_capture) in [
        (LightingMode::DirectLdr, false),
        (LightingMode::DirectLdr, true),
        (LightingMode::DirectLdrHdr, true),
    ] {
        let (directory, game, parent, _, _) = fixture();
        let output = compile_lighting(
            &parent,
            &game,
            directory.path(),
            &Control::default(),
            lighting,
        );
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.bsp.is_some());
        let path = directory.path().join("child.vmf");
        let original = std::fs::read(&path).unwrap();
        if !during_capture {
            std::fs::write(&path, b"changed before capture").unwrap();
        }
        let mut calls = 0;
        let result = finalize_reflections_with_capture(
            output,
            &CompileContext::new("reflections").unwrap(),
            &game,
            &environment(),
            &Control::default(),
            |request, _observer| {
                calls += 1;
                let receipt = receipt(request);
                std::fs::write(&path, b"changed during capture")?;
                Ok(receipt)
            },
        );
        assert!(result.is_err());
        assert_eq!(calls, usize::from(during_capture));
        std::fs::write(path, original).unwrap();
    }
}

#[test]
fn capture_cancellation_preserves_sources_and_previous_output() {
    let (directory, game, parent, _, _) = fixture();
    let output = ready(&parent, &game, directory.path());
    let child = std::fs::read(directory.path().join("child.vmf")).unwrap();
    let destination = tempfile::tempdir().unwrap();
    let path = destination.path().join("reflections.bsp");
    std::fs::write(&path, b"previous complete BSP").unwrap();
    let control = Control::default();
    let error = finalize_reflections_with_capture(
        output,
        &CompileContext::new("reflections").unwrap(),
        &game,
        &environment(),
        &control,
        |request, _observer| {
            let receipt = receipt(request);
            control.cancel().unwrap();
            Ok(receipt)
        },
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("cancelled"));
    assert_eq!(std::fs::read(path).unwrap(), b"previous complete BSP");
    assert_eq!(
        std::fs::read(directory.path().join("child.vmf")).unwrap(),
        child
    );
}

#[test]
fn verified_output_keeps_original_dependencies_for_reinspection_and_staged_publication() {
    let (directory, game, parent, _, _) = fixture();
    let output = ready(&parent, &game, directory.path());
    let finalized = finalize_reflections_with_capture(
        output,
        &CompileContext::new("reflections").unwrap(),
        &game,
        &environment(),
        &Control::default(),
        |request, _observer| Ok(receipt(request)),
    )
    .unwrap();
    let destination = tempfile::tempdir().unwrap();
    let path = destination.path().join("reflections.bsp");
    let pointfile = path.with_extension("lin");
    std::fs::write(&path, b"previous complete BSP").unwrap();
    std::fs::write(&pointfile, b"previous pointfile").unwrap();
    let staged = finalized
        .prepare_write_controlled(&path, &Control::default())
        .unwrap()
        .unwrap();
    std::fs::write(
        directory.path().join("middle.vmf"),
        b"changed original branch",
    )
    .unwrap();
    assert!(staged.commit(&Control::default()).is_err());
    assert!(finalized.write(&path).is_err());
    let mut calls = 0;
    assert!(
        finalize_reflections_with_capture(
            finalized,
            &CompileContext::new("reflections").unwrap(),
            &game,
            &environment(),
            &Control::default(),
            |request, _observer| {
                calls += 1;
                Ok(receipt(request))
            },
        )
        .is_err()
    );
    assert_eq!(calls, 0);
    assert_eq!(std::fs::read(path).unwrap(), b"previous complete BSP");
    assert_eq!(std::fs::read(pointfile).unwrap(), b"previous pointfile");
}
