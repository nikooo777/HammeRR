use std::collections::BTreeMap;
use std::sync::Arc;

use glam::DVec3;
use map_core::{ObjectId, query::Ray};
use source_formats::fgd::Catalog;

use super::*;
use crate::session::{Actor, SelectMode};

const FGD: &str = r#"
@PointClass size(-2 -2 -2, 2 2 2) sprite() = env_sprite [
    model(sprite) : "Sprite Name" : "sprites/glow01.spr"
    scale(string) : "Scale" : ""
    rendercolor(color255) : "FX Color" : "255 255 255"
]
@PointClass size(4 4 4) iconsprite("sprites/glow01") = icon_sample []
@PointClass size(4 4 4) studio() = prop_dynamic [ model(studio) : "Model" ]
"#;

fn fixture(actor: Actor) -> (tempfile::TempDir, Session, u32) {
    let (directory, game) = crate::test_assets::game();
    std::fs::create_dir_all(directory.path().join("materials/sprites")).unwrap();
    std::fs::write(directory.path().join("materials/sprites/glow01.vmt"), "Sprite { $basetexture test/checker $spriteorientation vp_parallel $spriteorigin \"[0.5 0.5]\" }").unwrap();
    let mut session = Session::new(Ok(game));
    session.set_definitions(actor, Catalog::parse(FGD).unwrap());
    let ObjectId::Entity(id) = session
        .create_entity(
            actor,
            "env_sprite",
            Some(DVec3::ZERO),
            BTreeMap::from([("scale".into(), "1.5".into())]),
        )
        .unwrap()
    else {
        panic!()
    };
    (directory, session, id)
}

fn report(session: &Session, id: u32) -> Report {
    session
        .entity_helper_report(id)
        .unwrap()
        .sprite_resource
        .unwrap()
}

#[test]
fn physical_sprite_images_share_assets_while_ordinary_spatial_targets_stay_exact() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, id) = fixture(actor);
        let object = ObjectId::Entity(id);
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let camera = session.camera;
        let selection = session.selected_ids();
        let helper = session.entity_helper_report(id).unwrap();
        assert_eq!(helper.bounds_source, "fgd_size");
        assert!(!helper.unsupported.iter().any(|helper| helper == "sprite"));
        let guide = helper.sprite_resource.unwrap();
        assert_eq!(guide.state, "ready");
        assert!(guide.displayed && !guide.runtime_fields_evaluated);
        assert_eq!(guide.model.raw.as_deref(), Some("sprites/glow01.spr"));
        assert_eq!(
            guide.resolved_vmt.as_deref(),
            Some("materials/sprites/glow01.vmt")
        );
        assert_eq!(guide.world_size, Some([96.0, 48.0]));
        assert!(guide.image.unwrap().source.is_some());
        assert!(
            session
                .model_instances(session.document())
                .issues
                .is_empty()
        );
        assert!(
            session
                .inspect_entity_model(id)
                .unwrap_err()
                .contains("Sprite resource source")
        );
        let bounds = Some((DVec3::splat(-2.0), DVec3::splat(2.0)));
        assert_eq!(
            session.object_bounds_in(session.document(), &[object]),
            bounds
        );
        assert_eq!(
            session
                .pick(&Ray {
                    origin: DVec3::new(0.0, -100.0, 0.0),
                    direction: DVec3::Y
                })
                .unwrap()
                .object,
            object
        );
        assert!(
            session
                .pick(&Ray {
                    origin: DVec3::new(20.0, -100.0, 0.0),
                    direction: DVec3::Y
                })
                .is_none()
        );
        let mesh = session.build_scene();
        assert_eq!(mesh.icons.len(), 1);
        assert_eq!(mesh.icons[0].world_size, [96.0, 48.0]);
        assert!(mesh.vertices.is_empty());
        assert!(!mesh.selection_outline_ranges.is_empty());
        assert_eq!(Some((mesh.mins.as_dvec3(), mesh.maxs.as_dvec3())), bounds);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        assert_eq!(session.camera, camera);
        assert_eq!(session.selected_ids(), selection);

        session
            .create_entity(
                actor,
                "env_sprite",
                Some(DVec3::X * 200.0),
                BTreeMap::from([("scale".into(), "0.5".into())]),
            )
            .unwrap();
        session
            .create_entity(
                actor,
                "icon_sample",
                Some(DVec3::X * 400.0),
                BTreeMap::new(),
            )
            .unwrap();
        session.select(&[], SelectMode::Replace).unwrap();
        let mesh = session.build_scene();
        assert_eq!(mesh.icons.len(), 3);
        assert_eq!(
            mesh.icons
                .iter()
                .map(|instance| instance.world_size)
                .collect::<Vec<_>>(),
            [[96.0, 48.0], [32.0, 16.0], [32.0, 16.0]]
        );
        assert!(Arc::ptr_eq(&mesh.icons[0].asset, &mesh.icons[1].asset));
        assert!(Arc::ptr_eq(&mesh.icons[0].asset, &mesh.icons[2].asset));
    }
}

