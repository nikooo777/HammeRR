use super::*;
use source_formats::fgd::Catalog;
use std::collections::BTreeMap;

#[test]
#[ignore = "requires installed CS:S FGD chain"]
fn installed_fgd_brush_models_keep_shared_authoring_source_identity_and_honest_preview() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, game, source, child) = crate::test_assets::instance_map(false);
        let fgd = crate::steam::find_css_fgd(&game).expect("installed CS:S FGD chain is required");
        let definitions = Catalog::load(&fgd).unwrap();
        let class = definitions.class("func_brush").unwrap();
        assert_eq!(class.property("origin").unwrap().label, "Origin (X Y Z)");
        assert!(class.property("origin").unwrap().default.is_none());
        assert!(!class.property("origin").unwrap().read_only);
        assert!(class.property("angles").is_none());
        let defaults = class.defaults();
        for (key, value) in [
            ("spawnflags", "2"),
            ("Solidity", "0"),
            ("StartDisabled", "0"),
            ("solidbsp", "0"),
            ("invert_exclusion", "0"),
            ("vrad_brush_cast_shadows", "0"),
            ("InputFilter", "0"),
            ("renderfx", "0"),
            ("rendermode", "0"),
            ("disableshadows", "0"),
            ("disablereceiveshadows", "0"),
            ("renderamt", "255"),
            ("rendercolor", "255 255 255"),
            ("globalname", ""),
            ("excludednpc", ""),
        ] {
            assert_eq!(defaults.get(key).map(String::as_str), Some(value));
        }
        std::fs::write(&child, Document::empty().vmf().write(&Default::default())).unwrap();
        let mut session = Session::new(Ok(game));
        session.set_definitions(actor, definitions);
        session.open(actor, &child, true).unwrap();
        let mut blocks = Vec::new();
        for (mins, maxs) in [
            ([-48.0, -32.0, 16.0], [48.0, 32.0, 24.0]),
            ([-8.0, -32.0, 32.0], [8.0, 32.0, 40.0]),
        ] {
            blocks.push(
                session
                    .create_block(actor, mins.into(), maxs.into(), "test/checker", false)
                    .unwrap(),
            );
        }
        let ObjectId::Entity(model) = session
            .tie_to_entity(actor, &blocks, "func_brush", BTreeMap::new())
            .unwrap()
        else {
            unreachable!()
        };
        let authored = session.document().entity(model).unwrap();
        assert_eq!(authored.solids.len(), 2);
        assert!(authored.properties.get("origin").is_none());
        assert!(authored.properties.get("angles").is_none());
        for (key, value) in &defaults {
            assert_eq!(authored.properties.get(key), Some(value.as_str()));
        }
        session
            .set_entity_properties(actor, model, &[("origin".into(), Some("16 8 4".into()))])
            .unwrap();
        session.save(actor, None).unwrap();
        let child_bytes = std::fs::read(&child).unwrap();
        session.open(actor, &child, false).unwrap();
        assert_eq!(
            session
                .document()
                .entity(model)
                .unwrap()
                .properties
                .get("origin"),
            Some("16 8 4")
        );
        assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
        session.open(actor, &source, false).unwrap();
        let owner = ObjectId::Entity(session.document().vmf().entities.last().unwrap().id);
        session.select(&[owner], SelectMode::Replace).unwrap();
        session.set_view(render::ViewOptions {
            instance_contents: true,
            ..session.view()
        });
        let memory = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let settings = session.compile_settings();
        let source_bytes = std::fs::read(&source).unwrap();
        let report = refresh(&mut session, actor);
        assert_eq!(report["state"], "ready", "{report}");
        assert_eq!(report["stats"]["brush_models"], 1);
        assert_eq!(report["stats"]["world_brushes"], 0);
        assert_eq!(report["stats"]["detail_brushes"], 0);
        assert_eq!(
            report["brush_model_preview"],
            json!({"available":false,"reason":BRUSH_MODEL_PREVIEW_REASON})
        );
        assert_eq!(report["available"], false);
        assert_eq!(report["displayed"], false);
        assert!(report["owners"][0]["bounds"].is_null());
        assert_eq!(report["brushes_total"], 0);
        assert_eq!(report["face_provenance_total"], 0);
        assert_eq!(report["stats"]["dependencies"][0]["path"], "child.vmf");
        assert_eq!(
            report["stats"]["dependencies"][0]["bytes"],
            child_bytes.len()
        );
        let mesh = session.instance_contents_preview(true).mesh.unwrap();
        assert!(
            mesh.vertices.is_empty()
                && mesh.indices.is_empty()
                && mesh.edges.is_empty()
                && mesh.brushes.is_empty()
        );
        assert!(
            session
                .frame_instance_contents(actor, report["token"].as_u64().unwrap(), &[owner], 1.0)
                .unwrap_err()
                .contains("no visible geometry")
        );
        std::fs::write(&child, [child_bytes.as_slice(), b"\n"].concat()).unwrap();
        let stale = session.inspect_instance_contents(0, 64).unwrap();
        assert_eq!(stale["state"], "stale");
        assert!(stale.get("brush_model_preview").is_none());
        assert!(session.instance_contents_preview(true).mesh.is_none());
        std::fs::write(&child, &child_bytes).unwrap();
        assert_eq!(refresh(&mut session, actor)["stats"]["brush_models"], 1);
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(session.selected_ids(), vec![owner]);
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        assert_eq!(std::fs::read(&child).unwrap(), child_bytes);

        session.open(actor, &child, false).unwrap();
        session
            .create_block(
                actor,
                DVec3::new(96.0, 0.0, 16.0),
                DVec3::new(112.0, 16.0, 32.0),
                "test/checker",
                false,
            )
            .unwrap();
        let detail = session
            .create_block(
                actor,
                DVec3::new(128.0, 0.0, 16.0),
                DVec3::new(144.0, 16.0, 32.0),
                "test/checker",
                false,
            )
            .unwrap();
        session
            .tie_to_entity(actor, &[detail], "func_detail", BTreeMap::new())
            .unwrap();
        session.save(actor, None).unwrap();
        session.open(actor, &source, false).unwrap();
        let mixed = refresh(&mut session, actor);
        assert_eq!(mixed["stats"]["brush_models"], 1);
        assert_eq!(mixed["stats"]["world_brushes"], 1);
        assert_eq!(mixed["stats"]["detail_brushes"], 1);
        assert_eq!(mixed["brushes_total"], 2);
        assert_eq!(mixed["face_provenance_total"], 12);
        assert_eq!(
            mixed["owners"][0]["bounds"],
            json!([[96.0, 0.0, 16.0], [144.0, 16.0, 32.0]])
        );
        assert_eq!(mixed["available"], true);
    }
}
