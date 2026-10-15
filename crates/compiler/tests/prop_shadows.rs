mod common;

use compiler::lighting::LuxelFrame;
use compiler::{LightingMode, Options};
use glam::DVec3;
use map_core::Plane;
use source_formats::{
    bsp::{self, Bsp, Face, LightingKind, TexInfo},
    gamefs::{GameFs, Mount, MountedPath},
    vmf::{Entity, Vmf},
};

fn put_int(data: &mut [u8], offset: usize, value: i32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_float(data: &mut [u8], offset: usize, value: f32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_string(data: &mut [u8], offset: usize, value: &str) {
    data[offset..offset + value.len()].copy_from_slice(value.as_bytes());
}

fn fixture() -> [Vec<u8>; 3] {
    let mut mdl = vec![0; 1024];
    mdl[..4].copy_from_slice(b"IDST");
    for (offset, value) in [
        (4, 48),
        (8, 7),
        (76, 1024),
        (152, 16),
        (204, 2),
        (208, 256),
        (212, 1),
        (216, 384),
        (220, 1),
        (224, 2),
        (228, 388),
        (232, 1),
        (236, 400),
        (256, 464),
        (320, 416),
        (384, 752),
        (404, 1),
        (408, 1),
        (412, 16),
        (488, 1),
        (492, 160),
        (496, 4),
        (500, 0),
        (576, 0),
        (580, -160),
        (584, 4),
        (588, 0),
    ] {
        put_int(&mut mdl, offset, value);
    }
    for offset in [116, 140] {
        put_float(&mut mdl, offset, 1.0);
        put_float(&mut mdl, offset + 4, 1.0);
    }
    mdl[390..392].copy_from_slice(&1u16.to_le_bytes());
    put_string(&mut mdl, 720, "one");
    put_string(&mut mdl, 736, "two");
    put_string(&mut mdl, 752, "models/test/");
    let mut vvd = vec![0; 256];
    vvd[..4].copy_from_slice(b"IDSV");
    for (offset, value) in [(4, 4), (8, 7), (12, 1), (16, 4), (56, 64)] {
        put_int(&mut vvd, offset, value);
    }
    for (index, point) in [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ]
    .into_iter()
    .enumerate()
    {
        let offset = 64 + index * 48;
        for (axis, value) in point.into_iter().enumerate() {
            put_float(&mut vvd, offset + 16 + axis * 4, value);
        }
        put_float(&mut vvd, offset + 36, 1.0);
        put_float(&mut vvd, offset + 40, point[0]);
        put_float(&mut vvd, offset + 44, point[1]);
    }
    let mut vtx = vec![0; 173];
    for (offset, value) in [
        (0, 7),
        (16, 7),
        (20, 1),
        (28, 1),
        (32, 36),
        (36, 1),
        (40, 8),
        (44, 1),
        (48, 8),
        (52, 1),
        (56, 12),
        (64, 1),
        (68, 9),
        (73, 4),
        (77, 25),
        (81, 6),
        (85, 61),
        (89, 1),
        (93, 73),
        (146, 6),
        (150, 0),
        (154, 4),
        (158, 0),
    ] {
        put_int(&mut vtx, offset, value);
    }
    for index in 0..4 {
        vtx[98 + index * 9 + 4..98 + index * 9 + 6].copy_from_slice(&(index as u16).to_le_bytes());
    }
    for (index, value) in [0u16, 1, 2, 0, 2, 3].into_iter().enumerate() {
        vtx[134 + index * 2..136 + index * 2].copy_from_slice(&value.to_le_bytes());
    }
    vtx[164] = 1;
    for offset in [104, 108, 128, 132] {
        put_float(&mut mdl, offset, -80.0);
    }
    for offset in [116, 120, 140, 144] {
        put_float(&mut mdl, offset, 80.0);
    }
    for (index, point) in [
        [-24.0, -24.0, 0.0],
        [24.0, -24.0, 0.0],
        [0.0, 24.0, 0.0],
        [0.0, 0.0, 0.0],
    ]
    .into_iter()
    .enumerate()
    {
        for (axis, value) in point.into_iter().enumerate() {
            put_float(&mut vvd, 64 + index * 48 + 16 + axis * 4, value);
        }
    }
    put_int(&mut vtx, 146, 3);
    [mdl, vvd, vtx]
}

fn floor_sample(bsp: &Bsp, point: DVec3) -> DVec3 {
    let faces = bsp.records::<Face>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let lighting = bsp.lighting(LightingKind::Ldr).unwrap().unwrap();
    for face in &faces {
        let plane = planes[face.plane as usize];
        if plane.normal[2] < 0.99 || plane.dist.abs() > 0.01 {
            continue;
        }
        let info = &infos[face.texinfo as usize];
        let frame = LuxelFrame::new(
            Plane {
                normal: DVec3::from_array(plane.normal.map(f64::from)),
                dist: f64::from(plane.dist),
            },
            info,
            face.lightmap_mins,
            face.lightmap_size,
        )
        .unwrap();
        let samples = lighting.face(face, info).unwrap().unwrap();
        let [width, height] = frame.dimensions();
        for t in 0..height {
            for s in 0..width {
                if frame.position(s, t).unwrap().distance(point) < 0.001 {
                    return DVec3::from_array(
                        samples.samples(0, 0).unwrap()[t * width + s].linear(),
                    );
                }
            }
        }
    }
    panic!("No baked floor sample at {point}")
}

fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    for folder in ["models/props", "materials/models/test"] {
        std::fs::create_dir_all(directory.path().join(folder)).unwrap();
    }
    for (suffix, bytes) in ["mdl", "vvd", "dx90.vtx"].into_iter().zip(fixture()) {
        std::fs::write(
            directory.path().join(format!("models/props/test.{suffix}")),
            bytes,
        )
        .unwrap();
    }
    for (name, text) in [
        ("models/test/one", "VertexLitGeneric { $basetexture test }"),
        (
            "models/test/two",
            "VertexLitGeneric { $alphatest 1 $basetexture test }",
        ),
        ("sky", "UnlitGeneric { %compilesky 1 }"),
    ] {
        std::fs::write(directory.path().join(format!("materials/{name}.vmt")), text).unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn scene() -> Vmf {
    let mut vmf = common::room();
    let mut light = Entity::new(9001, "light");
    for (key, value) in [
        ("origin", "0 0 100"),
        ("_light", "255 0 0 200"),
        ("_quadratic_attn", "1"),
    ] {
        light.properties.set(key, value);
    }
    vmf.entities.push(light);
    let mut prop = Entity::new(9002, "prop_static");
    for (key, value) in [
        ("origin", "0 0 50"),
        ("model", "models/props/test.mdl"),
        ("solid", "0"),
    ] {
        prop.properties.set(key, value);
    }
    vmf.entities.push(prop);
    vmf
}

fn options() -> Options {
    Options {
        lighting: LightingMode::DirectLdr,
        ..Options::default()
    }
}

fn alpha_game() -> (tempfile::TempDir, GameFs) {
    let (directory, game) = game();
    let path = directory.path().join("models/props/test.mdl");
    let mut mdl = std::fs::read(&path).unwrap();
    put_int(&mut mdl, 152, 0x0004_0010);
    std::fs::write(path, mdl).unwrap();
    let path = directory.path().join("models/props/test.vvd");
    let mut vvd = std::fs::read(&path).unwrap();
    for (index, uv) in [[0.0, 0.0], [1.0, 0.0], [0.5, 1.0]].into_iter().enumerate() {
        for (axis, value) in uv.into_iter().enumerate() {
            put_float(&mut vvd, 64 + index * 48 + 40 + axis * 4, value);
        }
    }
    std::fs::write(path, vvd).unwrap();
    std::fs::write(directory.path().join("materials/test.vtf"), alpha_texture()).unwrap();
    (directory, game)
}

fn alpha_texture() -> Vec<u8> {
    let mut bytes = vec![0; 80];
    bytes[..4].copy_from_slice(b"VTF\0");
    put_int(&mut bytes, 4, 7);
    put_int(&mut bytes, 12, 80);
    bytes[16..18].copy_from_slice(&4_u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&4_u16.to_le_bytes());
    put_int(&mut bytes, 20, 0x1000);
    bytes[24..26].copy_from_slice(&1_u16.to_le_bytes());
    bytes[56] = 1;
    put_int(&mut bytes, 57, -1);
    for _ in 0..4 {
        for alpha in [255, 255, 0, 0] {
            bytes.extend_from_slice(&[255, 255, 255, alpha]);
        }
    }
    bytes
}

fn compile(vmf: &Vmf, game: &GameFs) -> compiler::Output {
    let output = compiler::compile_with_options(vmf, game, options());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    output
}

#[test]
fn opaque_prop_uses_mesh_holes_not_bounds_and_preserves_source() {
    let (_directory, game) = game();
    let mut vmf = scene();
    let before = vmf.clone();
    let shadowed = compile(&vmf, &game);
    assert_eq!(shadowed.stats.prop_shadow_props, 1);
    assert_eq!(shadowed.stats.prop_shadow_models, 1);
    assert_eq!(shadowed.stats.prop_shadow_triangles, 1);
    assert!(shadowed.stats.prop_shadow_work > 0);
    let shadowed = shadowed.bsp.unwrap();
    assert_eq!(floor_sample(&shadowed, DVec3::ZERO), DVec3::ZERO);
    let hole = floor_sample(&shadowed, DVec3::new(48.0, 48.0, 0.0));
    assert!(hole.x > 0.0);
    assert_eq!(vmf, before);
    vmf.entities
        .last_mut()
        .unwrap()
        .properties
        .set("disableshadows", "1");
    let clear = compile(&vmf, &game);
    assert_eq!(clear.stats.prop_shadow_triangles, 0);
    assert_eq!(clear.stats.prop_shadow_disabled, 1);
    let clear = clear.bsp.unwrap();
    assert_eq!(floor_sample(&clear, DVec3::ZERO).x, 200.0);
    assert_eq!(floor_sample(&clear, DVec3::new(48.0, 48.0, 0.0)), hole);
    assert_eq!(
        clear.static_props().unwrap().unwrap().props[0].flags & 16,
        16
    );
}

#[test]
fn transformed_prop_spot_sun_and_skin_materials_use_shared_shadows() {
    let (_directory, game) = game();
    let mut vmf = scene();
    let prop_index = vmf.entities.len() - 1;
    let light_index = prop_index - 1;
    vmf.entities[prop_index].properties.set("angles", "90 0 0");
    assert_eq!(
        floor_sample(&compile(&vmf, &game).bsp.unwrap(), DVec3::ZERO).x,
        200.0
    );
    vmf.entities[prop_index].properties.set("angles", "0 90 0");
    vmf.entities[prop_index].properties.set("origin", "32 0 50");
    vmf.entities[light_index]
        .properties
        .set("origin", "32 0 100");
    assert_eq!(
        floor_sample(&compile(&vmf, &game).bsp.unwrap(), DVec3::X * 32.0),
        DVec3::ZERO
    );
    vmf.entities[prop_index].properties.set("origin", "0 0 50");
    vmf.entities[light_index]
        .properties
        .set("origin", "0 0 100");
    vmf.entities[light_index]
        .properties
        .set("classname", "light_spot");
    for (key, value) in [
        ("pitch", "-90"),
        ("_inner_cone", "30"),
        ("_cone", "45"),
        ("_exponent", "1"),
    ] {
        vmf.entities[light_index].properties.set(key, value);
    }
    assert_eq!(
        floor_sample(&compile(&vmf, &game).bsp.unwrap(), DVec3::ZERO),
        DVec3::ZERO
    );
    vmf.entities[prop_index].properties.set("skin", "1");
    let alpha = compile(&vmf, &game);
    assert!(
        alpha
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object.as_deref() == Some("entity:9002")
                    && diagnostic.message.contains("texture alpha")
            )
    );
    assert_eq!(floor_sample(&alpha.bsp.unwrap(), DVec3::ZERO), DVec3::ZERO);
    vmf.entities[light_index]
        .properties
        .set("classname", "light_environment");
    vmf.entities[light_index].properties.set("_ambient", "0");
    for solid in &mut vmf.world.solids {
        if solid
            .sides
            .iter()
            .all(|side| side.plane.0.iter().all(|point| point.z >= 256.0))
        {
            for side in &mut solid.sides {
                side.material = "sky".into();
            }
        }
    }
    assert_eq!(
        floor_sample(&compile(&vmf, &game).bsp.unwrap(), DVec3::ZERO),
        DVec3::ZERO
    );
    vmf.entities[prop_index]
        .properties
        .set("disableshadows", "1");
    assert_eq!(
        floor_sample(&compile(&vmf, &game).bsp.unwrap(), DVec3::ZERO).x,
        200.0
    );
}

#[test]
fn failed_shadow_assets_preserve_bsp_while_fullbright_and_disabled_props_need_only_header() {
    let (directory, game) = game();
    let mut vmf = scene();
    let destination = directory.path().join("existing.bsp");
    let good = compile(&vmf, &game);
    good.write(&destination).unwrap();
    let old = std::fs::read(&destination).unwrap();
    let path = directory.path().join("models/props/test.dx90.vtx");
    let mut bytes = std::fs::read(&path).unwrap();
    put_int(&mut bytes, 16, 8);
    std::fs::write(&path, bytes).unwrap();
    let before = vmf.clone();
    let failed = compiler::compile_with_options(&vmf, &game, options());
    assert!(failed.bsp.is_none());
    assert!(
        failed
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object.as_deref() == Some("entity:9002")
                    && diagnostic.message.contains("checksum")
            )
    );
    assert!(!failed.write(&destination).unwrap());
    assert_eq!(std::fs::read(&destination).unwrap(), old);
    assert_eq!(vmf, before);
    assert!(compiler::compile(&vmf, &game).bsp.is_some());
    vmf.entities
        .last_mut()
        .unwrap()
        .properties
        .set("disableshadows", "1");
    assert!(compile(&vmf, &game).bsp.is_some());
    vmf.entities.last_mut().unwrap().hidden = true;
    let hidden = compile(&vmf, &game);
    assert_eq!(hidden.stats.prop_shadow_props, 0);
    assert_eq!(hidden.stats.prop_shadow_disabled, 0);
}

#[test]
fn ambient_probe_moves_outside_closed_prop_instead_of_baking_black_center() {
    use source_formats::bsp::{Leaf, LeafAmbient};
    let (directory, game) = game();
    let [mdl, mut vvd, mut vtx] = fixture();
    for (index, point) in [
        [-64.0, -64.0, -64.0],
        [64.0, -64.0, -64.0],
        [0.0, 64.0, -64.0],
        [0.0, 0.0, 64.0],
    ]
    .into_iter()
    .enumerate()
    {
        for (axis, value) in point.into_iter().enumerate() {
            put_float(&mut vvd, 64 + index * 48 + 16 + axis * 4, value);
        }
    }
    let strip = vtx[146..173].to_vec();
    vtx.resize(185, 0);
    vtx[158..185].copy_from_slice(&strip);
    put_int(&mut vtx, 81, 12);
    put_int(&mut vtx, 93, 85);
    put_int(&mut vtx, 158, 12);
    for (index, value) in [0_u16, 2, 1, 0, 1, 3, 1, 2, 3, 2, 0, 3]
        .into_iter()
        .enumerate()
    {
        vtx[134 + index * 2..136 + index * 2].copy_from_slice(&value.to_le_bytes());
    }
    for (suffix, bytes) in ["mdl", "vvd", "dx90.vtx"].into_iter().zip([mdl, vvd, vtx]) {
        std::fs::write(
            directory.path().join(format!("models/props/test.{suffix}")),
            bytes,
        )
        .unwrap();
    }
    let mut vmf = scene();
    let prop = vmf.entities.len() - 1;
    vmf.entities[prop].properties.set("origin", "0 0 128");
    vmf.entities[prop - 1].properties.set("origin", "160 0 128");
    let output = compile(&vmf, &game);
    assert_eq!(output.stats.prop_shadow_triangles, 4);
    let bsp = output.bsp.unwrap();
    let leaves = bsp.records::<Leaf>().unwrap();
    let LeafAmbient::Indexed { indices, samples } =
        bsp.leaf_ambient(LightingKind::Ldr).unwrap().unwrap()
    else {
        panic!("Expected indexed ambient samples");
    };
    assert!(!samples.is_empty());
    assert!(samples.iter().any(|sample| {
        sample
            .cube
            .iter()
            .any(|color| color.rgb.iter().any(|&channel| channel > 0))
    }));
    for (leaf, index) in leaves.iter().zip(indices) {
        for sample in &samples[usize::from(index.first_sample)
            ..usize::from(index.first_sample) + usize::from(index.sample_count)]
        {
            let point = DVec3::from_array(sample.world_position(leaf).unwrap());
            assert!(
                !(point.cmpge(DVec3::new(-64.001, -64.001, 63.999)).all()
                    && point.cmple(DVec3::new(64.001, 64.001, 192.001)).all()),
                "Probe remained inside prop bounds: {point}"
            );
        }
    }
}

#[test]
fn requested_alpha_cutouts_bake_real_uv_holes_and_keep_opaque_defaults() {
    let (directory, game) = alpha_game();
    let mut vmf = scene();
    let prop = vmf.entities.len() - 1;
    let opaque = compile(&vmf, &game);
    assert_eq!(opaque.stats.prop_shadow_alpha_triangles, 0);
    assert_eq!(floor_sample(&opaque.bsp.unwrap(), DVec3::ZERO), DVec3::ZERO);
    vmf.entities[prop].properties.set("skin", "1");
    let before = vmf.clone();
    let cutout = compile(&vmf, &game);
    assert_eq!(cutout.stats.prop_shadow_alpha_triangles, 1);
    assert_eq!(cutout.stats.prop_shadow_alpha_materials, 1);
    assert_eq!(cutout.stats.prop_shadow_alpha_textures, 1);
    assert_eq!(cutout.stats.prop_shadow_alpha_bytes, 16);
    let cutout = cutout.bsp.unwrap();
    assert_eq!(floor_sample(&cutout, DVec3::ZERO).x, 200.0);
    assert_eq!(
        floor_sample(&cutout, DVec3::new(-16.0, 0.0, 0.0)),
        DVec3::ZERO
    );
    assert_eq!(vmf, before);
    let mut mdl = std::fs::read(directory.path().join("models/props/test.mdl")).unwrap();
    put_int(&mut mdl, 152, 16);
    std::fs::write(directory.path().join("models/props/test.mdl"), mdl).unwrap();
    let unflagged = compile(&vmf, &game);
    assert_eq!(unflagged.stats.prop_shadow_alpha_triangles, 0);
    let opaque_bytes = unflagged.bsp.unwrap().to_bytes();
    std::fs::write(
        directory.path().join("materials/models/test/two.vmt"),
        "VertexLitGeneric { $basetexture test }",
    )
    .unwrap();
    assert_eq!(compile(&vmf, &game).bsp.unwrap().to_bytes(), opaque_bytes);
}

#[test]
fn requested_alpha_assets_and_effects_fail_on_the_prop_and_preserve_the_build() {
    let (directory, game) = alpha_game();
    let mut vmf = scene();
    vmf.entities.last_mut().unwrap().properties.set("skin", "1");
    let before = vmf.clone();
    let destination = directory.path().join("existing.bsp");
    compile(&vmf, &game).write(&destination).unwrap();
    let old = std::fs::read(&destination).unwrap();
    for (material, expected) in [
        (
            "VertexLitGeneric { $alphatest 1 $basetexture absent }",
            "was not found",
        ),
        (
            "VertexLitGeneric { $alphatest 1 $basetexture test $basetexturetransform \"rotate 30\" }",
            "rotated",
        ),
        (
            "VertexLitGeneric { $translucent 1 $basetexture test }",
            "$translucent",
        ),
        (
            "Refract { $alphatest 1 $basetexture test }",
            "unsupported texture-shadow shader",
        ),
        (
            "VertexLitGeneric { $alphatest 1 $basetexture test Proxies { AnimatedTexture { } } }",
            "animation and conditional",
        ),
        (
            "VertexLitGeneric { $alphatest 1 $basetexture test $alphatestreference nan }",
            "nonfinite",
        ),
        (
            "VertexLitGeneric { $alphatest 1 $basetexture test $detail detail }",
            "$detail",
        ),
        (
            "VertexLitGeneric { $alphatest 1 $basetexture test $basealphaenvmapmask 1 }",
            "$basealphaenvmapmask",
        ),
    ] {
        std::fs::write(
            directory.path().join("materials/models/test/two.vmt"),
            material,
        )
        .unwrap();
        let failed = compiler::compile_with_options(&vmf, &game, options());
        assert!(failed.bsp.is_none());
        assert!(
            failed.report.diagnostics.iter().any(|diagnostic| {
                diagnostic.object.as_deref() == Some("entity:9002")
                    && diagnostic.message.contains(expected)
            }),
            "{:?}",
            failed.report
        );
        assert!(!failed.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), old);
        assert_eq!(vmf, before);
    }
    std::fs::write(
        directory.path().join("materials/models/test/two.vmt"),
        "VertexLitGeneric { $alphatest 1 $basetexture test }",
    )
    .unwrap();
    std::fs::write(directory.path().join("materials/test.vtf"), b"VTF\0").unwrap();
    let failed = compiler::compile_with_options(&vmf, &game, options());
    assert!(failed.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.object.as_deref() == Some("entity:9002")
            && diagnostic.message.contains("truncated")
    }));
    assert!(!failed.write(&destination).unwrap());
    assert_eq!(std::fs::read(&destination).unwrap(), old);
}

