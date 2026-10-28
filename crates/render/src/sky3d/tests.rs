use glam::{DVec3, Vec3};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

struct Fixture {
    root: tempfile::TempDir,
    fs: GameFs,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let fs = GameFs {
            game_dir: root.path().to_path_buf(),
            base_dir: root.path().to_path_buf(),
            mounts: vec![MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Directory(root.path().to_path_buf()),
            }],
        };
        Self { root, fs }
    }

    fn write(&self, path: &str, data: impl AsRef<[u8]>) {
        let path = self.root.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }

    fn texture(&self, name: &str, color: [u8; 4]) {
        self.image(name, [2, 2], &[color; 4]);
    }

    fn image(&self, name: &str, [width, height]: [u16; 2], pixels: &[[u8; 4]]) {
        assert_eq!(pixels.len(), usize::from(width) * usize::from(height));
        let mut bytes = vec![0u8; 80];
        bytes[..4].copy_from_slice(b"VTF\0");
        bytes[4..8].copy_from_slice(&7u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&80u32.to_le_bytes());
        bytes[16..18].copy_from_slice(&width.to_le_bytes());
        bytes[18..20].copy_from_slice(&height.to_le_bytes());
        bytes[24..26].copy_from_slice(&1u16.to_le_bytes());
        bytes[56] = 1;
        bytes[57..61].copy_from_slice(&(-1i32).to_le_bytes());
        bytes[63..65].copy_from_slice(&1u16.to_le_bytes());
        for color in pixels {
            bytes.extend_from_slice(color);
        }
        self.write(&texture_path(name), bytes);
    }

    fn material(&self, name: &str, color: [u8; 4]) {
        self.texture(name, color);
        self.write(
            &material_path(name),
            format!("UnlitGeneric {{ $basetexture {name} }}"),
        );
    }

    fn skies(&self) {
        self.texture("skybox/blue", [20, 50, 200, 255]);
        for suffix in ["rt", "bk", "lf", "ft", "up", "dn"] {
            self.write(
                &format!("materials/skybox/test{suffix}.vmt"),
                "UnlitGeneric { $basetexture skybox/blue }",
            );
        }
        for (name, flag) in [
            ("tools/toolsskybox", "%compilesky"),
            ("tools/toolsskybox2d", "%compile2dsky"),
        ] {
            self.write(
                &material_path(name),
                format!("LightmappedGeneric {{ {flag} 1 $basetexture skybox/blue }}"),
            );
        }
    }

    fn large_compressed_materials(&self) -> Vec<Surface> {
        self.texture("test/large", [0; 4]);
        let path = self.root.path().join(texture_path("test/large"));
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.truncate(80);
        bytes[16..18].copy_from_slice(&1024u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&1024u16.to_le_bytes());
        bytes[52..56].copy_from_slice(&13u32.to_le_bytes());
        bytes.resize(80 + 1024 * 1024 / 2, 0);
        std::fs::write(path, bytes).unwrap();
        (0..5)
            .map(|index| {
                let name = format!("test/large{index}");
                self.write(
                    &material_path(&name),
                    "UnlitGeneric { $basetexture test/large }",
                );
                surface(&name)
            })
            .collect()
    }
}

fn camera() -> Camera {
    Camera {
        origin: DVec3::new(1000.0, -600.0, 300.0),
        scale: 16.0,
    }
}

fn surface(material: &str) -> Surface {
    let origin = camera().origin;
    Surface {
        points: [
            DVec3::new(8.0, -4.0, -4.0),
            DVec3::new(8.0, -4.0, 4.0),
            DVec3::new(8.0, 4.0, 4.0),
            DVec3::new(8.0, 4.0, -4.0),
        ]
        .map(|point| point + origin)
        .to_vec(),
        normal: DVec3::NEG_X,
        material: material.into(),
        texture_u: [0.0, 1.0, 0.0, 0.0],
        texture_v: [0.0, 0.0, 1.0, 0.0],
    }
}

