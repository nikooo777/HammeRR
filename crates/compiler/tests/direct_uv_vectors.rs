mod common;

use glam::{DVec2, DVec3};
use map_core::{
    Document, ObjectId, Plane, edit,
    surface::{self, FaceId, SurfaceEdit},
    texel_uv,
};
use source_formats::{
    bsp::{self, Face, TexInfo},
    gamefs::{GameFs, Mount, MountedPath},
    vmf::Vmf,
};

fn lightmap_projection_game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials/dev");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(
        materials.join("dev_measuregeneric01.vmt"),
        "LightmappedGeneric { }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().into()),
    });
    (directory, game)
}

fn thin_projected_room(magnitude: f64) -> (Vmf, u32) {
    let mut source = common::room();
    for solid in &mut source.world.solids {
        for side in &mut solid.sides {
            for point in &mut side.plane.0 {
                point.x = point.x.signum() * (0.125 + (point.x.abs() - 256.0).max(0.0) / 8.0);
                point.y /= 8.0;
                point.z /= 8.0;
            }
        }
    }
    for entity in &mut source.entities {
        entity.properties.set("origin", "0 0 8");
    }
    let mut document = Document::new(source);
    document
        .edit("point light", |tx| {
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(0.0, 0.0, 16.0)),
                &[("_light".into(), "255 255 255 12".into())],
                Vec::new(),
            )
        })
        .unwrap();
    let mut source = document.vmf().clone();
    let side = source.world.solids[0]
        .sides
        .iter_mut()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
        .unwrap();
    side.lightmap_scale = 16;
    side.u_axis.axis = DVec3::new(magnitude, 0.0, 0.0);
    side.u_axis.scale = 0.25;
    side.u_axis.offset = 0.0;
    side.v_axis.axis = DVec3::new(0.0, -0.001, 0.0);
    side.v_axis.scale = 0.25;
    side.v_axis.offset = 6.4;
    let id = side.id;
    let source = Vmf::parse(&source.try_write(&Default::default()).unwrap()).unwrap();
    (source, id)
}

#[test]
fn finite_no_progress_lightmap_splits_fail_and_preserve_bsp_and_pointfile() {
    let (directory, game) = lightmap_projection_game();
    let (ordinary, _) = thin_projected_room(1.0);
    let (source, side) = thin_projected_room(64_000.0);
    let original = source.clone();
    for lighting in [
        compiler::LightingMode::Fullbright,
        compiler::LightingMode::DirectLdr,
    ] {
        let options = compiler::Options {
            lighting,
            ..Default::default()
        };
        let destination = directory.path().join(format!("{}.bsp", lighting.name()));
        let pointfile = destination.with_extension("lin");
        let baseline = compiler::compile_with_options(&ordinary, &game, options);
        assert!(!baseline.report.has_errors(), "{:?}", baseline.report);
        assert!(baseline.write(&destination).unwrap());
        let previous = std::fs::read(&destination).unwrap();
        std::fs::write(&pointfile, b"previous pointfile").unwrap();

        let output = compiler::compile_with_options(&source, &game, options);
        assert!(output.bsp.is_none());
        assert!(output.leak.is_none());
        assert!(
            output.report.diagnostics.iter().any(|diagnostic| {
                diagnostic.level == compiler::Level::Error
                    && diagnostic.message.contains(&format!("side:{side}:"))
                    && diagnostic.message.contains("Lightmap subdivision")
                    && diagnostic.message.contains("extent 1000")
            }),
            "{:?}",
            output.report
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), previous);
        assert_eq!(std::fs::read(&pointfile).unwrap(), b"previous pointfile");
        let fresh = directory
            .path()
            .join(format!("fresh-{}.bsp", lighting.name()));
        assert!(!output.write(&fresh).unwrap());
        assert!(!fresh.exists());
        assert!(!fresh.with_extension("lin").exists());
        assert_eq!(source, original);
    }
}

#[test]
fn ordinary_and_subdividing_authored_projections_keep_literal_lightmap_bounds() {
    let (_directory, game) = lightmap_projection_game();
    for lighting in [
        compiler::LightingMode::Fullbright,
        compiler::LightingMode::DirectLdr,
    ] {
        for (magnitude, expected) in [
            (1.0, vec![([-1, 0], [2, 1])]),
            (4096.0, vec![([-32, 0], [32, 1]), ([0, 0], [32, 1])]),
        ] {
            let (source, _) = thin_projected_room(magnitude);
            let output = compiler::compile_with_options(
                &source,
                &game,
                compiler::Options {
                    lighting,
                    ..Default::default()
                },
            );
            assert!(!output.report.has_errors(), "{:?}", output.report);
            assert!(output.leak.is_none());
            assert!(output.stats.lightmap_samples < 256);
            let bsp = output.bsp.unwrap();
            let planes = bsp.records::<bsp::Plane>().unwrap();
            let infos = bsp.records::<TexInfo>().unwrap();
            let faces = bsp.records::<Face>().unwrap();
            let floor = faces
                .iter()
                .filter(|face| {
                    let plane = planes[face.plane as usize];
                    plane.normal == [0.0, 0.0, 1.0] && plane.dist == 0.0
                })
                .collect::<Vec<_>>();
            let mut bounds = floor
                .iter()
                .map(|face| (face.lightmap_mins, face.lightmap_size))
                .collect::<Vec<_>>();
            bounds.sort_unstable();
            assert_eq!(bounds, expected);
            for face in floor {
                assert_eq!(
                    infos[face.texinfo as usize].lightmap_vecs[0],
                    [(magnitude / 16.0) as f32, 0.0, 0.0, 0.0]
                );
                assert_eq!(
                    face.light_offset >= 0,
                    lighting == compiler::LightingMode::DirectLdr
                );
            }
            assert!(
                faces
                    .iter()
                    .all(|face| face.lightmap_size.iter().all(|&extent| extent <= 32))
            );
        }
    }
}

