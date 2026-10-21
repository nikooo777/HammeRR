use super::*;
use crate::model::tests::put_int;

const MODEL: &str = "models/props/portable.mdl";

fn put_float(data: &mut [u8], offset: usize, value: f32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_string(data: &mut [u8], offset: usize, value: &str) {
    data[offset..offset + value.len()].copy_from_slice(value.as_bytes());
    data[offset + value.len()] = 0;
}

fn fixture() -> [Vec<u8>; 3] {
    let [mut mdl, mut vvd, mut vtx] = crate::model::tests::fixture();
    let textures = mdl[256..384].to_vec();
    let body = mdl[400..416].to_vec();
    mdl.resize(1280, 0);
    mdl[248..408].fill(0);
    mdl[1024..1152].copy_from_slice(&textures);
    mdl[1160..1176].copy_from_slice(&body);
    for (offset, value) in [
        (76, 1280),
        (156, 1),
        (160, 768),
        (208, 1024),
        (216, 1152),
        (228, 1156),
        (236, 1160),
        (308, 1200),
        (628, 4),
        (768, 1210 - 768),
        (772, -1),
        (944, 1200 - 768),
        (1024, 720 - 1024),
        (1088, 736 - 1088),
        (1152, 752),
        (1172, 416 - 1160),
    ] {
        put_int(&mut mdl, offset, value);
    }
    mdl[1158..1160].copy_from_slice(&1_u16.to_le_bytes());
    for field in (776..800).step_by(4) {
        put_int(&mut mdl, field, -1);
    }
    for field in [824, 864, 884, 904] {
        put_float(&mut mdl, field, 1.0);
    }
    put_string(&mut mdl, 12, "props/portable.mdl");
    put_string(&mut mdl, 1200, "default");
    put_string(&mut mdl, 1210, "root");
    vvd.resize(320, 0);
    put_int(&mut vvd, 60, 256);
    for index in 0..4 {
        put_float(&mut vvd, 256 + index * 16, 1.0);
        put_float(&mut vvd, 268 + index * 16, 1.0);
    }
    vtx.resize(181, 0);
    put_int(&mut vtx, 24, 173);
    vtx[97] = 2;
    [mdl, vvd, vtx]
}

fn parse(files: &[Vec<u8>; 3]) -> io::Result<Model> {
    validate_portable_static(
        MODEL,
        &files[0],
        &files[1],
        &files[2],
        HardwareLimits::default(),
        &mut |_| Ok(()),
    )
}

fn metadata_fixture(text: &[u8]) -> [Vec<u8>; 3] {
    let mut files = fixture();
    let start = files[0].len();
    files[0].extend_from_slice(text);
    let length = files[0].len();
    put_int(&mut files[0], 76, length as i32);
    put_int(&mut files[0], 312, start as i32);
    put_int(&mut files[0], 316, text.len() as i32);
    files
}

fn animation_fixture() -> [Vec<u8>; 3] {
    let mut files = fixture();
    files[0].resize(1640, 0);
    for (offset, value) in [
        (76, 1640),
        (180, 1),
        (184, 1280),
        (188, 1),
        (192, 1384),
        (1280, -1280),
        (1284, 324),
        (1296, 1),
        (1336, 100),
        (1384, -1384),
        (1388, 220),
        (1392, 220),
        (1440, 1),
        (1444, 216),
        (1452, 1),
        (1456, 1),
    ] {
        put_int(&mut files[0], offset, value);
    }
    put_float(&mut files[0], 1288, 30.0);
    files[0][1380..1384].copy_from_slice(&EMPTY_ANIMATION);
    put_string(&mut files[0], 1604, "idle");
    files
}

fn two_lod_fixture() -> [Vec<u8>; 3] {
    let [mut mdl, mut vvd, mut vtx] = fixture();
    put_int(&mut mdl, 632, 3);
    put_int(&mut vvd, 12, 2);
    put_int(&mut vvd, 20, 3);
    vtx.resize(318, 0);
    for (offset, value) in [
        (20, 2),
        (24, 302),
        (44, 2),
        (48, 184 - 44),
        (184, 1),
        (188, 64 - 184),
        (196, 1),
        (200, 208 - 196),
        (208, 1),
        (212, 217 - 208),
        (217, 3),
        (221, 242 - 217),
        (225, 3),
        (229, 269 - 217),
        (233, 1),
        (237, 275 - 217),
        (275, 3),
        (283, 3),
    ] {
        put_int(&mut vtx, offset, value);
    }
    put_float(&mut vtx, 204, 100.0);
    vtx[241] = 2;
    vtx[293] = 1;
    for index in 0..3 {
        vtx[246 + index * 9..248 + index * 9].copy_from_slice(&(index as u16).to_le_bytes());
        vtx[269 + index * 2..271 + index * 2].copy_from_slice(&(index as u16).to_le_bytes());
    }
    [mdl, vvd, vtx]
}

#[test]
fn portable_models_preserve_every_declared_material_and_skin() {
    for version in [44, 48] {
        let mut files = fixture();
        put_int(&mut files[0], 4, version);
        let model = parse(&files).unwrap();
        assert_eq!(model.materials, ["one", "two"]);
        assert_eq!(model.material_directories, ["models/test"]);
        assert_eq!(model.skins, [vec![0], vec![1]]);
        assert_eq!(model.parts[0].indices, [0, 1, 2, 0, 2, 3]);
    }
    let files = two_lod_fixture();
    let model = parse(&files).unwrap();
    assert_eq!(model.materials, ["one", "two"]);
    assert_eq!(model.skins, [vec![0], vec![1]]);
    let (_, layout) = Model::parse_with_hardware(
        &files[0],
        &files[1],
        &files[2],
        HardwareLimits::default(),
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(layout.lods, 2);
    assert_eq!(layout.sections.len(), 2);
    let mut files = files;
    files[2][246..248].copy_from_slice(&3_u16.to_le_bytes());
    assert!(parse(&files).is_err());
}

#[test]
fn supported_original_metadata_and_empty_animation_name_offsets_are_retained() {
    let mut files =
        metadata_fixture(b"mdlkeyvalue\n{\nprop_data {\n\"base\" \"Metal.Medium\"  }\n}\n\0");
    put_int(&mut files[0], 348, 1220);
    assert!(parse(&files).is_ok());
    put_int(&mut files[0], 348, 1200);
    assert!(parse(&files).is_err());
    assert!(parse(&animation_fixture()).is_ok());
}

#[test]
fn external_or_opaque_model_declarations_reject_before_hardware_parsing() {
    for (offset, value) in [
        (164, 1),
        (240, 1),
        (260, 1),
        (268, 1),
        (276, 1),
        (284, 1),
        (292, 1),
        (300, 1),
        (320, 1),
        (336, 1),
        (336, -1),
        (352, 1),
        (384, 1),
        (400, 1200),
        (1164, 2),
    ] {
        let mut files = fixture();
        put_int(&mut files[0], offset, value);
        assert!(parse(&files).is_err(), "offset {offset}");
    }
    for text in [
        &b"mdlkeyvalue { prop_data { base Wood.Small } }\0"[..],
        &b"mdlkeyvalue { prop_data { base Metal.Medium health 4 } }\0"[..],
        &b"mdlkeyvalue { prop_data { base Metal.Medium } } extra {}\0"[..],
        &b"mdlkeyvalue { prop_data { base Metal.Medium } }\0\0"[..],
        &b"mdlkeyvalue { prop_data { base Metal.Medium } } [$WIN32]\0"[..],
    ] {
        assert!(parse(&metadata_fixture(text)).is_err());
    }
}

#[test]
fn canonical_raw_names_cannot_be_recovered_by_normalization() {
    for (offset, value) in [
        (12, "props/other.mdl"),
        (12, "props\\portable.mdl"),
        (720, "One"),
        (720, "one.vmt"),
        (720, "../one"),
        (752, "Models/test/"),
        (752, "models\\test\\"),
        (752, "models//test/"),
        (752, "models/test//"),
        (1200, "custom_surface"),
    ] {
        let mut files = fixture();
        put_string(&mut files[0], offset, value);
        assert!(parse(&files).is_err(), "{value}");
    }
    let files = fixture();
    assert!(
        validate_portable_static(
            "MODELS/props/portable.mdl",
            &files[0],
            &files[1],
            &files[2],
            HardwareLimits::default(),
            &mut |_| Ok(())
        )
        .is_err()
    );
}

#[test]
fn rest_animation_and_sequence_dependencies_cannot_hide_in_local_tables() {
    for (offset, value) in [
        (1292, 8),
        (1296, 2),
        (1300, 1),
        (1332, 1),
        (1348, 1),
        (1360, 100),
        (1364, 30),
        (1396, 8),
        (1408, 1),
        (1532, 1),
        (1560, 10),
        (1572, 1),
    ] {
        let mut files = animation_fixture();
        put_int(&mut files[0], offset, value);
        assert!(parse(&files).is_err(), "offset {offset}");
    }
}

#[test]
fn terminal_empty_rest_stream_uses_the_validated_reference_pose() {
    let files = animation_fixture();
    let model = parse(&files).unwrap();
    assert_eq!(model.vertices, parse(&fixture()).unwrap().vertices);
    for (offset, value) in [
        (1280, 0),
        (1336, -1),
        (1336, 96),
        (1336, i32::MAX),
        (1380, 0),
        (1380, 0x1ff),
        (1380, 0x100ff),
        (1380, -0xff01),
        (1384, 0),
    ] {
        let mut files = animation_fixture();
        put_int(&mut files[0], offset, value);
        assert!(parse(&files).is_err(), "offset {offset}, value {value}");
    }
    let mut files = animation_fixture();
    put_int(&mut files[0], 1336, 359);
    assert!(parse(&files).is_err());
    let mut files = animation_fixture();
    put_float(&mut files[0], 824, 0.0);
    assert!(parse(&files).is_err());
}

#[test]
fn bone_controls_and_surface_dependencies_cannot_hide_in_static_bones() {
    for (offset, value) in [
        (772, 0),
        (772, 1),
        (776, 0),
        (928, 4),
        (932, 1),
        (936, 100),
        (944, 1210 - 768),
    ] {
        let mut files = fixture();
        put_int(&mut files[0], offset, value);
        assert!(parse(&files).is_err(), "offset {offset}");
    }
    let mut files = fixture();
    put_float(&mut files[0], 824, 0.0);
    assert!(parse(&files).is_err());
}

#[test]
fn complete_finite_tangents_and_existing_all_lod_checks_are_required() {
    for (file, offset, value) in [(1, 60, 0), (1, 60, 300), (1, 8, 8), (2, 16, 8), (2, 173, 1)] {
        let mut files = fixture();
        put_int(&mut files[file], offset, value);
        assert!(parse(&files).is_err());
    }
    for (offset, value) in [
        (256, f32::NAN),
        (256, 0.0),
        (268, 0.0),
        (268, f32::INFINITY),
    ] {
        let mut files = fixture();
        put_float(&mut files[1], offset, value);
        assert!(parse(&files).is_err());
    }
    let mut files = fixture();
    files[0][377] = 1;
    assert!(parse(&files).is_err());
    files = fixture();
    files[1].truncate(319);
    assert!(parse(&files).is_err());
}

#[test]
fn admission_uses_the_hardware_budget_and_cancellation_before_allocation() {
    let files = metadata_fixture(b"mdlkeyvalue { prop_data { base Metal.Medium } }\0");
    let result = validate_portable_static(
        MODEL,
        &files[0],
        &files[1],
        &files[2],
        HardwareLimits {
            bytes: 1,
            ..HardwareLimits::default()
        },
        &mut |_| Ok(()),
    );
    assert!(result.is_err());
    let mut calls = 0;
    let result = validate_portable_static(
        MODEL,
        &files[0],
        &files[1],
        &files[2],
        HardwareLimits::default(),
        &mut |charge| {
            calls += 1;
            if charge.bytes > 0 {
                Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "cancel portable admission",
                ))
            } else {
                Ok(())
            }
        },
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
    assert!(calls > 1);
}

fn physics(text: &[u8]) -> Vec<u8> {
    let mut data = [16_i32, 0, 1, 7].map(i32::to_le_bytes).concat();
    data.extend_from_slice(&32_i32.to_le_bytes());
    data.extend_from_slice(b"VPHY");
    data.extend_from_slice(&0x100_u16.to_le_bytes());
    data.extend_from_slice(&0_u16.to_le_bytes());
    data.extend_from_slice(&4_u32.to_le_bytes());
    data.extend_from_slice(&[0; 20]);
    data.extend_from_slice(text);
    data
}

#[test]
fn portable_physics_accepts_only_plain_complete_scalar_metadata() {
    let data = physics(b"solid { index 0 name file_cabinet1_phy mass 50 surfaceprop metalvent damping 0 rotdamping 0 inertia 1 volume 34976.390625 } editparams { rootname \"\" totalmass 50 }\0");
    assert!(validate_portable_physics(&data, 7).is_ok());
    assert!(validate_portable_physics(&data, 8).is_err());
    for text in [
        &b"solid {}\0"[..],
        &b"solid { index 1 }\0"[..],
        &b"solid { index 0 index 0 }\0"[..],
        &b"solid { index 0 mass NaN }\0"[..],
        &b"solid { index 0 surfaceprop custom }\0"[..],
        &b"solid { index 0 break { model models/other.mdl } }\0"[..],
        &b"solid { index 0 } editparams { rootname external }\0"[..],
        &b"solid { index 0 } editparams {} editparams {}\0"[..],
        &b"solid { index 0 } break { model models/other.mdl }\0"[..],
    ] {
        assert!(validate_portable_physics(&physics(text), 7).is_err());
    }
}