#[test]
fn camera_translation_preserves_direction_fov_and_main_equivalent_projection() {
    let miniature = camera();
    let main = crate::Camera {
        position: Vec3::new(64.0, -32.0, 48.0),
        yaw_degrees: 23.0,
        pitch_degrees: -7.0,
        fov_y_degrees: 83.0,
        ..crate::Camera::looking_at(Vec3::ZERO, Vec3::X)
    };
    let transformed = miniature.transform(&main).unwrap();
    assert_eq!(transformed.position, Vec3::new(1004.0, -602.0, 303.0));
    assert_eq!(transformed.forward(), main.forward());
    assert_eq!(transformed.right(), main.right());
    assert_eq!(transformed.up(), main.up());
    assert_eq!(transformed.fov_y_degrees, main.fov_y_degrees);
    let point = miniature.origin.as_vec3() + Vec3::new(20.0, 3.0, 2.0);
    let projected = miniature
        .view_projection(&main, 1.7)
        .unwrap()
        .project_point3(point);
    let expected = main
        .view_projection(1.7)
        .project_point3((point - miniature.origin.as_vec3()) * miniature.scale as f32);
    assert!(
        projected.abs_diff_eq(expected, 0.0001),
        "{projected:?} != {expected:?}"
    );
    let just_visible = Camera {
        origin: DVec3::ZERO,
        scale: 16.0,
    };
    let main = crate::Camera::looking_at(Vec3::ZERO, Vec3::X);
    let depth = just_visible
        .view_projection(&main, 1.0)
        .unwrap()
        .project_point3(Vec3::new(0.5, 0.0, 0.0))
        .z;
    assert!((0.0..1.0).contains(&depth));
}

#[test]
fn camera_rejects_unsupported_scale_and_invalid_view_inputs() {
    for scale in [0.0, -1.0, 1.5, 129.0, f64::INFINITY, f64::NAN] {
        assert!(Camera { scale, ..camera() }.validate().is_err());
    }
    assert!(
        Camera {
            origin: DVec3::splat(f64::MAX),
            ..camera()
        }
        .validate()
        .is_err()
    );
    let main = crate::Camera::looking_at(Vec3::ZERO, Vec3::X);
    assert!(
        camera()
            .transform(&crate::Camera {
                fov_y_degrees: 180.0,
                ..main
            })
            .is_err()
    );
    assert!(
        camera()
            .transform(&crate::Camera::planar(
                crate::Projection::Top,
                Vec3::ZERO,
                100.0
            ))
            .is_err()
    );
    assert!(camera().view_projection(&main, f32::NAN).is_err());
}

#[test]
fn mesh_has_only_clipped_surfaces_actual_dimensions_and_clockwise_triangles() {
    let fixture = Fixture::new();
    fixture.material("test/green", [30, 220, 70, 255]);
    let polygon = surface("test/green");
    let scene = Scene::build(camera(), std::slice::from_ref(&polygon), &fixture.fs).unwrap();
    assert_eq!(scene.vertices.len(), 4);
    assert_eq!(scene.indices.len(), 6);
    assert_eq!(scene.batches.len(), 1);
    assert_eq!(scene.materials.len(), 1);
    assert!(scene.materials[0].unlit);
    assert_eq!(
        scene.vertices[0].uv,
        [
            (polygon.points[0].y / 2.0) as f32,
            (polygon.points[0].z / 2.0) as f32
        ]
    );
    assert!(scene.vertices.iter().all(|vertex| vertex.color == [255; 4]
        && vertex.opacity == 1.0
        && vertex.lighting_receiver == 0));
    for triangle in scene.indices.as_chunks::<3>().0 {
        let [a, b, c] =
            triangle.map(|index| Vec3::from_array(scene.vertices[index as usize].position));
        assert!((b - a).cross(c - a).dot(Vec3::NEG_X) < 0.0);
    }
    assert!(scene.stats.cpu_bytes <= MAX_CPU_BYTES);
    assert!(scene.stats.gpu_bytes <= MAX_GPU_ASSET_BYTES);
    assert!(scene.stats.upload_bytes <= MAX_GPU_UPLOAD_BYTES);
}

