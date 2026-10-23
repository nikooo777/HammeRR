use super::*;
use source_formats::fgd::Catalog;
use std::collections::BTreeMap;

const FGD: &str = r#"
@BaseClass = Angles [ angles(angle) : "Pitch Yaw Roll (Y Z X)" : "0 0 0" ]
@PointClass base(Angles) = light_environment [
    pitch(integer) : "Pitch" : 0
    _light(color255) : "Brightness" : "255 255 255 200"
    _ambient(color255) : "Ambient" : "255 255 255 20"
    _lightHDR(color255) : "BrightnessHDR" : "-1 -1 -1 1"
    _lightscaleHDR(float) : "BrightnessScaleHDR" : "1"
    _ambientHDR(color255) : "AmbientHDR" : "-1 -1 -1 1"
    _AmbientScaleHDR(float) : "AmbientScaleHDR" : "1"
    SunSpreadAngle(float) : "SunSpreadAngle" : 0
]
"#;

struct Authored {
    _directory: tempfile::TempDir,
    session: Session,
    source: PathBuf,
    child: PathBuf,
    owner: ObjectId,
    helper: u32,
    bare_child: Vec<u8>,
}

fn authored(actor: Actor, mixed: bool) -> Authored {
    let (directory, game) = crate::test_assets::game();
    let source = directory.path().join("parent.vmf");
    let child = directory.path().join("child.vmf");
    let definitions = crate::steam::find_css_fgd(&game)
        .map_or_else(|| Catalog::parse(FGD), |path| Catalog::load(&path))
        .unwrap();
    let defaults = definitions.class("light_environment").unwrap().defaults();
    assert_eq!(defaults["_AmbientScaleHDR"], "1");
    assert_eq!(defaults["SunSpreadAngle"], "0");
    let mut session = Session::new(Ok(game));
    session.set_definitions(actor, definitions);
    if mixed {
        session
            .create_block(
                actor,
                DVec3::new(0.0, 0.0, 32.0),
                DVec3::new(16.0, 16.0, 48.0),
                "test/checker",
                false,
            )
            .unwrap();
    }
    session.save(actor, Some(&child)).unwrap();
    let bare_child = std::fs::read(&child).unwrap();
    let ObjectId::Entity(helper) = session
        .create_entity(
            actor,
            "light_environment",
            Some(DVec3::new(80.0, -16.0, 96.0)),
            BTreeMap::new(),
        )
        .unwrap()
    else {
        unreachable!()
    };
    for (key, value) in &defaults {
        assert!(
            session
                .document()
                .entity(helper)
                .unwrap()
                .properties
                .iter()
                .any(|(name, authored)| name == key.as_str() && authored == value.as_str()),
            "{key}"
        );
    }
    session
        .set_entity_properties(actor, helper, &[("pitch".into(), Some("-45".into()))])
        .unwrap();
    assert_eq!(
        session
            .document()
            .entity(helper)
            .unwrap()
            .properties
            .get("pitch"),
        Some("-45")
    );
    session.save(actor, None).unwrap();
    std::fs::write(
        &source,
        crate::test_assets::room().write(&Default::default()),
    )
    .unwrap();
    session.open(actor, &source, false).unwrap();
    let owner = session
        .create_entity(
            actor,
            "func_instance",
            Some(DVec3::ZERO),
            BTreeMap::from([("file".into(), "child.vmf".into())]),
        )
        .unwrap();
    session.save(actor, None).unwrap();
    std::fs::write(source.with_extension("bsp"), b"previous BSP").unwrap();
    std::fs::write(source.with_extension("lin"), b"previous pointfile").unwrap();
    session.set_view(render::ViewOptions {
        instance_contents: true,
        ..session.view()
    });
    session.select(&[owner], SelectMode::Replace).unwrap();
    Authored {
        _directory: directory,
        session,
        source,
        child,
        owner,
        helper,
        bare_child,
    }
}

