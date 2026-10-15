use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use source_formats::gamefs::{GameFs, Mount, MountedPath};

use super::*;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    fs: GameFs,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("hammerr-sky-{}-{}", std::process::id(), NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(root.join("materials/skybox")).unwrap();
        let fs = GameFs {
            game_dir: root.clone(), base_dir: root.clone(),
            mounts: vec![MountedPath { ids: vec!["game".into()], mount: Mount::Directory(root.clone()) }],
        };
        Self { root, fs }
    }

    fn write(&self, path: &str, data: impl AsRef<[u8]>) {
        let path = self.root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }

    fn faces(&self) {
        for (index, suffix) in SUFFIXES.into_iter().enumerate() {
            self.write(&format!("materials/skybox/test{suffix}.vmt"), format!("UnlitGeneric {{ $basetexture skybox/test{suffix} %tooltexture skybox/missing }}"));
            let mut data = vec![0u8; 80];
            data[..4].copy_from_slice(b"VTF\0");
            data[4..8].copy_from_slice(&7u32.to_le_bytes());
            data[8..12].copy_from_slice(&2u32.to_le_bytes());
            data[12..16].copy_from_slice(&80u32.to_le_bytes());
            data[16..18].copy_from_slice(&(index as u16 + 1).to_le_bytes());
            data[18..20].copy_from_slice(&1u16.to_le_bytes());
            data[24..26].copy_from_slice(&1u16.to_le_bytes());
            data[56] = 1;
            data[57..61].copy_from_slice(&(-1i32).to_le_bytes());
            data[63..65].copy_from_slice(&1u16.to_le_bytes());
            for _ in 0..=index { data.extend_from_slice(&[index as u8 * 30, 80, 160, 255]); }
            self.write(&format!("materials/skybox/test{suffix}.vtf"), data);
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn six_faces_resolve_patch_materials_and_distinct_sizes_without_editor_substitutes() {
    let fixture = Fixture::new();
    fixture.faces();
    fixture.write("materials/skybox/testrt.vmt", "patch { include skybox/testbk insert { $basetexture skybox/missing } replace { $basetexture skybox/testrt } }");
    let skybox = Skybox::load(&fixture.fs, " TEST ");
    assert!(skybox.ready(), "{}", skybox.summary());
    for (index, face) in skybox.faces.iter().enumerate() {
        let texture = face.texture.as_ref().unwrap();
        assert_eq!(texture.width, index as u32 + 1);
        assert_eq!(texture.decode_rgba8(0, 0, 0).unwrap().pixels[..4], [index as u8 * 30, 80, 160, 255]);
    }
    let mut library = crate::MaterialLibrary::default();
    let first = library.skybox(&fixture.fs, "TEST");
    let second = library.skybox(&fixture.fs, "test");
    assert!(std::sync::Arc::ptr_eq(&first, &second));
}

#[test]
fn partial_and_unsupported_skies_report_each_failed_face() {
    let fixture = Fixture::new();
    fixture.faces();
    fixture.write("materials/skybox/testup.vmt", "Sky { $hdrbasetexture skybox/testup }");
    fixture.write("materials/skybox/testdn.vmt", "UnlitGeneric { $basetexture skybox/missing }");
    fixture.write("materials/skybox/testlf.vmt", "UnlitGeneric { $basetexture skybox/testlf Proxies { TextureScroll {} } }");
    let skybox = Skybox::load(&fixture.fs, "test");
    assert!(!skybox.ready());
    assert!(skybox.faces[0].texture.is_some());
    assert_eq!(skybox.faces[4].issue.as_deref(), Some("no LDR $basetexture"));
    assert_eq!(skybox.faces[5].issue.as_deref(), Some("missing LDR texture"));
    assert!(skybox.faces[2].issue.as_ref().unwrap().contains("animation"));
    assert!(skybox.summary().contains("up: no LDR"));
    assert!(!Skybox::load(&fixture.fs, "../../outside").ready());
}

#[test]
fn stock_vertical_scale_and_centered_scale_translate_are_supported() {
    assert_eq!(texture_transform(None).unwrap(), [1.0, 1.0, 0.0, 0.0]);
    assert_eq!(texture_transform(Some("center 0 0 scale 1 2 rotate 0 translate 0 0")).unwrap(), [1.0, 2.0, 0.0, 0.0]);
    assert_eq!(texture_transform(Some("scale 2 3 translate 0.25 -0.5")).unwrap(), [2.0, 3.0, -0.25, -1.5]);
    for value in ["rotate 90", "scale 1", "scale NaN 1", "translate inf 0", "scale 2000 1", "unknown 0"] {
        assert!(texture_transform(Some(value)).is_err(), "{value}");
    }
}

#[test]
fn scene_retains_sky_geometry_and_resolves_custom_compile_sky_materials() {
    let fixture = Fixture::new();
    fixture.faces();
    fixture.write("materials/custom/sky.vmt", "LightmappedGeneric { %compile2dsky 1 }");
    let mut document = map_core::Document::empty();
    document.edit("sky room", |tx| {
        tx.world_properties_mut().set("skyname", "test");
        map_core::edit::create_block(tx, glam::DVec3::ZERO, glam::DVec3::splat(64.0), "custom/sky")?;
        Ok(())
    }).unwrap();
    let before = document.vmf().clone();
    let mut library = crate::MaterialLibrary::default();
    let mesh = crate::SceneMesh::build(document.vmf(), &fixture.fs, &mut library, &Default::default());
    assert!(mesh.skybox.as_ref().unwrap().ready());
    assert_eq!(mesh.indices.len(), 36);
    assert!(library.get(mesh.batches[0].material).sky);
    assert_eq!(&before, document.vmf());
}

#[test]
#[ignore = "requires an offscreen GPU; run explicitly with --ignored"]
fn rendered_six_face_directions_translation_depth_and_fallback() {
    use glam::{DVec3, Vec3};
    use crate::{Camera, GpuContext, Renderer, SceneMesh, ViewOptions};

    let fixture = Fixture::new();
    fixture.faces();
    fixture.write("materials/tools/toolsskybox.vmt", "LightmappedGeneric { %compilesky 1 }");
    let mut document = map_core::Document::empty();
    document.edit("sky room", |tx| {
        tx.world_properties_mut().set("skyname", "test");
        for axis in 0..3 {
            for sign in [-1.0, 1.0] {
                let mut mins = DVec3::splat(-272.0);
                let mut maxs = DVec3::splat(272.0);
                mins[axis] = if sign < 0.0 { -272.0 } else { 256.0 };
                maxs[axis] = if sign < 0.0 { -256.0 } else { 272.0 };
                map_core::edit::create_block(tx, mins, maxs, "tools/toolsskybox")?;
            }
        }
        Ok(())
    }).unwrap();
    let ctx = GpuContext::new().unwrap();
    let mut renderer = Renderer::new(&ctx);
    let mut library = crate::MaterialLibrary::default();
    let mesh = SceneMesh::build(document.vmf(), &fixture.fs, &mut library, &Default::default());
    let scene = renderer.upload(&ctx, &mesh, &library);
    let options = ViewOptions { sky_preview: true, grid_visible: false, ..Default::default() };
    for (index, direction) in [Vec3::X, Vec3::Y, Vec3::NEG_X, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z].into_iter().enumerate() {
        let camera = Camera::looking_at(Vec3::ZERO, direction);
        let pixels = renderer.render_to_rgba(&ctx, &scene, &camera, &options, 65, 65);
        let center = (32 * 65 + 32) * 4;
        let expected = [index as u8 * 30, 80, 160, 255];
        for (actual, expected) in pixels[center..center + 4].iter().zip(expected) {
            assert!(actual.abs_diff(expected) <= 1, "{}: {:?}", SUFFIXES[index], &pixels[center..center + 4]);
        }
        let moved = Camera { position: Vec3::new(20.0, -15.0, 10.0), ..camera };
        assert_eq!(pixels, renderer.render_to_rgba(&ctx, &scene, &moved, &options, 65, 65));
    }
    let camera = Camera::looking_at(Vec3::ZERO, Vec3::X);
    let baseline = renderer.render_to_rgba(&ctx, &scene, &camera, &options, 65, 65);
    document.edit("beyond sky", |tx| {
        map_core::edit::create_block(tx, DVec3::new(300.0, -100.0, -100.0), DVec3::new(340.0, 100.0, 100.0), "brick/missing")?;
        Ok(())
    }).unwrap();
    let mesh = SceneMesh::build(document.vmf(), &fixture.fs, &mut library, &Default::default());
    let scene = renderer.upload(&ctx, &mesh, &library);
    assert_eq!(baseline, renderer.render_to_rgba(&ctx, &scene, &camera, &options, 65, 65));
    document.edit("foreground", |tx| {
        map_core::edit::create_block(tx, DVec3::new(64.0, -32.0, -32.0), DVec3::new(96.0, 32.0, 32.0), "brick/missing")?;
        Ok(())
    }).unwrap();
    let mesh = SceneMesh::build(document.vmf(), &fixture.fs, &mut library, &Default::default());
    let scene = renderer.upload(&ctx, &mesh, &library);
    assert_ne!(baseline, renderer.render_to_rgba(&ctx, &scene, &camera, &options, 65, 65));
    let disabled = ViewOptions { sky_preview: false, ..options };
    let mut unavailable = mesh;
    unavailable.skybox = Some(std::sync::Arc::new(Skybox::load(&fixture.fs, "missing")));
    let scene = renderer.upload(&ctx, &unavailable, &library);
    assert_eq!(renderer.render_to_rgba(&ctx, &scene, &camera, &disabled, 65, 65), renderer.render_to_rgba(&ctx, &scene, &camera, &options, 65, 65));
    for surface in [crate::SurfaceView::Flat, crate::SurfaceView::Wireframe, crate::SurfaceView::LightmapDensity, crate::SurfaceView::SmoothingGroups] {
        let options = ViewOptions { surface, ..options };
        assert_eq!(renderer.render_to_rgba(&ctx, &scene, &camera, &ViewOptions { sky_preview: false, ..options }, 65, 65), renderer.render_to_rgba(&ctx, &scene, &camera, &options, 65, 65));
    }
}
