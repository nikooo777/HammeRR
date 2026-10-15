mod common;

use compiler::{LightingMode, Options};
use compiler::lighting::LuxelFrame;
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
    for (index, point) in [[-24.0, -24.0, 0.0], [24.0, -24.0, 0.0], [0.0, 24.0, 0.0], [0.0, 0.0, 0.0]].into_iter().enumerate() {
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
        std::fs::write(directory.path().join(format!("models/props/test.{suffix}")), bytes).unwrap();
    }
    for (name, text) in [
        ("models/test/one", "VertexLitGeneric { $basetexture test }"),
        ("models/test/two", "VertexLitGeneric { $alphatest 1 $basetexture test }"),
        ("sky", "UnlitGeneric { %compilesky 1 }"),
    ] {
        std::fs::write(directory.path().join(format!("materials/{name}.vmt")), text).unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath { ids: vec!["game".into()], mount: Mount::Directory(directory.path().to_owned()) });
    (directory, game)
}

fn scene() -> Vmf {
    let mut vmf = common::room();
    let mut light = Entity::new(9001, "light");
    for (key, value) in [("origin", "0 0 100"), ("_light", "255 0 0 200"), ("_quadratic_attn", "1")] {
        light.properties.set(key, value);
    }
    vmf.entities.push(light);
    let mut prop = Entity::new(9002, "prop_static");
    for (key, value) in [("origin", "0 0 50"), ("model", "models/props/test.mdl"), ("solid", "0")] {
        prop.properties.set(key, value);
    }
    vmf.entities.push(prop);
    vmf
}

fn options() -> Options {
    Options { lighting: LightingMode::DirectLdr, ..Options::default() }
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
    vmf.entities.last_mut().unwrap().properties.set("disableshadows", "1");
    let clear = compile(&vmf, &game);
    assert_eq!(clear.stats.prop_shadow_triangles, 0);
    assert_eq!(clear.stats.prop_shadow_disabled, 1);
    let clear = clear.bsp.unwrap();
    assert_eq!(floor_sample(&clear, DVec3::ZERO).x, 200.0);
    assert_eq!(floor_sample(&clear, DVec3::new(48.0, 48.0, 0.0)), hole);
    assert_eq!(clear.static_props().unwrap().unwrap().props[0].flags & 16, 16);
}

#[test]
fn transformed_prop_spot_sun_and_skin_materials_use_shared_shadows() {
    let (_directory, game) = game();
    let mut vmf = scene();
    let prop_index = vmf.entities.len() - 1;
    let light_index = prop_index - 1;
    vmf.entities[prop_index].properties.set("angles", "90 0 0");
    assert_eq!(floor_sample(&compile(&vmf, &game).bsp.unwrap(), DVec3::ZERO).x, 200.0);
    vmf.entities[prop_index].properties.set("angles", "0 90 0");
    vmf.entities[prop_index].properties.set("origin", "32 0 50");
    vmf.entities[light_index].properties.set("origin", "32 0 100");
    assert_eq!(floor_sample(&compile(&vmf, &game).bsp.unwrap(), DVec3::X * 32.0), DVec3::ZERO);
    vmf.entities[prop_index].properties.set("origin", "0 0 50");
    vmf.entities[light_index].properties.set("origin", "0 0 100");
    vmf.entities[light_index].properties.set("classname", "light_spot");
    for (key, value) in [("pitch", "-90"), ("_inner_cone", "30"), ("_cone", "45"), ("_exponent", "1")] {
        vmf.entities[light_index].properties.set(key, value);
    }
    assert_eq!(floor_sample(&compile(&vmf, &game).bsp.unwrap(), DVec3::ZERO), DVec3::ZERO);
    vmf.entities[prop_index].properties.set("skin", "1");
    let alpha = compile(&vmf, &game);
    assert!(alpha.report.diagnostics.iter().any(|diagnostic| diagnostic.object.as_deref() == Some("entity:9002") && diagnostic.message.contains("texture alpha")));
    assert_eq!(floor_sample(&alpha.bsp.unwrap(), DVec3::ZERO), DVec3::ZERO);
    vmf.entities[light_index].properties.set("classname", "light_environment");
    vmf.entities[light_index].properties.set("_ambient", "0");
    for solid in &mut vmf.world.solids {
        if solid.sides.iter().all(|side| side.plane.0.iter().all(|point| point.z >= 256.0)) {
            for side in &mut solid.sides { side.material = "sky".into(); }
        }
    }
    assert_eq!(floor_sample(&compile(&vmf, &game).bsp.unwrap(), DVec3::ZERO), DVec3::ZERO);
    vmf.entities[prop_index].properties.set("disableshadows", "1");
    assert_eq!(floor_sample(&compile(&vmf, &game).bsp.unwrap(), DVec3::ZERO).x, 200.0);
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
    assert!(failed.report.diagnostics.iter().any(|diagnostic| diagnostic.object.as_deref() == Some("entity:9002") && diagnostic.message.contains("checksum")));
    assert!(!failed.write(&destination).unwrap());
    assert_eq!(std::fs::read(&destination).unwrap(), old);
    assert_eq!(vmf, before);
    assert!(compiler::compile(&vmf, &game).bsp.is_some());
    vmf.entities.last_mut().unwrap().properties.set("disableshadows", "1");
    assert!(compile(&vmf, &game).bsp.is_some());
    vmf.entities.last_mut().unwrap().hidden = true;
    let hidden = compile(&vmf, &game);
    assert_eq!(hidden.stats.prop_shadow_props, 0);
    assert_eq!(hidden.stats.prop_shadow_disabled, 0);
}