#[test]
fn bound_environment_colors_refresh_without_parent_preview_and_preserve_saved_templates() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = authored(actor, false);
        let session = &mut fixture.session;
        session.open(actor, &fixture.child, false).unwrap();
        session
            .set_entity_properties(
                actor,
                fixture.helper,
                &[
                    ("_light".into(), Some("$Sun".into())),
                    ("_lightHDR".into(), Some("$SunHDR".into())),
                    ("_ambient".into(), Some("$Sky".into())),
                    ("_ambientHDR".into(), Some("$SkyHDR".into())),
                    ("_lightscaleHDR".into(), Some("$SunGain".into())),
                    ("_AmbientScaleHDR".into(), Some("$SkyGain".into())),
                    ("SunSpreadAngle".into(), Some("$Spread".into())),
                ],
            )
            .unwrap();
        session.save(actor, None).unwrap();
        let child = std::fs::read(&fixture.child).unwrap();
        session.open(actor, &fixture.source, false).unwrap();
        let ObjectId::Entity(owner) = fixture.owner else {
            unreachable!()
        };
        session
            .set_entity_properties(
                actor,
                owner,
                &[
                    ("replace01".into(), Some("$sun 255 0 0 200".into())),
                    ("replace02".into(), Some("$sunhdr -1 -1 -1 1".into())),
                    ("replace03".into(), Some("$sky 0 0 255 40".into())),
                    ("replace10".into(), Some("$skyhdr -1 -1 -1 1".into())),
                    ("replace04".into(), Some("$sungain 0.5".into())),
                    ("replace05".into(), Some("$skygain 2".into())),
                    ("replace06".into(), Some("$spread 5".into())),
                ],
            )
            .unwrap();
        session.save(actor, None).unwrap();
        session.open(actor, &fixture.source, false).unwrap();
        let before = session.document().vmf().clone();
        let history = session.history_snapshot();
        let parent = std::fs::read(&fixture.source).unwrap();
        let report = refresh(session, actor);
        assert_eq!(report["state"], "ready", "{report}");
        assert_eq!(report["stats"]["environment_lights"], 1);
        assert!(report["stats"].get("point_lights").is_none());
        assert!(report["stats"].get("spot_lights").is_none());
        assert_eq!(report["static_light_preview"]["available"], false);
        assert_eq!(
            report["static_light_preview"]["reason"],
            ENVIRONMENT_LIGHT_PREVIEW_REASON
        );
        assert_eq!(report["available"], false);
        assert_eq!(report["geometry"]["vertices"], 0);
        assert_eq!(
            report["light_value_bindings"]["class_properties"]["light_environment"],
            json!([
                "_light",
                "_lightHDR",
                "_ambient",
                "_ambientHDR",
                "_lightscaleHDR",
                "_AmbientScaleHDR",
                "SunSpreadAngle"
            ])
        );
        assert_eq!(session.document().vmf(), &before);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), parent);
        assert_eq!(std::fs::read(&fixture.child).unwrap(), child);
        session
            .set_entity_properties(actor, owner, &[("replace06".into(), None)])
            .unwrap();
        let failed = refresh(session, actor);
        assert_eq!(failed["state"], "failed", "{failed}");
        assert!(failed["stats"].get("environment_lights").is_none());
        assert_eq!(std::fs::read(&fixture.child).unwrap(), child);
    }
}

#[test]
fn fgd_authored_environment_lights_have_sun_counts_and_no_parent_light_preview_for_both_actors() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = authored(actor, false);
        let parent = std::fs::read(&fixture.source).unwrap();
        let child = std::fs::read(&fixture.child).unwrap();
        let session = &mut fixture.session;
        let memory = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let settings = session.compile_settings();
        let report = refresh(session, actor);
        assert_eq!(report["state"], "ready", "{report}");
        assert_eq!(report["stats"]["environment_lights"], 1);
        assert!(report["stats"].get("point_lights").is_none());
        assert!(report["stats"].get("spot_lights").is_none());
        assert_eq!(report["static_light_preview"]["available"], false);
        assert_eq!(
            report["static_light_preview"]["reason"],
            ENVIRONMENT_LIGHT_PREVIEW_REASON
        );
        assert_eq!(report["available"], false);
        assert_eq!(report["displayed"], false);
        assert!(report["owners"][0]["bounds"].is_null());
        assert_eq!(report["brushes_total"], 0);
        assert_eq!(report["face_provenance_total"], 0);
        for field in [
            "vertices",
            "indices",
            "line_vertices",
            "triangles",
            "gpu_bytes",
        ] {
            assert_eq!(report["geometry"][field], 0);
        }
        assert_eq!(report["stats"]["dependencies"][0]["path"], "child.vmf");
        assert_eq!(report["stats"]["dependencies"][0]["bytes"], child.len());
        assert!(
            session
                .instance_contents_hits(
                    &Ray {
                        origin: DVec3::new(80.0, -16.0, 160.0),
                        direction: DVec3::NEG_Z
                    },
                    Some(true)
                )
                .is_empty()
        );
        assert!(
            session
                .frame_instance_contents(
                    actor,
                    report["token"].as_u64().unwrap(),
                    &[fixture.owner],
                    1.0
                )
                .unwrap_err()
                .contains("no visible geometry")
        );
        assert_eq!(session.camera(), camera);
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(session.selected_ids(), vec![fixture.owner]);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), parent);
        assert_eq!(std::fs::read(&fixture.child).unwrap(), child);
        assert_eq!(
            std::fs::read(fixture.source.with_extension("bsp")).unwrap(),
            b"previous BSP"
        );
        assert_eq!(
            std::fs::read(fixture.source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
    }
}