#[test]
fn scale_and_dispatch_failures_never_invent_model_errors_or_read_assets() {
    let (_directory, mut session, id) = fixture(Actor::Agent);
    for raw in ["", "0", "-1", "bad", "NaN", "inf", "1e99", "1e-99", "65536"] {
        session
            .set_entity_properties(Actor::Agent, id, &[("scale".into(), Some(raw.into()))])
            .unwrap();
        let guide = report(&session, id);
        assert_eq!(guide.state, "unavailable", "{raw}");
        assert!(!guide.displayed && guide.error.is_some());
        assert_eq!(guide.scale.raw.as_deref(), Some(raw));
        assert!(session.build_scene().icons.is_empty());
        assert!(
            session
                .model_instances(session.document())
                .issues
                .is_empty()
        );
    }
    session
        .set_entity_properties(Actor::Agent, id, &[("scale".into(), Some("1".into()))])
        .unwrap();
    for raw in [
        "sprites/glow01",
        "custom/glow01.spr",
        "sprites/foo.bar.spr",
        "materials/sprites/foo.bar.vmt",
    ] {
        session
            .set_entity_properties(Actor::Agent, id, &[("model".into(), Some(raw.into()))])
            .unwrap();
        let guide = report(&session, id);
        assert_eq!(guide.state, "unavailable");
        assert!(guide.image.is_none() && guide.error.is_some());
        assert_eq!(guide.model.raw.as_deref(), Some(raw));
        assert!(
            session
                .model_instances(session.document())
                .issues
                .is_empty()
        );
    }
    let ObjectId::Entity(mdl) = session
        .create_entity(
            Actor::Agent,
            "prop_dynamic",
            Some(DVec3::ZERO),
            BTreeMap::from([("model".into(), "malformed.spr".into())]),
        )
        .unwrap()
    else {
        panic!()
    };
    assert!(
        session
            .model_instances(session.document())
            .issues
            .contains_key(&mdl)
    );
}

#[test]
fn image_visibility_is_lazy_temporary_and_shared_with_reload_failures() {
    let (directory, mut session, id) = fixture(Actor::Agent);
    let path = directory.path().join("materials/sprites/glow01.vmt");
    let healthy = std::fs::read(&path).unwrap();
    std::fs::write(&path, "broken winning material").unwrap();
    session.set_view(render::ViewOptions {
        entity_images: false,
        ..session.view()
    });
    let deferred = report(&session, id);
    assert_eq!(deferred.suppression_reason, Some("view_disabled"));
    assert!(deferred.image.is_none());
    std::fs::write(&path, &healthy).unwrap();
    let disabled = session.view();
    assert_eq!(
        session
            .build_scene_for_view(render::ViewOptions {
                entity_images: true,
                ..disabled
            })
            .icons
            .len(),
        1
    );
    assert_eq!(session.view(), disabled);
    assert!(!report(&session, id).displayed);
    session.set_view(render::ViewOptions {
        entity_images: true,
        ..disabled
    });
    assert!(report(&session, id).displayed);
    std::fs::write(&path, "broken winning material").unwrap();
    session.reload_material_assets(Actor::Agent);
    let unavailable = report(&session, id);
    assert_eq!(unavailable.state, "unavailable");
    assert!(unavailable.image.unwrap().source.is_some());
    std::fs::write(&path, healthy).unwrap();
    assert!(!report(&session, id).displayed);
    session.reload_material_assets(Actor::Agent);
    assert!(report(&session, id).displayed);
}

