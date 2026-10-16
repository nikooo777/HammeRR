use super::*;

#[test]
fn generated_detail_tint_world_drafts_share_validation_undo_and_saved_values() {
    let key = compiler::GENERATED_DETAIL_LIGHTING_TINT_KEY;
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("property editing needs no assets".into()));
        session.set_definitions(
            actor,
            source_formats::fgd::Catalog::parse(&format!(
                "@SolidClass = worldspawn [ {key}(color255) : \"Tint\" ]"
            ))
            .unwrap(),
        );
        let before = session.document().vmf().clone();
        let camera = session.camera();
        let history = session.history_snapshot();
        session.begin_property_edit(PropertyTarget::World).unwrap();
        let token = session.property_draft().unwrap().token;
        session
            .append_property_change(
                token,
                PropertyChange::Set {
                    key: key.into(),
                    value: "128 64 0".into(),
                },
            )
            .unwrap();
        let draft = serde_json::to_value(session.property_draft()).unwrap();
        for value in [
            "", "1 2", "1 2 3 4", "-1 0 0", "256 0 0", "1.5 0 0", "NaN 0 0",
        ] {
            let error = session
                .update_property_edit(
                    token,
                    vec![PropertyChange::Set {
                        key: key.to_ascii_uppercase(),
                        value: value.into(),
                    }],
                )
                .unwrap_err();
            assert!(error.contains(key));
            assert_eq!(
                serde_json::to_value(session.property_draft()).unwrap(),
                draft
            );
            assert_eq!(session.document().vmf(), &before);
            assert_eq!(session.history_snapshot(), history);
        }
        let preview = session.preview_property_edit().unwrap();
        let field = &preview
            .properties
            .iter()
            .find(|field| field.field.key == key)
            .unwrap()
            .field;
        assert_eq!(field.label, "Generated detail lighting tint · linear RGB");
        assert_eq!(field.kind, "string");
        assert_eq!(field.control, inspector::Control::Text);
        assert!(field.description.contains("Explicit detail entities"));
        session.cancel_property_edit(token).unwrap();
        assert_eq!(session.document().vmf(), &before);
        session.begin_property_edit(PropertyTarget::World).unwrap();
        let token = session.property_draft().unwrap().token;
        session
            .append_property_change(
                token,
                PropertyChange::Set {
                    key: key.into(),
                    value: "128 64 0".into(),
                },
            )
            .unwrap();
        session.commit_property_edit(actor, token).unwrap();
        assert_eq!(
            session.document().vmf().world.properties.get(key),
            Some("128 64 0")
        );
        assert_eq!(session.document().undo_labels().count(), 1);
        let authored = session.document().vmf().clone();
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &before);
        session.redo(actor).unwrap();
        assert_eq!(session.document().vmf(), &authored);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("generated-tint.vmf");
        session.save(actor, Some(&path)).unwrap();
        let mut reopened = Session::new(Err("property editing needs no assets".into()));
        reopened.open(actor, &path, false).unwrap();
        assert_eq!(
            reopened.document().vmf().world.properties,
            session.document().vmf().world.properties
        );
        reopened.begin_property_edit(PropertyTarget::World).unwrap();
        let token = reopened.property_draft().unwrap().token;
        reopened
            .append_property_change(token, PropertyChange::Remove { key: key.into() })
            .unwrap();
        reopened.commit_property_edit(actor, token).unwrap();
        assert!(
            reopened
                .document()
                .vmf()
                .world
                .properties
                .get(key)
                .is_none()
        );
        assert_eq!(session.camera(), camera);
    }
}
