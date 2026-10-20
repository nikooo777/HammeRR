use super::*;

const FLOOR_MATERIAL: &str = "spherical/floor";
const FLOOR_TEXTURE: &str = "materials/spherical/floor.vtf";

fn entity(vmf: &mut Vmf, id: u32) -> &mut Entity {
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
}

fn fixture(sky_ambient: bool) -> (tempfile::TempDir, GameFs, Vmf) {
    let (directory, game) = fixtures::game();
    fixtures::add_lod(directory.path());
    std::fs::create_dir_all(directory.path().join("materials/spherical")).unwrap();
    std::fs::write(
        directory.path().join("materials/spherical/floor.vmt"),
        "LightmappedGeneric { $basetexture spherical/floor }",
    )
    .unwrap();
    let mut texture = fixtures::caster_texture(255);
    let rho = if sky_ambient {
        [0.0; 3]
    } else {
        [0.5, 0.25, 0.125]
    };
    for (axis, value) in rho.into_iter().enumerate() {
        texture[32 + axis * 4..36 + axis * 4].copy_from_slice(&f32::to_le_bytes(value));
    }
    std::fs::write(directory.path().join(FLOOR_TEXTURE), &texture).unwrap();
    for axis in 0..3 {
        texture[32 + axis * 4..36 + axis * 4].copy_from_slice(&0.0_f32.to_le_bytes());
    }
    std::fs::write(directory.path().join("materials/test.vtf"), texture).unwrap();
    std::fs::write(
        directory.path().join("materials/spherical/nodraw.vmt"),
        "LightmappedGeneric { %compilenodraw 1 }",
    )
    .unwrap();
    let mut vmf = fixtures::scene(false);
    vmf.world.solids.clear();
    let mut document = Document::new(vmf);
    document
        .edit("constant reflected floor with sky enclosure", |tx| {
            for (mins, maxs, material) in [
                (
                    [-1024.0, -1024.0, -16.0],
                    [1024.0, 1024.0, 0.0],
                    FLOOR_MATERIAL,
                ),
                (
                    [-2064.0, -2064.0, -16.0],
                    [-1024.0, 2064.0, 0.0],
                    "spherical/nodraw",
                ),
                (
                    [1024.0, -2064.0, -16.0],
                    [2064.0, 2064.0, 0.0],
                    "spherical/nodraw",
                ),
                (
                    [-1024.0, -2064.0, -16.0],
                    [1024.0, -1024.0, 0.0],
                    "spherical/nodraw",
                ),
                (
                    [-1024.0, 1024.0, -16.0],
                    [1024.0, 2064.0, 0.0],
                    "spherical/nodraw",
                ),
                ([-2064.0, -2064.0, 256.0], [2064.0, 2064.0, 272.0], "sky"),
                ([-2064.0, -2064.0, 0.0], [-2048.0, 2064.0, 256.0], "sky"),
                ([2048.0, -2064.0, 0.0], [2064.0, 2064.0, 256.0], "sky"),
                ([-2048.0, -2064.0, 0.0], [2048.0, -2048.0, 256.0], "sky"),
                ([-2048.0, 2048.0, 0.0], [2048.0, 2064.0, 256.0], "sky"),
            ] {
                edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    material,
                )?;
            }
            Ok(())
        })
        .unwrap();
    let mut vmf = document.vmf().clone();
    for side in vmf
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 128;
    }
    for (key, value) in [
        ("origin", "0 0 1"),
        ("ignorenormals", "1"),
        ("disableshadows", "1"),
    ] {
        entity(&mut vmf, 9101).properties.set(key, value);
    }
    for (key, value) in [
        ("classname", "light_environment"),
        ("pitch", "-90"),
        (
            "_light",
            if sky_ambient {
                "0"
            } else {
                "255 255 255 64.125"
            },
        ),
        (
            "_lightHDR",
            if sky_ambient {
                "0"
            } else {
                "255 255 255 128.125"
            },
        ),
        ("_ambient", if sky_ambient { "255 0 0 40" } else { "0" }),
        ("_ambientHDR", if sky_ambient { "0 0 255 80" } else { "0" }),
    ] {
        entity(&mut vmf, 9001).properties.set(key, value);
    }
    (directory, game, vmf)
}

fn assert_floor(bsp: &Bsp, kind: LightingKind, expected: [f64; 3]) {
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let infos = bsp.records::<bsp::TexInfo>().unwrap();
    let faces = bsp.lighting_faces(kind).unwrap();
    let lighting = bsp.lighting(kind).unwrap().unwrap();
    let mut found = 0;
    for face in faces.iter().filter(|face| {
        let plane = planes[usize::from(face.plane)];
        plane.normal == [0.0, 0.0, 1.0] && plane.dist == 0.0 && face.light_offset >= 0
    }) {
        let light = lighting
            .face(face, &infos[face.texinfo as usize])
            .unwrap()
            .unwrap();
        assert_eq!(light.average(0).unwrap().linear(), expected);
        assert!(
            light
                .samples(0, 0)
                .unwrap()
                .iter()
                .all(|color| color.linear() == expected)
        );
        found += 1;
    }
    assert!(found > 0);
}

