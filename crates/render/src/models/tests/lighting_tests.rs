use super::*;
use crate::{SceneMesh, SurfaceView, ViewOptions};

fn fixture() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    let game = material_game(directory.path());
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    for (name, extra) in [
        ("opaque", ""),
        ("unsupported", "$phong 0"),
        ("tool", "%compilenodraw 1"),
    ] {
        std::fs::write(
            directory.path().join(format!("materials/{name}.vmt")),
            format!("VertexLitGeneric {{ $basetexture opaque {extra} }}"),
        )
        .unwrap();
    }
    std::fs::write(
        directory.path().join("materials/opaque.vtf"),
        opacity_texture([255; 4]),
    )
    .unwrap();
    (directory, game)
}

fn mesh(
    game: &GameFs,
    materials: &mut crate::MaterialLibrary,
    instance: &Instance,
    view: ViewOptions,
) -> SceneMesh {
    let mut vmf = Document::empty().vmf().clone();
    let ObjectId::Entity(id) = instance.id else {
        unreachable!()
    };
    let mut entity = Entity::new(id, "prop_dynamic");
    entity.properties.set("origin", "0 0 0");
    vmf.entities.push(entity);
    SceneMesh::build_with_previews_filtered(
        &vmf,
        game,
        materials,
        (&HashSet::new(), &HashSet::new()),
        &[],
        (std::slice::from_ref(instance), &Default::default()),
        (&Default::default(), &[]),
        None,
        view.tool_textures,
        false,
        view.lighting_preview && view.surface == SurfaceView::Textured,
        |_| true,
    )
}

fn two_parts() -> Arc<Asset> {
    let mut asset = asset();
    let model = &mut Arc::get_mut(&mut asset).unwrap().model;
    let indices = model.parts[0].indices.clone();
    model.parts.push(Part {
        material: 1,
        indices,
    });
    model.materials = vec!["opaque".into(), "unsupported".into()];
    model.skins = vec![vec![0, 1], vec![0, 0]];
    model.body_groups = vec![source_formats::model::BodyGroup {
        name: "parts".into(),
        base: 1,
        variants: vec![
            source_formats::model::BodyVariant {
                name: "one".into(),
                parts: 0..1,
                bounds: Some(model.bounds),
            },
            source_formats::model::BodyVariant {
                name: "both".into(),
                parts: 0..2,
                bounds: Some(model.bounds),
            },
        ],
    }];
    *asset.materials.write().unwrap() = Ok(vec!["opaque".into(), "unsupported".into()]);
    asset
}

#[test]
fn selected_body_skin_materials_are_whole_instance_and_shared_vertices_stay_exact() {
    let (_directory, game) = fixture();
    let mut materials = crate::MaterialLibrary::default();
    let asset = two_parts();
    let mut entity = Entity::new(7, "prop_dynamic");
    entity.properties.set("origin", "10 20 30");
    entity.properties.set("angles", "20 35 10");
    entity.properties.set("modelscale", "2");
    entity.properties.set("rendercolor", "64 128 255");
    let off = ViewOptions::default();
    let on = ViewOptions {
        lighting_preview: true,
        ..off
    };
    for (body, skin, admitted, parts) in [(0, 0, true, 1), (1, 0, false, 2), (1, 1, true, 2)] {
        entity.properties.set("body", body.to_string());
        entity.properties.set("skin", skin.to_string());
        let instance = Instance::new(&entity, asset.clone()).unwrap();
        let ordinary = mesh(&game, &mut materials, &instance, off);
        let lit = mesh(&game, &mut materials, &instance, on);
        assert_eq!(
            lit.lighting.report.models.admitted_instances,
            usize::from(admitted)
        );
        assert_eq!(
            lit.lighting.report.models.excluded_instances,
            usize::from(!admitted)
        );
        assert_eq!(
            lit.lighting.report.models.admitted_parts,
            if admitted { parts } else { 0 }
        );
        assert_eq!(
            lit.lighting.report.models.excluded_parts,
            if admitted { 0 } else { parts }
        );
        assert_eq!(
            lit.lighting.report.models.admitted_triangles,
            if admitted { parts * 2 } else { 0 }
        );
        assert_eq!(
            lit.lighting.report.models.excluded_triangles,
            if admitted { 0 } else { parts * 2 }
        );
        assert_eq!(lit.indices, ordinary.indices);
        let mut vertices = lit.vertices.clone();
        for vertex in &mut vertices {
            assert_eq!(vertex.lighting_receiver, u32::from(admitted));
            vertex.lighting_receiver = 0;
        }
        assert_eq!(vertices, ordinary.vertices);
        assert!(
            lit.vertices
                .iter()
                .all(|vertex| vertex.color == [64, 128, 255, 0])
        );
        let expected = (instance.rotation * DVec3::Z).as_vec3();
        assert!(glam::Vec3::from_array(lit.vertices[0].normal).abs_diff_eq(expected, 1e-6));
        if !admitted {
            assert!(
                lit.lighting
                    .report
                    .diagnostics
                    .iter()
                    .any(|row| row.entity == Some(7)
                        && row.material.as_deref() == Some("unsupported")
                        && row.reason.contains("$phong"))
            );
        }
    }
    entity.properties.set("body", "0");
    entity.properties.set("skin", "0");
    for amount in ["0", "255"] {
        entity.properties.set("rendermode", "2");
        entity.properties.set("renderamt", amount);
        let instance = Instance::new(&entity, asset.clone()).unwrap();
        let lit = mesh(&game, &mut materials, &instance, on);
        assert_eq!(lit.lighting.report.models.excluded_instances, 1);
        assert!(
            lit.vertices
                .iter()
                .all(|vertex| vertex.lighting_receiver == 0)
        );
        assert!(
            lit.lighting.report.diagnostics[0]
                .reason
                .contains("Normal render mode")
        );
    }
}