#[test]
fn labeled_fgd_defaults_and_excluded_orientations_remain_explicit() {
    let (_directory, mut session, id) = fixture(Actor::Agent);
    session.set_definitions(
        Actor::Agent,
        Catalog::parse(&FGD.replace(
            "scale(string) : \"Scale\" : \"\"",
            "scale(string) : \"Scale\" : \"2\"",
        ))
        .unwrap(),
    );
    session
        .set_entity_properties(
            Actor::Agent,
            id,
            &[("model".into(), None), ("scale".into(), None)],
        )
        .unwrap();
    let defaults = report(&session, id);
    assert!(defaults.displayed);
    assert_eq!(defaults.model.value_source, "fgd_default");
    assert_eq!(defaults.scale.value_source, "fgd_default");
    assert_eq!(defaults.world_size, Some([128.0, 64.0]));
    for (key, raw) in [
        ("angles", "0 0 1"),
        ("angle", "90"),
        ("parentname", "parent"),
    ] {
        session
            .set_entity_properties(Actor::Agent, id, &[(key.into(), Some(raw.into()))])
            .unwrap();
        assert!(!report(&session, id).displayed);
        assert!(session.build_scene().icons.is_empty());
        session
            .set_entity_properties(Actor::Agent, id, &[(key.into(), None)])
            .unwrap();
    }
    let inspector = session.entity_inspector(id).unwrap();
    let name = inspector
        .properties
        .iter()
        .find(|field| field.key == "model")
        .unwrap();
    assert_eq!(name.label, "Sprite Name");
    assert_eq!(name.control, crate::inspector::Control::Material);
    let color = inspector
        .properties
        .iter()
        .find(|field| field.key == "rendercolor")
        .unwrap();
    assert_eq!(color.label, "FX Color");
    assert!(!color.description.contains("model textures"));
}

#[test]
fn malformed_imported_angles_keep_sprite_unavailable_without_mutation() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, mut session, id) = fixture(actor);
        for raw in ["NaN 0 0", "inf 0 0", "0 0", "0 0 0 0"] {
            let source = session.document().vmf().clone();
            let history = session.document().undo_labels().count();
            let error = session
                .set_entity_properties(actor, id, &[("angles".into(), Some(raw.into()))])
                .unwrap_err();
            assert_eq!(error, "angles expects three finite coordinates");
            assert_eq!(session.document().vmf(), &source);
            assert_eq!(session.document().undo_labels().count(), history);

            let path = directory.path().join("malformed-sprite-angles.vmf");
            let text = format!(
                r#"world {{ "id" "1" "classname" "worldspawn" }}
entity {{ "id" "{id}" "classname" "env_sprite" "origin" "0 0 0"
"model" "sprites/glow01.spr" "scale" "1" "angles" "{raw}" }}
"#
            );
            std::fs::write(&path, &text).unwrap();
            session.open(actor, &path, true).unwrap();
            let source = session.document().vmf().clone();
            let history = session.document().undo_labels().count();
            let camera = session.camera();
            let selection = session.selected_ids();
            assert_eq!(
                session
                    .document()
                    .entity(id)
                    .unwrap()
                    .properties
                    .get("angles"),
                Some(raw)
            );
            let guide = report(&session, id);
            assert_eq!(guide.state, "unavailable");
            assert!(!guide.displayed && guide.image.is_none());
            assert!(guide.error.as_deref().unwrap().contains("angles"));
            assert!(session.build_scene().icons.is_empty());
            assert!(
                session
                    .model_instances(session.document())
                    .issues
                    .is_empty()
            );
            assert_eq!(session.document().vmf(), &source);
            assert_eq!(session.document().undo_labels().count(), history);
            assert_eq!(session.camera(), camera);
            assert_eq!(session.selected_ids(), selection);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

            session
                .set_entity_properties(actor, id, &[("angles".into(), Some("0 0 0".into()))])
                .unwrap();
            assert!(report(&session, id).displayed);
            session.undo(actor).unwrap();
            assert_eq!(session.document().vmf(), &source);
            assert!(!report(&session, id).displayed);
        }
    }
}

#[test]
fn shared_visible_image_limit_uses_entity_order_for_icons_and_sprite_resources() {
    let (_directory, mut session, first) = fixture(Actor::Agent);
    let mut last = first;
    for index in 0..render::icons::MAX_INSTANCES {
        let classname = if index % 2 == 0 {
            "icon_sample"
        } else {
            "env_sprite"
        };
        let ObjectId::Entity(id) = session
            .create_entity(
                Actor::Agent,
                classname,
                Some(DVec3::ZERO),
                BTreeMap::from([("scale".into(), "1".into())]),
            )
            .unwrap()
        else {
            panic!()
        };
        last = id;
    }
    assert_eq!(
        session.build_scene().icons.len(),
        render::icons::MAX_INSTANCES
    );
    assert!(report(&session, first).displayed);
    assert_eq!(
        report(&session, last).suppression_reason,
        Some("scene_image_limit")
    );
}