#[test]
fn alpha_cutouts_share_spot_and_sun_rays_and_shader_reference_defaults() {
    let (directory, game) = alpha_game();
    let mut vmf = scene();
    let prop = vmf.entities.len() - 1;
    let light = prop - 1;
    let opaque_sample = DVec3::new(-16.0, -16.0, 0.0);
    vmf.entities[prop].properties.set("skin", "1");
    vmf.entities[light]
        .properties
        .set("classname", "light_spot");
    for (key, value) in [
        ("pitch", "-90"),
        ("_inner_cone", "30"),
        ("_cone", "45"),
        ("_exponent", "1"),
    ] {
        vmf.entities[light].properties.set(key, value);
    }
    let spot = compile(&vmf, &game).bsp.unwrap();
    assert_eq!(floor_sample(&spot, DVec3::ZERO).x, 200.0);
    assert_eq!(floor_sample(&spot, opaque_sample), DVec3::ZERO);
    vmf.entities[light]
        .properties
        .set("classname", "light_environment");
    vmf.entities[light].properties.set("_ambient", "0");
    for solid in &mut vmf.world.solids {
        if solid
            .sides
            .iter()
            .all(|side| side.plane.0.iter().all(|point| point.z >= 256.0))
        {
            for side in &mut solid.sides {
                side.material = "sky".into();
            }
        }
    }
    let sun = compile(&vmf, &game).bsp.unwrap();
    assert_eq!(floor_sample(&sun, DVec3::ZERO).x, 200.0);
    assert_eq!(floor_sample(&sun, opaque_sample), DVec3::ZERO);
    assert_eq!(floor_sample(&sun, DVec3::new(-16.0, 0.0, 0.0)).x, 200.0);
    let mut texture = alpha_texture();
    for pixel in texture[80..].as_chunks_mut::<4>().0 {
        pixel[3] = 128;
    }
    std::fs::write(directory.path().join("materials/test.vtf"), texture).unwrap();
    for (material, lit) in [
        ("VertexLitGeneric { $alphatest 1 $basetexture test }", false),
        ("UnlitGeneric { $alphatest 1 $basetexture test }", true),
        (
            "UnlitGeneric { $alphatest 1 $basetexture test $alphatestreference 0 }",
            false,
        ),
        (
            "VertexLitGeneric { $alphatest 1 $basetexture test $alphatestreference 0.75 }",
            true,
        ),
    ] {
        std::fs::write(
            directory.path().join("materials/models/test/two.vmt"),
            material,
        )
        .unwrap();
        let color = floor_sample(&compile(&vmf, &game).bsp.unwrap(), DVec3::ZERO);
        assert_eq!(color.x, if lit { 200.0 } else { 0.0 }, "{material}");
    }
}