#[test]
fn base_texture_transform_composes_with_literal_texel_axes_and_nonsquare_dimensions() {
    let fixture = Fixture::new();
    fixture.image("test/mapped", [4, 2], &[[30, 220, 70, 255]; 8]);
    let surface = Surface {
        points: vec![
            DVec3::new(2.0, 6.0, 0.0),
            DVec3::new(6.0, 6.0, 0.0),
            DVec3::new(6.0, 10.0, 0.0),
            DVec3::new(2.0, 10.0, 0.0),
        ],
        normal: DVec3::Z,
        material: "test/mapped".into(),
        texture_u: [0.5, 0.25, 0.0, 1.0],
        texture_v: [-0.25, 0.5, 0.0, -2.0],
    };
    let identity = [[0.875, 0.25], [1.375, -0.25], [1.625, 0.75], [1.125, 1.25]];
    let mapped = [[1.625, 1.5], [2.625, 2.0], [3.125, 1.0], [2.125, 0.5]];
    for (transform, expected) in [
        (None, identity),
        (Some(""), identity),
        (
            Some("center 0.5 0.5 scale 1 1 rotate 0 translate 0 0"),
            identity,
        ),
        (
            Some("center 0.25 0.75 scale 2 -1 translate 0.125 0.25"),
            mapped,
        ),
        (
            Some("scale 8 8 center 0.25 0.75 scale 2 -1 translate 0.125 0.25"),
            mapped,
        ),
    ] {
        let declaration = transform
            .map(|value| format!(" $basetexturetransform \"{value}\""))
            .unwrap_or_default();
        fixture.write(
            "materials/test/mapped.vmt",
            format!("UnlitGeneric {{ $basetexture test/mapped{declaration} }}"),
        );
        let scene = Scene::build(camera(), std::slice::from_ref(&surface), &fixture.fs).unwrap();
        let actual = scene
            .vertices
            .iter()
            .map(|vertex| vertex.uv)
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "{transform:?}");
    }
}

#[test]
fn invalid_base_texture_transforms_fail_without_substitution() {
    let fixture = Fixture::new();
    fixture.texture("test/mapped", [30, 220, 70, 255]);
    for transform in [
        "rotate 90",
        "scale NaN 1",
        "translate inf 0",
        "scale 1025 1",
        "center 0.5",
        "unknown 1",
        "scale [1 1]",
    ] {
        fixture.write(
            "materials/test/mapped.vmt",
            format!(
                "UnlitGeneric {{ $basetexture test/mapped $basetexturetransform \"{transform}\" }}"
            ),
        );
        let error = Scene::build(camera(), &[surface("test/mapped")], &fixture.fs).unwrap_err();
        assert!(error.contains("texture transform"), "{transform}: {error}");
    }
    fixture.write(
        "materials/test/mapped.vmt",
        "UnlitGeneric { $basetexture test/mapped $basetexturetransform \"scale 1024 1\" }",
    );
    let mut overflowing = surface("test/mapped");
    overflowing.texture_u = [0.0, 0.0, 0.0, f64::from(f32::MAX)];
    let error = Scene::build(camera(), &[overflowing], &fixture.fs).unwrap_err();
    assert!(
        error.contains("texture coordinates exceed finite range"),
        "{error}"
    );
}

#[test]
fn missing_malformed_and_unsupported_requested_assets_fail_instead_of_checkerboards() {
    let fixture = Fixture::new();
    fixture.material("test/green", [30, 220, 70, 255]);
    for material in [
        "UnlitGeneric { $basetexture test/missing }",
        "UnlitGeneric { $basetexture test/green $translucent 1 }",
        "UnlitGeneric { $basetexture test/green $alphatest 1 }",
        "LightmappedGeneric { $basetexture test/green $bumpmap test/green }",
        "UnlitGeneric { $basetexture test/green Proxies { TextureScroll {} } }",
        "Water { $basetexture test/green }",
    ] {
        fixture.write("materials/test/unsupported.vmt", material);
        assert!(
            Scene::build(camera(), &[surface("test/unsupported")], &fixture.fs).is_err(),
            "{material}"
        );
    }
    assert!(Scene::build(camera(), &[surface("missing/vmt")], &fixture.fs).is_err());
    fixture.write("materials/test/green.vtf", b"malformed VTF");
    assert!(Scene::build(camera(), &[surface("test/green")], &fixture.fs).is_err());
}

#[test]
fn generated_detail_type_rejects_without_rejecting_absent_or_empty_optional_input() {
    let fixture = Fixture::new();
    fixture.texture("test/green", [30, 220, 70, 255]);
    for detail in [None, Some(""), Some(" "), Some("grass")] {
        let declaration = detail
            .map(|value| format!(" %detailtype \"{value}\""))
            .unwrap_or_default();
        fixture.write(
            "materials/test/generated.vmt",
            format!("UnlitGeneric {{ $basetexture test/green{declaration} }}"),
        );
        let result = Scene::build(camera(), &[surface("test/generated")], &fixture.fs);
        if detail.is_some_and(|value| !value.trim().is_empty()) {
            assert!(
                result
                    .unwrap_err()
                    .contains("unsupported generated detail geometry")
            );
        } else {
            assert!(result.is_ok(), "{detail:?}: {result:?}");
        }
    }
}

