mod common;
#[path = "common/static_prop_model.rs"]
mod static_prop_model;

use std::path::Path;

use compiler::{Control, LightingMode, Options, OutputLease, WriteReservation};
use glam::DVec3;
use map_core::{BrushGeometry, Document, ObjectId, Plane, edit};
use source_formats::bsp::{self, Bsp, LeafAmbient, LightingKind, Model, TexInfo};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::{DispInfo, Vmf};

struct Fixture {
    directory: tempfile::TempDir,
    game: GameFs,
    source: Vmf,
}

fn fixture(receivers: bool) -> Fixture {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/validation/material-emission");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    for folder in [
        "game/materials/dev",
        "game/materials/test",
        "game/materials/models/test",
        "game/materials/models/detl",
        "game/models/props",
        "game/scripts",
        "outputs",
    ] {
        std::fs::create_dir_all(directory.path().join(folder)).unwrap();
    }
    for (name, text) in [
        (
            "dev/dev_measuregeneric01",
            "LightmappedGeneric { %notooltexture 1 $reflectivity \"[0.5 0.5 0.5]\" }",
        ),
        (
            "test/emitter",
            "LightmappedGeneric { %notooltexture 1 $reflectivity \"[0 0 0]\" }",
        ),
        (
            "models/test/one",
            "VertexLitGeneric { $basetexture test/white }",
        ),
        (
            "models/test/two",
            "VertexLitGeneric { $basetexture test/white }",
        ),
    ] {
        std::fs::write(
            directory.path().join(format!("game/materials/{name}.vmt")),
            text,
        )
        .unwrap();
    }
    for name in ["one", "two"] {
        std::fs::write(
            directory
                .path()
                .join(format!("game/materials/models/detl/{name}.vmt")),
            "UnlitGeneric { $basetexture test/white }",
        )
        .unwrap();
    }
    std::fs::write(
        directory.path().join("game/scripts/emission.rad"),
        "ldr: test/emitter 255 0 0 255\nhdr: test/emitter 0 0 255 510\n",
    )
    .unwrap();
    for (extension, data) in ["mdl", "vvd", "dx90.vtx"]
        .into_iter()
        .zip(static_prop_model::model())
    {
        std::fs::write(
            directory
                .path()
                .join(format!("game/models/props/test.{extension}")),
            &data,
        )
        .unwrap();
        let mut detail = data;
        if extension == "mdl" {
            detail[752..764].copy_from_slice(b"models/detl/");
        }
        std::fs::write(
            directory
                .path()
                .join(format!("game/models/props/detail.{extension}")),
            detail,
        )
        .unwrap();
    }
    let mut source = common::room();
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 128;
        let plane = Plane::from_points(side.plane.0).unwrap();
        if plane.normal.z < -0.99 && (plane.dist + 256.0).abs() < 0.001 {
            side.material = "test/emitter".into();
        }
    }
    source
        .world
        .properties
        .set(compiler::MATERIAL_EMISSION_FILE_KEY, "scripts/emission.rad");
    let mut document = Document::new(source);
    if receivers {
        document
            .edit("receivers", |tx| {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(-16.0, -16.0, 32.0),
                    DVec3::new(16.0, 16.0, 64.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                for side in &mut tx.solid_mut(id)?.sides {
                    side.lightmap_scale = 128;
                }
                edit::tie_to_entity(
                    tx,
                    &[id],
                    "func_brush",
                    &[("origin".into(), "0 0 32".into())],
                )?;
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(-160.0, 32.0, 16.0),
                    DVec3::new(-32.0, 160.0, 32.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let solid = tx.solid_mut(id)?;
                for side in &mut solid.sides {
                    side.lightmap_scale = 128;
                }
                let geometry = BrushGeometry::from_solid(solid).unwrap();
                let top = geometry
                    .faces
                    .iter()
                    .find(|face| face.plane.normal.z > 0.99)
                    .unwrap();
                solid.sides[top.side].displacement = Some(DispInfo {
                    power: 2,
                    start_position: top.vertices[0],
                    flags: Some(0),
                    elevation: 0.0,
                    subdivided: false,
                    normals: Some(vec![vec![DVec3::Z; 5]; 5]),
                    distances: Some(vec![vec![0.0; 5]; 5]),
                    offsets: None,
                    offset_normals: None,
                    alphas: None,
                    triangle_tags: None,
                    allowed_verts: None,
                    extra_properties: Default::default(),
                    extra: Vec::new(),
                });
                for (class, origin, model) in [
                    ("prop_static", DVec3::new(96.0, 96.0, 48.0), "test"),
                    ("prop_detail", DVec3::new(-96.0, -96.0, 48.0), "detail"),
                ] {
                    edit::create_entity(
                        tx,
                        class,
                        Some(origin),
                        &[
                            ("model".into(), format!("models/props/{model}.mdl")),
                            ("solid".into(), "0".into()),
                        ],
                        Vec::new(),
                    )?;
                }
                Ok(())
            })
            .unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().join("game")),
    });
    Fixture {
        directory,
        game,
        source: document.into_vmf(),
    }
}