#[test]
fn extreme_finite_source_projections_fail_without_panics_or_output_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("previous.bsp");
    std::fs::write(&destination, b"previous output").unwrap();
    for (axis, scale, offset, expected) in [
        (1e12, 0.25, 0.0, "Lightmap bounds"),
        (1.0, 1e-300, 0.0, "finite native floats"),
        (1e300, 0.25, 0.0, "finite native floats"),
        (1.0, 0.25, 1e300, "finite native floats"),
    ] {
        let mut source = common::room();
        for side in &mut source.world.solids[0].sides {
            side.u_axis.axis.x = axis;
            side.u_axis.scale = scale;
            side.u_axis.offset = offset;
        }
        let source = Vmf::parse(&source.try_write(&Default::default()).unwrap()).unwrap();
        let output = compiler::compile(&source, &common::empty_game());
        assert!(output.bsp.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.level == compiler::Level::Error
                    && diagnostic.message.contains(expected)),
            "{:?}",
            output.report
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous output");
    }
}

fn projected(rows: [[f32; 4]; 2], point: DVec3) -> DVec2 {
    DVec2::from_array(rows.map(|row| {
        point.dot(DVec3::new(
            f64::from(row[0]),
            f64::from(row[1]),
            f64::from(row[2]),
        )) + f64::from(row[3])
    }))
}

#[test]
fn authored_skew_vectors_preserve_known_texels_through_vmf_and_world_or_model_bsp() {
    for owned in [false, true] {
        let mut document = Document::new(common::room());
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(0.0, 0.0, 16.0),
                    DVec3::new(128.0, 64.0, 80.0),
                    "uv/authored_skew",
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        let face = FaceId {
            solid: id,
            side: document
                .solid(id)
                .unwrap()
                .sides
                .iter()
                .find(|side| Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
                .unwrap()
                .id,
        };
        let pivot = if owned {
            DVec3::new(80.125, 48.25, 32.5)
        } else {
            DVec3::ZERO
        };
        if owned {
            document
                .edit("model", |tx| {
                    edit::tie_to_entity(
                        tx,
                        &[id],
                        "func_brush",
                        &[("origin".into(), "80.125 48.25 32.5".into())],
                    )
                })
                .unwrap();
        }
        document
            .edit("author vectors", |tx| {
                surface::edit_faces(
                    tx,
                    &[face],
                    &SurfaceEdit {
                        u_axis: Some([Some(2.0), Some(0.5), Some(0.125)]),
                        v_axis: Some([Some(0.25), Some(-1.5), Some(0.375)]),
                        scale_u: Some(-0.5),
                        scale_v: Some(0.25),
                        shift_u: Some(13.125),
                        shift_v: Some(-27.5),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let authored = surface::side(&document, face).unwrap().clone();
        let known = DVec3::new(16.0, 24.0, 80.0);
        assert_eq!(texel_uv(&authored, known), DVec2::new(-94.875, -35.5));
        let reopened = Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        let document = Document::new(reopened);
        let source = surface::side(&document, face).unwrap();
        assert_eq!(source, &authored);
        let expected = [source.u_axis, source.v_axis].map(|axis| {
            let vector = axis.axis / axis.scale;
            [
                vector.x as f32,
                vector.y as f32,
                vector.z as f32,
                (axis.offset + pivot.dot(vector)) as f32,
            ]
        });
        let output = compiler::compile(document.vmf(), &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.leak.is_none());
        let bsp = output.bsp.unwrap();
        let infos = bsp.records::<TexInfo>().unwrap();
        let faces = bsp.records::<Face>().unwrap();
        let matches = infos
            .iter()
            .enumerate()
            .filter(|(_, info)| info.texture_vecs == expected)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert!(!matches.is_empty());
        assert!(
            faces
                .iter()
                .any(|face| matches.contains(&(face.texinfo as usize)))
        );
        for index in matches {
            for point in surface::face_vertices(&document, face)
                .unwrap()
                .into_iter()
                .chain([known])
            {
                assert!(
                    projected(infos[index].texture_vecs, point - pivot)
                        .abs_diff_eq(texel_uv(source, point), 1e-5)
                );
            }
        }
    }
}
