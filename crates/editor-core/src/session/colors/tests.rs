use super::*;
use map_core::ObjectId;
use source_formats::fgd::Catalog;
use std::collections::BTreeMap;

fn fixture() -> (Session, u32) {
    let mut session = Session::new(Err("no assets".into()));
    session.set_definitions(
        Actor::User,
        Catalog::parse(
            r#"@PointClass = fixture [
        _light(color255) : "Light" : "20 40 80 450.5"
        rendercolor(color255) : "Tint" : "20 40 80 99"
        tint(color1) : "Normalized" : "0.123456789 0.25 0.75"
    ]"#,
        )
        .unwrap(),
    );
    let ObjectId::Entity(id) = session
        .create_entity(Actor::User, "fixture", None, BTreeMap::new())
        .unwrap()
    else {
        panic!()
    };
    (session, id)
}

#[test]
fn hsv_hsl_apply_preserves_brightness_alpha_precision_and_one_undo() {
    for actor in [Actor::User, Actor::Agent] {
        let (mut session, id) = fixture();
        let original = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        session
            .set_entity_color_space(
                actor,
                id,
                "_light",
                Input {
                    space: Space::Hsv,
                    channels: [240.0, 100.0, 100.0],
                },
                None,
            )
            .unwrap();
        assert_eq!(
            session
                .document()
                .entity(id)
                .unwrap()
                .properties
                .get("_light"),
            Some("0 0 255 450.5")
        );
        assert_eq!(session.document().undo_labels().count(), history + 1);
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &original);
        session
            .set_entity_color_space(
                actor,
                id,
                "rendercolor",
                Input {
                    space: Space::Hsl,
                    channels: [120.0, 100.0, 50.0],
                },
                None,
            )
            .unwrap();
        assert_eq!(
            session
                .document()
                .entity(id)
                .unwrap()
                .properties
                .get("rendercolor"),
            Some("0 255 0 99")
        );
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &original);
        let input = Input::property("color1", "0.123456789 0.25 0.75").unwrap();
        session
            .set_entity_color_space(actor, id, "tint", input, None)
            .unwrap();
        assert_eq!(
            session
                .document()
                .entity(id)
                .unwrap()
                .properties
                .get("tint"),
            Some("0.123456789 0.25 0.75")
        );
    }
}

#[test]
fn invalid_space_colors_and_brightness_preserve_source_history_and_camera() {
    let (mut session, id) = fixture();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    for (key, input, intensity) in [
        (
            "_light",
            Input {
                space: Space::Hsv,
                channels: [361.0, 50.0, 50.0],
            },
            None,
        ),
        (
            "_light",
            Input {
                space: Space::Hsl,
                channels: [240.0, f64::NAN, 50.0],
            },
            None,
        ),
        (
            "tint",
            Input {
                space: Space::Rgb1,
                channels: [0.2, 0.3, 1.1],
            },
            None,
        ),
        (
            "tint",
            Input {
                space: Space::Hsl,
                channels: [240.0, 100.0, 50.0],
            },
            Some(10.0),
        ),
        (
            "rendercolor",
            Input {
                space: Space::Hsv,
                channels: [120.0, 100.0, 100.0],
            },
            Some(10.0),
        ),
        (
            "_light",
            Input {
                space: Space::Hsv,
                channels: [120.0, 100.0, 100.0],
            },
            Some(f64::INFINITY),
        ),
    ] {
        assert!(
            session
                .set_entity_color_space(Actor::Agent, id, key, input, intensity)
                .is_err()
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
    }
}