fn options(bounces: u8, receivers: bool) -> Options {
    Options {
        lighting: LightingMode::DirectLdr,
        bounces,
        bake_static_prop_vertices: receivers,
        ..Default::default()
    }
}

fn compile(fixture: &Fixture, source: &Vmf, options: Options) -> compiler::Output {
    let output = compiler::compile_with_options(source, &fixture.game, options);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    output
}

fn energy(bsp: &Bsp, kind: LightingKind, model: Option<usize>, terrain: bool) -> DVec3 {
    let models = bsp.records::<Model>().unwrap();
    let faces = bsp.lighting_faces(kind).unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let lighting = bsp.lighting(kind).unwrap().unwrap();
    let range = model
        .map(|index| {
            let item = models[index];
            item.first_face as usize..(item.first_face + item.face_count) as usize
        })
        .unwrap_or(0..faces.len());
    let mut sum = DVec3::ZERO;
    for face in &faces[range] {
        if terrain && face.dispinfo < 0 {
            continue;
        }
        if let Some(light) = lighting.face(face, &infos[face.texinfo as usize]).unwrap() {
            for color in light.samples(0, 0).unwrap() {
                sum += DVec3::from_array(color.linear());
            }
        }
    }
    sum
}

#[test]
fn emission_only_ldr_reaches_real_receivers_and_probes_without_worldlights() {
    let fixture = fixture(true);
    let mut off = fixture.source.clone();
    off.world
        .properties
        .remove(compiler::MATERIAL_EMISSION_FILE_KEY);
    let baseline = compile(&fixture, &off, options(0, true));
    let enabled = compile(&fixture, &fixture.source, options(0, true));
    let dark = baseline.bsp.as_ref().unwrap();
    let lit = enabled.bsp.as_ref().unwrap();
    assert_eq!(
        dark.lumps[bsp::lump::WORLDLIGHTS].data,
        lit.lumps[bsp::lump::WORLDLIGHTS].data
    );
    assert!(lit.world_lights(LightingKind::Ldr).unwrap().is_empty());
    assert_eq!(energy(dark, LightingKind::Ldr, None, false), DVec3::ZERO);
    for (model, terrain) in [(Some(0), false), (Some(1), false), (Some(0), true)] {
        let value = energy(lit, LightingKind::Ldr, model, terrain);
        assert!(value.x > 0.0, "{model:?} terrain {terrain}: {value:?}");
        assert_eq!(value.y, 0.0);
        assert_eq!(value.z, 0.0);
    }
    assert!(
        lit.detail_props().unwrap().unwrap().props[0]
            .lighting
            .linear()[0]
            > 0.0
    );
    let ambient = lit.leaf_ambient(LightingKind::Ldr).unwrap().unwrap();
    let LeafAmbient::Indexed { samples, .. } = ambient else {
        panic!("Expected indexed ambient")
    };
    assert!(
        samples
            .iter()
            .flat_map(|sample| sample.cube)
            .any(|color| color.linear()[0] > 0.0)
    );
    let manifest =
        compiler::prop_lighting_assets::read_generated_prop_lighting(lit, &Control::default())
            .unwrap()
            .unwrap();
    let archive = lit.pak_data(bsp::PakLimits::default()).unwrap();
    let asset = &manifest.assets[0];
    let bytes = &archive
        .entries
        .iter()
        .find(|entry| entry.name == asset.path)
        .unwrap()
        .data;
    let vertices =
        source_formats::vhv::File::parse(bytes, source_formats::vhv::Limits::default()).unwrap();
    assert!(
        vertices
            .meshes
            .iter()
            .flat_map(|mesh| &mesh.colors)
            .any(|color| color.red > 0)
    );
    let stats = enabled.stats.material_emission.as_ref().unwrap();
    assert!(stats.source_faces > 0 && stats.source_triangles > 0 && stats.channel_rays[0] > 0);
    assert_eq!(stats.channel_rays[1], 0);
    assert!(
        !lit.entities()
            .unwrap()
            .contains(compiler::MATERIAL_EMISSION_FILE_KEY)
    );
    let mut previous_energy = energy(lit, LightingKind::Ldr, None, false).x;
    for bounces in [1, 4] {
        let bounced = compile(&fixture, &fixture.source, options(bounces, true));
        let bounced_energy = energy(
            bounced.bsp.as_ref().unwrap(),
            LightingKind::Ldr,
            None,
            false,
        )
        .x;
        assert!(bounced_energy > previous_energy);
        previous_energy = bounced_energy;
        assert!(
            bounced
                .bsp
                .as_ref()
                .unwrap()
                .world_lights(LightingKind::Ldr)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn hdr_channels_require_a_real_native_source_and_keep_authored_records() {
    let fixture = fixture(false);
    let mut settings = options(0, false);
    settings.lighting = LightingMode::DirectLdrHdr;
    let refused = compiler::compile_with_options(&fixture.source, &fixture.game, settings);
    assert!(refused.report.has_errors());
    assert!(
        refused
            .report
            .diagnostics
            .iter()
            .any(|item| item.message.contains("real authored"))
    );
    let mut document = Document::new(fixture.source.clone());
    document
        .edit("authored black light", |tx| {
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::Z * 128.0),
                &[
                    ("_light".into(), "0 0 0 0".into()),
                    ("_lightHDR".into(), "0 0 0 0".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    let mut off = document.vmf().clone();
    off.world
        .properties
        .remove(compiler::MATERIAL_EMISSION_FILE_KEY);
    let baseline = compile(&fixture, &off, settings);
    let output = compile(&fixture, document.vmf(), settings);
    let bsp = output.bsp.as_ref().unwrap();
    let ldr = energy(bsp, LightingKind::Ldr, None, false);
    let hdr = energy(bsp, LightingKind::Hdr, None, false);
    assert!(ldr.x > 0.0 && ldr.z == 0.0 && hdr.x == 0.0 && hdr.z > ldr.x);
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        assert_eq!(
            baseline.bsp.as_ref().unwrap().world_lights(kind).unwrap(),
            bsp.world_lights(kind).unwrap()
        );
        assert_eq!(bsp.world_lights(kind).unwrap().len(), 1);
    }
    let mut padded = document.vmf().clone();
    for side in padded
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        if side.material == "test/emitter" {
            side.material = " test/emitter ".into();
        }
    }
    let padded = compile(&fixture, &padded, settings);
    for lump in [
        bsp::lump::LIGHTING,
        bsp::lump::LIGHTING_HDR,
        bsp::lump::LEAF_AMBIENT_LIGHTING,
        bsp::lump::LEAF_AMBIENT_LIGHTING_HDR,
    ] {
        assert_eq!(
            padded.bsp.as_ref().unwrap().lumps[lump].data,
            bsp.lumps[lump].data
        );
    }
}

#[test]
fn original_emission_file_guards_publication_and_work_refusal() {
    let fixture = fixture(false);
    let destination = fixture.directory.path().join("outputs/previous.bsp");
    std::fs::write(&destination, b"previous").unwrap();
    let control = Control::default();
    let _lease = OutputLease::acquire(&destination, &control).unwrap();
    let reservation = WriteReservation::new(&destination, &control).unwrap();
    let output = compile(&fixture, &fixture.source, options(0, false));
    std::fs::write(
        fixture.directory.path().join("game/scripts/emission.rad"),
        "test/emitter 0 255 0 200\n",
    )
    .unwrap();
    assert!(
        output
            .prepare_write_reserved(reservation, &control)
            .is_err()
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"previous");
    let mut settings = options(0, false);
    settings.lighting_work_limit = 1;
    let failed = compiler::compile_with_options(&fixture.source, &fixture.game, settings);
    assert!(failed.report.has_errors() && failed.bsp.is_none());
    let mut invalid = fixture.source.clone();
    invalid.world.solids[1]
        .sides
        .iter_mut()
        .find(|side| side.material == "test/emitter")
        .unwrap()
        .material = "test/missing".into();
    std::fs::write(
        fixture.directory.path().join("game/scripts/emission.rad"),
        "test/missing 255 0 0 100\n",
    )
    .unwrap();
    let failed = compiler::compile_with_options(&invalid, &fixture.game, options(0, false));
    assert!(failed.report.has_errors() && failed.bsp.is_none());
}