fn additive_definitions() -> Catalog {
    Catalog::parse(&FGD.replace(
        "rendercolor(color255) : \"FX Color\" : \"255 255 255\"",
        "rendercolor(color255) : \"FX Color\" : \"255 255 255\"\nrenderamt(integer) : \"FX Amount\" : 255\nrendermode(choices) : \"Render Mode\" : 0 = [0 : \"Normal\" 5 : \"Additive\"]\nrenderfx(integer) : \"Render FX\" : 0",
    )).unwrap()
}

#[test]
fn additive_sprite_guides_are_opt_in_per_instance_and_preserve_owned_reviews() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, id) = fixture(actor);
        session.set_definitions(actor, additive_definitions());
        session
            .set_entity_properties(
                actor,
                id,
                &[
                    ("rendermode".into(), Some("5".into())),
                    ("HDRColorScale".into(), Some("0.7".into())),
                ],
            )
            .unwrap();
        session
            .create_entity(
                actor,
                "icon_sample",
                Some(DVec3::X * 200.0),
                BTreeMap::new(),
            )
            .unwrap();
        session
            .create_entity(
                actor,
                "env_sprite",
                Some(DVec3::X * 400.0),
                BTreeMap::from([("scale".into(), "1".into())]),
            )
            .unwrap();
        session.select(&[], SelectMode::Replace).unwrap();
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let bounds = session.object_bounds_in(session.document(), &[ObjectId::Entity(id)]);
        let before = session.build_scene();
        assert!(
            before
                .icons
                .iter()
                .all(|instance| instance.blend == render::icons::Blend::Alpha)
        );
        assert_eq!(
            report(&session, id).rendering_policy,
            "unlit_rgba_editor_image_no_fog"
        );
        session
            .preview_transform(
                actor,
                &[ObjectId::Entity(id)],
                map_core::edit::Transform {
                    translation: DVec3::Y * 8.0,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let preview = session.render_document().vmf().clone();
        let view = session
            .resolve_view_update(crate::session::ViewUpdate {
                additive_sprite_guides: Some(true),
                ..Default::default()
            })
            .unwrap();
        session.set_view(view);
        assert_eq!(session.render_document().vmf(), &preview);
        let mesh = session.build_scene();
        assert_eq!(
            mesh.icons
                .iter()
                .map(|instance| instance.blend)
                .collect::<Vec<_>>(),
            [
                render::icons::Blend::Additive,
                render::icons::Blend::Alpha,
                render::icons::Blend::Alpha
            ]
        );
        assert!(Arc::ptr_eq(&before.icons[0].asset, &mesh.icons[0].asset));
        assert!(Arc::ptr_eq(&mesh.icons[0].asset, &mesh.icons[1].asset));
        assert!(Arc::ptr_eq(&mesh.icons[0].asset, &mesh.icons[2].asset));
        let guide = report(&session, id);
        assert!(
            guide.displayed
                && guide.blend_preview.applied
                && guide.blend_preview.appearance_supported
        );
        assert_eq!(guide.blend_preview.rendermode.value_source, "authored");
        assert_eq!(guide.blend_preview.renderamt.value_source, "fgd_default");
        assert_eq!(guide.blend_preview.evaluated_guide_mode, Some(5));
        assert!(guide.unevaluated_runtime_fields.contains(&"HDRColorScale"));
        assert!(!guide.unevaluated_runtime_fields.contains(&"rendermode"));
        assert!(
            session
                .entity_helper_report(id)
                .unwrap()
                .status
                .contains("authored Additive LDR")
        );
        let disabled = render::ViewOptions {
            additive_sprite_guides: false,
            ..session.view()
        };
        assert!(
            session
                .build_scene_for_view(disabled)
                .icons
                .iter()
                .all(|instance| instance.blend == render::icons::Blend::Alpha)
        );
        assert!(session.view().additive_sprite_guides);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        assert_eq!(
            session.object_bounds_in(session.document(), &[ObjectId::Entity(id)]),
            bounds
        );
    }
}