#[test]
fn mixed_environment_lights_preserve_physical_frame_picking_and_owned_root_drafts() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = authored(actor, true);
        let parent = std::fs::read(&fixture.source).unwrap();
        let child = std::fs::read(&fixture.child).unwrap();
        let session = &mut fixture.session;
        let solid = &session.document().vmf().world.solids[0];
        let source = FaceId {
            solid: solid.id,
            side: solid
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id,
        };
        session
            .preview_overlay(
                actor,
                crate::session::overlay::Request {
                    source,
                    options: map_core::overlay::Options {
                        material: "test/checker".into(),
                        ..Default::default()
                    },
                },
                &session.overlay_stamp(),
            )
            .unwrap();
        let draft = session.get_overlay_state();
        let memory = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let settings = session.compile_settings();
        std::fs::write(&fixture.child, &fixture.bare_child).unwrap();
        let baseline = refresh(session, actor);
        assert!(baseline["stats"].get("environment_lights").is_none());
        assert!(baseline.get("static_light_preview").is_none());
        let baseline_mesh = session.instance_contents_preview(true).mesh.unwrap();
        session
            .frame_instance_contents(
                actor,
                baseline["token"].as_u64().unwrap(),
                &[fixture.owner],
                1.5,
            )
            .unwrap();
        let framed = session.camera();
        session.set_camera(camera);
        std::fs::write(&fixture.child, &child).unwrap();
        let report = refresh(session, actor);
        assert_eq!(report["state"], "ready", "{report}");
        assert_eq!(report["stats"]["environment_lights"], 1);
        assert_eq!(
            report["static_light_preview"]["reason"],
            ENVIRONMENT_LIGHT_PREVIEW_REASON
        );
        assert_eq!(report["available"], true);
        assert_eq!(report["displayed"], true);
        for field in ["geometry", "brushes", "owners", "face_provenance"] {
            assert_eq!(report[field], baseline[field], "{field}");
        }
        let mesh = session.instance_contents_preview(true).mesh.unwrap();
        assert_eq!(mesh.vertices, baseline_mesh.vertices);
        assert_eq!(mesh.indices, baseline_mesh.indices);
        assert_eq!(mesh.edges, baseline_mesh.edges);
        let hits = session.instance_contents_hits(
            &Ray {
                origin: DVec3::new(8.0, 8.0, 160.0),
                direction: DVec3::NEG_Z,
            },
            Some(true),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].object, fixture.owner);
        assert_eq!(hits[0].point, DVec3::new(8.0, 8.0, 48.0));
        session
            .frame_instance_contents(
                actor,
                report["token"].as_u64().unwrap(),
                &[fixture.owner],
                1.5,
            )
            .unwrap();
        assert_eq!(session.camera(), framed);
        session.set_camera(camera);
        let result = session.begin_compile(actor, None).unwrap().run();
        let output = &result.as_ref().unwrap().output;
        assert!(output.bsp.is_some(), "{:?}", output.report);
        assert_eq!(
            output.stats.instances.as_ref().unwrap().environment_lights,
            1
        );
        assert_eq!(output.stats.world_lights, 0);
        session.finish_compile(actor, &result);
        assert_eq!(session.get_overlay_state(), draft);
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(session.selected_ids(), vec![fixture.owner]);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), parent);
        assert_eq!(std::fs::read(&fixture.child).unwrap(), child);
    }
}