#[test]
fn geometry_and_aggregate_allocations_reject_before_material_reads() {
    let fixture = Fixture::new();
    let mut oversized = surface("missing/material");
    oversized.points = vec![DVec3::ZERO; MAX_VERTICES + 1];
    let error = Scene::build(camera(), &[oversized], &fixture.fs).unwrap_err();
    assert!(error.contains("vertices"), "{error}");
    let mut budget = Budget::default();
    budget.cpu(MAX_CPU_BYTES).unwrap();
    let error = budget
        .read(&fixture.fs, "materials/missing.vtf", false)
        .unwrap_err();
    assert!(error.contains("budget"));
    let mut budget = Budget::default();
    budget.gpu(MAX_GPU_ASSET_BYTES).unwrap();
    assert!(budget.gpu(1).is_err());
    assert!(checked_sum(usize::MAX, 1, MAX_CPU_BYTES, "CPU").is_err());
}

#[test]
fn material_count_and_source_read_limits_are_explicit_failures() {
    let fixture = Fixture::new();
    fixture.material("test/green", [30, 220, 70, 255]);
    let mut surfaces = Vec::new();
    for index in 0..=MAX_MATERIALS {
        let name = format!("test/material{index}");
        fixture.write(
            &material_path(&name),
            "UnlitGeneric { $basetexture test/green }",
        );
        surfaces.push(surface(&name));
    }
    let error = Scene::build(camera(), &surfaces, &fixture.fs).unwrap_err();
    assert!(error.contains("materials exceed budget"), "{error}");
    fixture.write(
        "materials/test/oversized.vmt",
        vec![b' '; MAX_VMT_BYTES + 1],
    );
    let error = Scene::build(camera(), &[surface("test/oversized")], &fixture.fs).unwrap_err();
    assert!(error.contains("byte read limit"), "{error}");
}

#[test]
fn invalid_surface_inputs_and_overflowing_uvs_never_return_a_scene() {
    let fixture = Fixture::new();
    fixture.material("test/green", [30, 220, 70, 255]);
    let mut invalid = surface("test/green");
    invalid.normal = DVec3::ZERO;
    assert!(Scene::build(camera(), &[invalid], &fixture.fs).is_err());
    let mut invalid = surface("test/green");
    invalid.points[0].z = f64::NAN;
    assert!(Scene::build(camera(), &[invalid], &fixture.fs).is_err());
    let mut invalid = surface("test/green");
    invalid.texture_u[0] = f64::MAX;
    let error = Scene::build(camera(), &[invalid], &fixture.fs).unwrap_err();
    assert!(error.contains("texture coordinates"), "{error}");
}

#[test]
fn bounded_targets_preserve_aspect_and_check_aggregate_before_allocation() {
    assert_eq!(
        target_size((8192, 4096), MAX_GPU_ASSET_BYTES).unwrap(),
        (2048, 1024)
    );
    assert_eq!(
        target_size((4096, 8192), MAX_GPU_ASSET_BYTES).unwrap(),
        (1024, 2048)
    );
    assert_eq!(target_size((65, 65), 0).unwrap(), (65, 65));
    assert!(target_size((0, 100), 0).is_err());
    assert!(target_size((100, 100), MAX_GPU_ASSET_BYTES + 1).is_err());
    let size = target_size((u32::MAX, u32::MAX), MAX_GPU_ASSET_BYTES).unwrap();
    assert!(size.0 as usize * size.1 as usize * TARGET_PIXEL_BYTES <= MAX_TARGET_BYTES);
}