fn assert_colors(bsp: &Bsp, name: &str, expected: [u8; 4]) {
    let file = file(bsp, name);
    assert_eq!(
        file.meshes
            .iter()
            .map(|mesh| mesh.layout)
            .collect::<Vec<_>>(),
        [
            vhv::MeshLayout {
                lod: 0,
                vertex_count: 6
            },
            vhv::MeshLayout {
                lod: 1,
                vertex_count: 4
            },
        ]
    );
    let [blue, green, red, alpha] = expected;
    for mesh in file.meshes {
        assert!(
            mesh.colors.iter().all(|color| *color
                == vhv::Bgra8 {
                    blue,
                    green,
                    red,
                    alpha
                }),
            "{name}: {:?}",
            mesh.colors
        );
    }
}

#[test]
fn spherical_floor_energy_uses_literal_channels_all_lods_and_retained_whole_vtf_guard() {
    let (directory, game, vmf) = fixture(false);
    let before = vmf.clone();
    for (quality, normal, bounces) in [
        (SkyQuality::Fast, [0.0, 0.0, 1.0], 1),
        (SkyQuality::Balanced, [0.0, 0.0, -1.0], 1),
        (SkyQuality::Detailed, [1.0, 2.0, -3.0], 1),
        (SkyQuality::Fast, [0.0, 0.0, -1.0], 4),
    ] {
        fixtures::normals(directory.path(), normal);
        let output_directory = tempfile::tempdir_in(directory.path().parent().unwrap()).unwrap();
        let output = crate::compile_with_options(
            &vmf,
            &game,
            Options {
                bounces,
                sky_quality: quality,
                ..options(true)
            },
        );
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let bsp = output.bsp.as_ref().unwrap();
        assert_floor(bsp, LightingKind::Ldr, [64.0; 3]);
        assert_floor(bsp, LightingKind::Hdr, [128.0; 3]);
        assert_colors(bsp, "sp_0.vhv", [51, 58, 70, 255]);
        assert_colors(bsp, "sp_hdr_0.vhv", [69, 79, 95, 255]);
        assert_eq!(
            output.stats.prop_vertex_lighting.as_ref().unwrap().vertices,
            10
        );
        let manifest =
            crate::prop_lighting_assets::read_generated_prop_lighting(bsp, &Control::default())
                .unwrap()
                .unwrap();
        let original = std::fs::read(directory.path().join(FLOOR_TEXTURE)).unwrap();
        let captured = manifest
            .sources
            .iter()
            .find(|source| source.path == FLOOR_TEXTURE)
            .unwrap();
        assert_eq!(captured.bytes, original.len());
        assert_eq!(captured.sha256, crate::asset_sources::hash(&original));
        assert!(
            !manifest
                .sources
                .iter()
                .any(|source| source.path == "materials/test.vtf")
        );
        assert_eq!(vmf, before);
        if bounces == 4 {
            let destination = output_directory.path().join("previous.bsp");
            std::fs::write(&destination, b"previous complete output").unwrap();
            let control = Control::default();
            let prepared = output
                .prepare_write_controlled(&destination, &control)
                .unwrap()
                .unwrap();
            overwrite(
                &directory.path().join(FLOOR_TEXTURE),
                (original.len() - 1) as u64,
                &[0],
            );
            drop(output);
            assert!(prepared.commit(&control).is_err());
            assert_eq!(
                std::fs::read(&destination).unwrap(),
                b"previous complete output"
            );
        }
    }
}

#[test]
fn spherical_indirect_excludes_sky_and_preserves_direct_ambient_unit_mean() {
    let (directory, game, vmf) = fixture(true);
    fixtures::normals(directory.path(), [0.0, 0.0, -1.0]);
    let output = crate::compile_with_options(
        &vmf,
        &game,
        Options {
            bounces: 4,
            ..options(true)
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let bsp = output.bsp.as_ref().unwrap();
    assert_floor(bsp, LightingKind::Ldr, [40.0, 0.0, 0.0]);
    assert_floor(bsp, LightingKind::Hdr, [0.0, 0.0, 80.0]);
    assert_colors(bsp, "sp_0.vhv", [0, 0, 40, 255]);
    assert_colors(bsp, "sp_hdr_0.vhv", [55, 0, 0, 255]);
}
