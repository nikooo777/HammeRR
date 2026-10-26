use super::*;
use source_formats::vmf::{Properties, Vmf};

#[test]
fn collapse_static_props_preserve_intermediate_bounds_and_refuse_unpreserved_semantics() {
    let (directory, game, parent, child) = crate::test_assets::instance_static_props::fixture();
    let bytes = std::fs::read(&child).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent, true).unwrap();
    let owner = session.document.vmf().entities.last().unwrap().id;
    let mut middle = Document::empty().into_vmf();
    let mut helper = Entity::new(10, "func_instance");
    helper.properties.set("file", "child.vmf");
    helper.properties.set("origin", "16350 0 0");
    middle.entities.push(helper);
    std::fs::write(
        directory.path().join("middle.vmf"),
        middle.write(&Default::default()),
    )
    .unwrap();
    session
        .edit(Actor::User, "intermediate prop pose", |tx| {
            tx.entity_mut(owner)?.properties.set("file", "middle.vmf");
            tx.entity_mut(owner)?.properties.set("origin", "-16350 0 0");
            Ok(())
        })
        .unwrap();
    let source = session.document.vmf().clone();
    let history = session.history_snapshot();
    let request = session
        .begin_instance_collapse(Actor::User, &session.instance_collapse_stamp())
        .unwrap();
    let error = session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap_err();
    assert!(
        error.contains("intermediate pose 1") && error.contains("bounds"),
        "{error}"
    );
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.render_document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    let token = session.get_instance_collapse()["token"].as_u64().unwrap();
    session
        .cancel_instance_collapse(Actor::User, token)
        .unwrap();
    session.undo(Actor::User).unwrap();
    let source = session.document.vmf().clone();
    let history = session.history_snapshot();
    let request = session
        .begin_instance_collapse(Actor::User, &session.instance_collapse_stamp())
        .unwrap();
    let ready = session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap();
    let shown = session.render_document().vmf().clone();
    assert_eq!(ready["stats"]["static_props"], 1);
    assert!(
        !session
            .game
            .game_dir
            .join("models/props/instance.vvd")
            .exists()
    );
    for kind in ["lightingorigin", "info_lighting", "body", "editor"] {
        let mut invalid = Vmf::parse(std::str::from_utf8(&bytes).unwrap()).unwrap();
        let expected = match kind {
            "lightingorigin" => {
                invalid.entities[0]
                    .properties
                    .set("lightingorigin", "Anchor");
                "requires empty child prop lightingorigin"
            }
            "info_lighting" => {
                let mut anchor = Entity::new(100, "info_lighting");
                anchor.properties.set("origin", "0 0 32");
                anchor.properties.set("targetname", "Anchor");
                invalid.entities.push(anchor);
                "requires its own editable semantics"
            }
            "body" => {
                invalid.entities[0].properties.set("body", "1");
                "requires body 0"
            }
            "editor" => {
                invalid.entities[0].editor =
                    Some(Properties(vec![("custom".into(), "keep".into())]));
                "editor metadata custom"
            }
            _ => unreachable!(),
        };
        std::fs::write(&child, invalid.write(&Default::default())).unwrap();
        let request = session
            .begin_instance_collapse(Actor::User, &session.instance_collapse_stamp())
            .unwrap();
        let error = session
            .finish_instance_collapse(Actor::User, request.run())
            .unwrap_err();
        assert!(error.contains(expected), "{kind}: {error}");
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.render_document().vmf(), &shown);
        assert_eq!(session.history_snapshot(), history);
    }
    std::fs::write(&child, &bytes).unwrap();
    let token = session.get_instance_collapse()["token"].as_u64().unwrap();
    session
        .cancel_instance_collapse(Actor::User, token)
        .unwrap();
    let request = session
        .begin_instance_collapse(Actor::User, &session.instance_collapse_stamp())
        .unwrap();
    session
        .cancel_instance_collapse(Actor::User, request.token())
        .unwrap();
    assert!(
        session
            .finish_instance_collapse(Actor::User, request.run())
            .is_err()
    );
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.render_document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(std::fs::read(child).unwrap(), bytes);
    let mut unmounted = Session::new(Err("No mounted game".into()));
    unmounted.open(Actor::User, &parent, true).unwrap();
    let request = unmounted
        .begin_instance_collapse(Actor::User, &unmounted.instance_collapse_stamp())
        .unwrap();
    let error = unmounted
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap_err();
    assert!(
        error.contains("model models/props/instance.mdl was not found"),
        "{error}"
    );
    assert_eq!(unmounted.document.vmf(), &source);
}