#[test]
fn upload_rows_cross_256_byte_boundaries_and_reject_aggregate_before_allocation() {
    for (width, height, expected) in [
        (1, 1, 256),
        (63, 7, 1792),
        (64, 7, 1792),
        (65, 7, 3584),
        (128, 3, 1536),
        (129, 3, 2304),
    ] {
        assert_eq!(
            rgba_upload_bytes(width, height).unwrap(),
            expected,
            "{width}x{height}"
        );
    }
    let rows = (MAX_GPU_UPLOAD_BYTES / 256) as u32;
    assert_eq!(rgba_upload_bytes(1, rows).unwrap(), MAX_GPU_UPLOAD_BYTES);
    assert!(rgba_upload_bytes(1, rows + 1).is_err());
    assert!(rgba_upload_bytes(u32::MAX, u32::MAX).is_err());
    assert!(rgba_upload_bytes(0, 1).is_err());
    let mut budget = Budget::default();
    budget.upload(MAX_GPU_UPLOAD_BYTES - 256).unwrap();
    budget.upload(rgba_upload_bytes(1, 1).unwrap()).unwrap();
    assert!(budget.upload(1).is_err());
    assert_eq!(MAX_GPU_BYTES, 128 * 1024 * 1024);
}

#[test]
fn cancelled_material_preparation_never_returns_a_ready_scene() {
    let fixture = Fixture::new();
    let error = Scene::build_controlled(
        camera(),
        &[surface("missing/material")],
        &fixture.fs,
        || Err("cancelled sky preparation".into()),
    )
    .unwrap_err();
    assert_eq!(error, "cancelled sky preparation");
}

fn pixel(pixels: &[u8], x: usize, y: usize) -> [u8; 4] {
    pixels[(y * 65 + x) * 4..(y * 65 + x + 1) * 4]
        .try_into()
        .unwrap()
}

fn near_color(actual: [u8; 4], expected: [u8; 4]) {
    assert!(
        actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 2),
        "{actual:?} != {expected:?}"
    );
}