#[test]
fn mixed_tool_material_drawn_counts_follow_actual_request_and_nontextured_views_do_no_admission() {
    let (_directory, game) = fixture();
    let mut materials = crate::MaterialLibrary::default();
    let asset = two_parts();
    *asset.materials.write().unwrap() = Ok(vec!["opaque".into(), "tool".into()]);
    let mut entity = Entity::new(7, "prop_dynamic");
    entity.properties.set("origin", "0 0 0");
    entity.properties.set("body", "1");
    let instance = Instance::new(&entity, asset).unwrap();
    for tools in [true, false, true] {
        let lit = mesh(
            &game,
            &mut materials,
            &instance,
            ViewOptions {
                lighting_preview: true,
                tool_textures: tools,
                ..Default::default()
            },
        );
        assert_eq!(lit.lighting.report.models.excluded_instances, 1);
        assert_eq!(lit.lighting.report.models.excluded_parts, 2);
        assert_eq!(
            lit.lighting.report.models.excluded_triangles,
            if tools { 4 } else { 2 }
        );
        assert_eq!(lit.indices.len(), 12);
    }
    for surface in [
        SurfaceView::Flat,
        SurfaceView::Wireframe,
        SurfaceView::LightmapDensity,
        SurfaceView::SmoothingGroups,
    ] {
        let lit = mesh(
            &game,
            &mut materials,
            &instance,
            ViewOptions {
                lighting_preview: true,
                surface,
                ..Default::default()
            },
        );
        assert_eq!(lit.lighting.report.models, Default::default());
        assert!(
            lit.vertices
                .iter()
                .all(|vertex| vertex.lighting_receiver == 0)
        );
    }
}