#[test]
fn hidden_stale_cancelled_and_invalid_suns_omit_counts_and_keep_point_light_reason_unchanged() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = authored(actor, false);
        let parent = std::fs::read(&fixture.source).unwrap();
        let child = std::fs::read(&fixture.child).unwrap();
        let memory = fixture.session.document().vmf().clone();
        let history = fixture.session.history_snapshot();
        let camera = fixture.session.camera();
        let settings = fixture.session.compile_settings();
        let mut changed = Vmf::parse(&String::from_utf8(child.clone()).unwrap()).unwrap();
        changed
            .entities
            .iter_mut()
            .find(|entity| entity.id == fixture.helper)
            .unwrap()
            .hidden = true;
        std::fs::write(&fixture.child, changed.write(&Default::default())).unwrap();
        let report = refresh(&mut fixture.session, actor);
        assert_eq!(report["state"], "ready");
        assert!(report["stats"].get("environment_lights").is_none());
        assert!(report.get("static_light_preview").is_none());
        let helper = changed
            .entities
            .iter_mut()
            .find(|entity| entity.id == fixture.helper)
            .unwrap();
        helper.hidden = false;
        helper.properties.0 = vec![
            ("classname".into(), "light".into()),
            ("origin".into(), "80 -16 96".into()),
            ("_light".into(), "255 255 255 200".into()),
        ];
        std::fs::write(&fixture.child, changed.write(&Default::default())).unwrap();
        let point = refresh(&mut fixture.session, actor);
        assert_eq!(point["state"], "ready", "{point}");
        assert_eq!(point["stats"]["point_lights"], 1);
        assert!(point["stats"].get("environment_lights").is_none());
        assert_eq!(
            point["static_light_preview"]["reason"],
            "Child lights, illumination and icons are not shown in the reference or authored-lighting preview."
        );
        std::fs::write(&fixture.child, &child).unwrap();
        assert_eq!(
            refresh(&mut fixture.session, actor)["stats"]["environment_lights"],
            1
        );
        std::fs::write(&fixture.child, [child.as_slice(), b"\n"].concat()).unwrap();
        let stale = fixture.session.inspect_instance_contents(0, 64).unwrap();
        assert_eq!(stale["state"], "stale");
        assert!(stale.get("static_light_preview").is_none());
        std::fs::write(&fixture.child, &child).unwrap();
        let request = fixture
            .session
            .begin_instance_contents_refresh(actor)
            .unwrap();
        fixture
            .session
            .cancel_instance_contents_refresh(actor, request.token())
            .unwrap();
        let cancelled = fixture
            .session
            .finish_instance_contents_refresh(actor, request.run())
            .unwrap();
        assert_eq!(cancelled["state"], "cancelled");
        assert!(cancelled.get("static_light_preview").is_none());
        assert!(!fixture.session.instance_workers_busy());
        let mut invalid = Vmf::parse(&String::from_utf8(child.clone()).unwrap()).unwrap();
        invalid
            .entities
            .iter_mut()
            .find(|entity| entity.id == fixture.helper)
            .unwrap()
            .properties
            .set("_AmbientScaleHDR", "NaN");
        std::fs::write(&fixture.child, invalid.write(&Default::default())).unwrap();
        let failed = refresh(&mut fixture.session, actor);
        assert_eq!(failed["state"], "failed", "{failed}");
        assert!(failed.get("static_light_preview").is_none());
        assert!(
            fixture
                .session
                .instance_contents_preview(true)
                .mesh
                .is_none()
        );
        std::fs::write(&fixture.child, &child).unwrap();
        assert_eq!(
            refresh(&mut fixture.session, actor)["stats"]["environment_lights"],
            1
        );
        assert_eq!(fixture.session.document().vmf(), &memory);
        assert_eq!(fixture.session.history_snapshot(), history);
        assert_eq!(fixture.session.camera(), camera);
        assert_eq!(fixture.session.compile_settings(), settings);
        assert_eq!(fixture.session.selected_ids(), vec![fixture.owner]);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), parent);
        assert_eq!(std::fs::read(&fixture.child).unwrap(), child);
        assert_eq!(
            std::fs::read(fixture.source.with_extension("bsp")).unwrap(),
            b"previous BSP"
        );
        assert_eq!(
            std::fs::read(fixture.source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
    }
}