#[test]
#[ignore = "requires an offscreen GPU; run explicitly with --ignored"]
fn miniature_composites_only_sky_retains_depth_and_excludes_sky2d() {
    use crate::{GpuContext, Renderer, SceneMesh, ViewOptions};

    let fixture = Fixture::new();
    fixture.skies();
    fixture.material("test/green", [30, 220, 70, 255]);
    fixture.material("test/red", [230, 20, 20, 255]);
    let miniature =
        Arc::new(Scene::build(camera(), &[surface("test/green")], &fixture.fs).unwrap());
    let mut document = map_core::Document::empty();
    document
        .edit("mixed sky wall", |tx| {
            tx.world_properties_mut().set("skyname", "test");
            map_core::edit::create_block(
                tx,
                DVec3::new(64.0, 0.0, -64.0),
                DVec3::new(72.0, 64.0, 64.0),
                "tools/toolsskybox",
            )?;
            map_core::edit::create_block(
                tx,
                DVec3::new(64.0, -64.0, -64.0),
                DVec3::new(72.0, 0.0, 64.0),
                "tools/toolsskybox2d",
            )?;
            Ok(())
        })
        .unwrap();
    let ctx = GpuContext::new().unwrap();
    let mut renderer = Renderer::new(&ctx);
    let mut library = crate::MaterialLibrary::default();
    let mut mesh = SceneMesh::build(
        document.vmf(),
        &fixture.fs,
        &mut library,
        &Default::default(),
    );
    mesh.sky3d = Some(miniature.clone());
    let scene = renderer.upload(&ctx, &mesh, &library).unwrap();
    let main = crate::Camera::looking_at(Vec3::ZERO, Vec3::X);
    let options = ViewOptions {
        sky_preview: true,
        sky3d_preview: true,
        grid_visible: false,
        ..Default::default()
    };
    assert!(scene.sky3d_render_error(&main, &options).is_none());
    assert!(
        renderer
            .render_to_rgba_checked(&ctx, &scene, &main, &options, 0, 65)
            .is_err()
    );
    assert!(renderer.last_sky3d_error().is_some());
    let baseline = renderer.render_to_rgba(&ctx, &scene, &main, &options, 65, 65);
    assert!(renderer.last_sky3d_error().is_none());
    near_color(pixel(&baseline, 16, 32), [30, 220, 70, 255]);
    near_color(pixel(&baseline, 8, 32), [30, 220, 70, 255]);
    near_color(pixel(&baseline, 48, 32), [20, 50, 200, 255]);
    near_color(pixel(&baseline, 0, 0), [20, 50, 200, 255]);
    {
        let checker = (0..8)
            .flat_map(|row| {
                (0..8).map(move |column| {
                    if (row / 4 + column / 4) % 2 == 0 {
                        [30, 220, 70, 255]
                    } else {
                        [230, 20, 20, 255]
                    }
                })
            })
            .collect::<Vec<_>>();
        fixture.image("test/checker", [8, 8], &checker);
        fixture.write(
            "materials/test/checker.vmt",
            "UnlitGeneric { $basetexture test/checker }",
        );
        let mut polygon = surface("test/checker");
        polygon.texture_u = [0.0, 1.0, 0.0, 604.0];
        polygon.texture_v = [0.0, 0.0, -1.0, 304.0];
        let plain =
            Arc::new(Scene::build(camera(), std::slice::from_ref(&polygon), &fixture.fs).unwrap());
        fixture.write(
            "materials/test/checker.vmt",
            "UnlitGeneric { $basetexture test/checker $basetexturetransform \"translate 0.5 0\" }",
        );
        let shifted = Arc::new(Scene::build(camera(), &[polygon], &fixture.fs).unwrap());
        let mut patterned = SceneMesh::build(
            document.vmf(),
            &fixture.fs,
            &mut library,
            &Default::default(),
        );
        patterned.sky3d = Some(plain);
        let plain_gpu = renderer.upload(&ctx, &patterned, &library).unwrap();
        let plain_pixels = renderer
            .render_to_rgba_checked(&ctx, &plain_gpu, &main, &options, 65, 65)
            .unwrap();
        patterned.sky3d = Some(shifted);
        let shifted_gpu = renderer
            .upload_replacing(&ctx, Some(plain_gpu), &patterned, &library)
            .unwrap();
        let shifted_pixels = renderer
            .render_to_rgba_checked(&ctx, &shifted_gpu, &main, &options, 65, 65)
            .unwrap();
        near_color(pixel(&plain_pixels, 18, 18), [230, 20, 20, 255]);
        near_color(pixel(&plain_pixels, 18, 46), [30, 220, 70, 255]);
        near_color(pixel(&shifted_pixels, 18, 18), [30, 220, 70, 255]);
        near_color(pixel(&shifted_pixels, 18, 46), [230, 20, 20, 255]);
        near_color(pixel(&shifted_pixels, 48, 18), [20, 50, 200, 255]);
    }
    let moved = crate::Camera {
        position: Vec3::new(0.0, 16.0, 0.0),
        ..main
    };
    let translated = renderer.render_to_rgba(&ctx, &scene, &moved, &options, 65, 65);
    near_color(pixel(&translated, 8, 32), [20, 50, 200, 255]);
    near_color(pixel(&translated, 16, 32), [30, 220, 70, 255]);
    let wide = crate::Camera {
        fov_y_degrees: 90.0,
        ..main
    };
    let wider = renderer.render_to_rgba(&ctx, &scene, &wide, &options, 65, 65);
    near_color(pixel(&wider, 8, 32), [20, 50, 200, 255]);
    let disabled = ViewOptions {
        sky3d_preview: false,
        ..options
    };
    let without = renderer.render_to_rgba(&ctx, &scene, &main, &disabled, 65, 65);
    near_color(pixel(&without, 16, 32), [20, 50, 200, 255]);
    document
        .edit("beyond sky", |tx| {
            map_core::edit::create_block(
                tx,
                DVec3::new(96.0, 0.0, -64.0),
                DVec3::new(128.0, 64.0, 64.0),
                "test/red",
            )?;
            Ok(())
        })
        .unwrap();
    let mut mesh = SceneMesh::build(
        document.vmf(),
        &fixture.fs,
        &mut library,
        &Default::default(),
    );
    mesh.sky3d = Some(miniature.clone());
    let scene = renderer.upload(&ctx, &mesh, &library).unwrap();
    assert_eq!(
        baseline,
        renderer.render_to_rgba(&ctx, &scene, &main, &options, 65, 65)
    );
    document
        .edit("foreground", |tx| {
            map_core::edit::create_block(
                tx,
                DVec3::new(24.0, 4.0, -8.0),
                DVec3::new(32.0, 16.0, 8.0),
                "test/red",
            )?;
            Ok(())
        })
        .unwrap();
    let selected = std::collections::HashSet::from([map_core::ObjectId::Solid(
        document.vmf().world.solids.last().unwrap().id,
    )]);
    let mut mesh = SceneMesh::build(document.vmf(), &fixture.fs, &mut library, &selected);
    mesh.sky3d = Some(miniature);
    let scene = renderer.upload(&ctx, &mesh, &library).unwrap();
    let foreground = renderer.render_to_rgba(
        &ctx,
        &scene,
        &main,
        &ViewOptions {
            selection_outlines: false,
            ..options
        },
        65,
        65,
    );
    let color = pixel(&foreground, 16, 32);
    assert!(
        u16::from(color[0]) > u16::from(color[1]) * 2
            && u16::from(color[0]) > u16::from(color[2]) * 2,
        "{color:?}"
    );
    near_color(pixel(&foreground, 48, 32), [20, 50, 200, 255]);
    for surface in [
        crate::SurfaceView::Flat,
        crate::SurfaceView::Wireframe,
        crate::SurfaceView::LightmapDensity,
        crate::SurfaceView::SmoothingGroups,
    ] {
        let options = ViewOptions { surface, ..options };
        assert_eq!(
            renderer.render_to_rgba(&ctx, &scene, &main, &options, 65, 65),
            renderer.render_to_rgba(
                &ctx,
                &scene,
                &main,
                &ViewOptions {
                    sky3d_preview: false,
                    ..options
                },
                65,
                65
            )
        );
    }
    let filtered = ViewOptions {
        tool_textures: false,
        ..options
    };
    assert_ne!(
        renderer.render_to_rgba(&ctx, &scene, &main, &filtered, 65, 65),
        renderer.render_to_rgba(
            &ctx,
            &scene,
            &main,
            &ViewOptions {
                sky3d_preview: false,
                ..filtered
            },
            65,
            65
        )
    );
}