#[test]
fn cached_base_and_model_capability_change_together_only_on_material_reload() {
    let (directory, game) = fixture();
    let mut materials = crate::MaterialLibrary::default();
    let (_entity, instance) = opacity_model(7, 0.0, "0", "0", "opaque");
    assert_eq!(
        mesh(
            &game,
            &mut materials,
            &instance,
            ViewOptions {
                lighting_preview: true,
                ..Default::default()
            }
        )
        .lighting
        .report
        .models
        .admitted_instances,
        1
    );
    let identity = materials.find("opaque").unwrap().source_identity();
    std::fs::write(
        directory.path().join("materials/opaque.vmt"),
        "VertexLitGeneric { $basetexture opaque $bumpmap no/fresh/read }",
    )
    .unwrap();
    assert_eq!(
        mesh(
            &game,
            &mut materials,
            &instance,
            ViewOptions {
                lighting_preview: true,
                ..Default::default()
            }
        )
        .lighting
        .report
        .models
        .admitted_instances,
        1
    );
    assert_eq!(
        materials.find("opaque").unwrap().source_identity(),
        identity
    );
    materials.reload(&game);
    let rejected = mesh(
        &game,
        &mut materials,
        &instance,
        ViewOptions {
            lighting_preview: true,
            ..Default::default()
        },
    );
    assert_eq!(rejected.lighting.report.models.excluded_instances, 1);
    assert!(
        rejected.lighting.report.diagnostics[0]
            .reason
            .contains("$bumpmap")
    );
    assert_ne!(
        materials.find("opaque").unwrap().source_identity(),
        identity
    );
    for extra in [
        "$alpha 1",
        "$halflambert 0",
        "$normalmap missing",
        "$detail missing",
        "$envmap env_cubemap",
        "$basetexturetransform identity",
        "$frame 0",
        "$translucent 1",
        "$alphatest 1",
        "Proxies { }",
        "$selfillum 0",
    ] {
        std::fs::write(
            directory.path().join("materials/opaque.vmt"),
            format!("VertexLitGeneric {{ $basetexture opaque {extra} }}"),
        )
        .unwrap();
        materials.reload(&game);
        assert_eq!(
            mesh(
                &game,
                &mut materials,
                &instance,
                ViewOptions {
                    lighting_preview: true,
                    ..Default::default()
                }
            )
            .lighting
            .report
            .models
            .excluded_instances,
            1,
            "{extra}"
        );
    }
}

#[test]
fn unsafe_packed_model_normals_and_triangles_keep_the_ordinary_geometry() {
    let (_directory, game) = fixture();
    let on = ViewOptions {
        lighting_preview: true,
        ..Default::default()
    };
    for case in ["opposite", "perpendicular", "zero", "collapsed"] {
        let mut asset = asset();
        let model = &mut Arc::get_mut(&mut asset).unwrap().model;
        match case {
            "opposite" => model.vertices[0].normal = [0., 0., -1.],
            "perpendicular" => model.vertices[0].normal = [1., 0., 0.],
            "zero" => model.vertices[0].normal = [0.; 3],
            "collapsed" => model.vertices[1].position = model.vertices[0].position,
            _ => unreachable!(),
        }
        *asset.materials.write().unwrap() = Ok(vec!["opaque".into()]);
        let mut entity = Entity::new(7, "prop_dynamic");
        entity.properties.set("origin", "0 0 0");
        let instance = Instance::new(&entity, asset).unwrap();
        let mut materials = crate::MaterialLibrary::default();
        let ordinary = mesh(&game, &mut materials, &instance, ViewOptions::default());
        let rejected = mesh(&game, &mut materials, &instance, on);
        assert_eq!(
            rejected.lighting.report.models.excluded_instances, 1,
            "{case}"
        );
        assert_eq!(rejected.vertices, ordinary.vertices, "{case}");
        assert_eq!(rejected.indices, ordinary.indices, "{case}");
    }
}

#[test]
fn inserted_model_base_capability_rejects_shader_mismatch_animation_and_display_substitutes() {
    for (case, reason) in [
        ("shader", "VertexLitGeneric"),
        ("frames", "static single-frame"),
        ("faces", "static single-frame"),
        ("depth", "static single-frame"),
        ("format", "LDR RGB"),
        ("display", "declared base"),
    ] {
        let material = source_formats::vmt::Material::parse(if case == "shader" {
            "UnlitGeneric { $basetexture opaque }"
        } else {
            "VertexLitGeneric { $basetexture opaque }"
        })
        .unwrap();
        let mut base = source_formats::vtf::Vtf::parse(opacity_texture([255; 4])).unwrap();
        match case {
            "frames" => base.frames = 2,
            "faces" => base.faces = 6,
            "depth" => base.depth = 2,
            "format" => base.format = source_formats::vtf::ImageFormat::Rgba16161616F,
            _ => {}
        }
        let mut materials = crate::MaterialLibrary::default();
        let id = materials.insert_base_texture(
            "candidate",
            &material,
            base,
            if case == "display" {
                "materials/substitute.vtf"
            } else {
                "materials/opaque.vtf"
            }
            .into(),
        );
        assert!(
            materials
                .get(id)
                .model_lighting_error
                .as_deref()
                .unwrap()
                .contains(reason),
            "{case}"
        );
    }
}