#[test]
fn additive_sprite_appearance_constraints_never_invent_defaults_or_runtime_tint() {
    let (_directory, session, id) = fixture(Actor::Agent);
    let catalog = additive_definitions();
    let class = catalog.class("env_sprite").unwrap();
    let mut entity = session.document().entity(id).unwrap().clone();
    entity.properties.set("rendermode", "5");
    let baseline = BlendPreview::resolve(class, &entity, true);
    assert!(baseline.appearance_supported);
    for (key, value, error) in [
        ("rendermode", "0", "Render Mode 0"),
        ("rendermode", "3", "Render Mode 3"),
        ("rendermode", "9", "Render Mode 9"),
        ("rendermode", "NaN", "integer byte"),
        ("rendercolor", "254 255 255", "FX Color"),
        ("rendercolor", "255 255", "FX Color"),
        ("renderamt", "128", "FX Amount"),
        ("renderamt", "0", "FX Amount"),
        ("renderamt", "255.0", "integer byte"),
        ("renderamt", "inf", "integer byte"),
        ("renderfx", "1", "Render FX"),
    ] {
        let mut changed = entity.clone();
        changed.properties.set(key, value);
        let report = BlendPreview::resolve(class, &changed, true);
        assert!(!report.appearance_supported && !report.applied);
        assert_eq!(report.effective_policy, "raw_rgba");
        assert!(report.error.unwrap().contains(error), "{key}={value}");
        assert!(
            BlendPreview::resolve(class, &changed, false)
                .error
                .is_none()
        );
    }
    let mut oversized = entity.clone();
    oversized.properties.set("rendercolor", " ".repeat(257));
    let rejected = BlendPreview::resolve(class, &oversized, true);
    assert!(rejected.rendercolor.raw_truncated);
    assert!(rejected.error.unwrap().contains("256 bytes"));
    for key in ["rendermode", "rendercolor", "renderamt", "renderfx"] {
        let mut duplicate = entity.clone();
        let value = match key {
            "rendermode" => "5",
            "rendercolor" => "255 255 255",
            "renderamt" => "255",
            _ => "0",
        };
        duplicate.properties.0.push((key.into(), value.into()));
        duplicate.properties.0.push((key.into(), value.into()));
        assert!(
            BlendPreview::resolve(class, &duplicate, true)
                .error
                .unwrap()
                .contains("Duplicate")
        );
    }
    entity.properties.remove("rendermode");
    let defaults = Catalog::parse(&FGD.replace(
        "rendercolor(color255)",
        "rendermode(integer) : \"Mode\" : 5\nrendercolor(color255)",
    ))
    .unwrap();
    let report = BlendPreview::resolve(defaults.class("env_sprite").unwrap(), &entity, true);
    assert_eq!(report.rendermode.value_source, "fgd_default");
    assert!(report.error.unwrap().contains("explicitly authored"));
    entity.properties.set("rendermode", "5");
    let missing = Catalog::parse(FGD).unwrap();
    assert!(
        BlendPreview::resolve(missing.class("env_sprite").unwrap(), &entity, true)
            .error
            .unwrap()
            .contains("renderamt")
    );
}

#[test]
fn additive_sprite_unavailable_appearance_keeps_raw_image_and_parent_visibility_is_lazy() {
    let (directory, mut session, id) = fixture(Actor::Agent);
    session.set_definitions(Actor::Agent, additive_definitions());
    session
        .set_entity_properties(
            Actor::Agent,
            id,
            &[
                ("rendermode".into(), Some("5".into())),
                ("renderamt".into(), Some("128".into())),
            ],
        )
        .unwrap();
    session.set_view(render::ViewOptions {
        additive_sprite_guides: true,
        entity_images: false,
        ..session.view()
    });
    let path = directory.path().join("materials/sprites/glow01.vmt");
    let good = std::fs::read(&path).unwrap();
    std::fs::write(&path, "broken winning material").unwrap();
    let deferred = report(&session, id);
    assert_eq!(deferred.suppression_reason, Some("view_disabled"));
    assert!(deferred.image.is_none());
    std::fs::write(&path, &good).unwrap();
    session.set_view(render::ViewOptions {
        entity_images: true,
        ..session.view()
    });
    let raw = report(&session, id);
    assert!(raw.displayed && raw.error.is_none() && raw.blend_preview.error.is_some());
    assert_eq!(
        session.build_scene().icons[0].blend,
        render::icons::Blend::Alpha
    );
    assert!(
        session
            .entity_helper_report(id)
            .unwrap()
            .status
            .contains("Additive guide unavailable")
    );
    std::fs::write(&path, "broken winning material").unwrap();
    session.reload_material_assets(Actor::Agent);
    assert!(!report(&session, id).displayed);
    std::fs::write(&path, &good).unwrap();
    assert!(!report(&session, id).displayed);
    session.reload_material_assets(Actor::Agent);
    assert!(report(&session, id).displayed);
}