#[test]
#[ignore = "requires an offscreen GPU; run explicitly with --ignored"]
fn replacing_large_gpu_scenes_preserves_reuse_and_releases_old_admission() {
    let fixture = Fixture::new();
    let surfaces = fixture.large_compressed_materials();
    let first = Arc::new(Scene::build(camera(), &surfaces, &fixture.fs).unwrap());
    let second = Arc::new(Scene::build(camera(), &surfaces, &fixture.fs).unwrap());
    assert!(first.stats.gpu_bytes > MAX_GPU_ASSET_BYTES / 2);
    assert!(first.stats.upload_bytes >= 20 * 1024 * 1024);
    let ctx = crate::GpuContext::new().unwrap();
    let mut renderer = crate::Renderer::new(&ctx);
    let completed = Arc::new(AtomicBool::new(false));
    let completion = completed.clone();
    ctx.queue
        .on_submitted_work_done(move || completion.store(true, Ordering::Relaxed));
    let mut library = crate::MaterialLibrary::default();
    let document = map_core::Document::empty();
    let mut mesh = crate::SceneMesh::build(
        document.vmf(),
        &fixture.fs,
        &mut library,
        &Default::default(),
    );
    mesh.sky3d = Some(first.clone());
    let old = renderer.upload(&ctx, &mesh, &library).unwrap();
    assert!(old.sky3d_error().is_none());
    let other_view = renderer.upload(&ctx, &mesh, &library).unwrap();
    assert!(other_view.sky3d_error().is_none());
    let same = renderer
        .upload_replacing(&ctx, Some(old), &mesh, &library)
        .unwrap();
    assert!(same.sky3d_error().is_none());
    assert!(!completed.load(Ordering::Relaxed));
    mesh.sky3d = Some(second);
    let rejected = renderer
        .upload_replacing(&ctx, Some(same), &mesh, &library)
        .unwrap();
    assert!(rejected.sky3d_error().unwrap().contains("retained GPU"));
    let options = crate::ViewOptions {
        sky3d_preview: true,
        ..Default::default()
    };
    let main = crate::Camera::looking_at(Vec3::ZERO, Vec3::X);
    assert!(
        renderer
            .render_to_rgba_checked(&ctx, &rejected, &main, &options, 65, 65)
            .is_err()
    );
    drop(other_view);
    let admitted = renderer
        .upload_replacing(&ctx, Some(rejected), &mesh, &library)
        .unwrap();
    assert!(admitted.sky3d_error().is_none());
    assert!(completed.load(Ordering::Relaxed));
    mesh.sky3d = Some(first);
    let replacement = renderer
        .upload_replacing(&ctx, Some(admitted), &mesh, &library)
        .unwrap();
    assert!(replacement.sky3d_error().is_none());
}
