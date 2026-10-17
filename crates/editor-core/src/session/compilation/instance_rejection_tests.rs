use super::*;
use source_formats::{keyvalues::Node, vmf::Entity};

#[test]
fn instance_failures_preserve_shared_session_and_regular_or_guarded_outputs() {
    for (actor, classname, region) in [
        (Actor::User, "FuNc_InStAnCe", false),
        (Actor::Agent, "FUNC_INSTANCE_PARMS", true),
    ] {
        let (directory, game) = crate::test_assets::game();
        let source_path = directory.path().join("instances.vmf");
        let mut vmf = crate::test_assets::room();
        let id = vmf.object_ids().chain(vmf.side_ids()).max().unwrap() + 1;
        let mut entity = Entity::new(id, classname);
        entity.properties.set("file", "unresolved.vmf");
        entity.properties.set("origin", "4096 0 64");
        vmf.entities.push(entity);
        let mut hidden = Entity::new(id + 1, "func_instance_parms");
        hidden.hidden = true;
        vmf.entities.push(hidden);
        vmf.entities.push(Entity::new(id + 2, "func_viscluster"));
        if region {
            vmf.cordon = Some(Node::block(
                "cordon",
                vec![
                    Node::string("active", "1"),
                    Node::string("mins", "-128 -128 0"),
                    Node::string("maxs", "128 128 256"),
                ],
            ));
        }
        let source_bytes = vmf.write(&Default::default());
        std::fs::write(&source_path, &source_bytes).unwrap();
        let bsp = source_path.with_extension("bsp");
        let lin = source_path.with_extension("lin");
        std::fs::write(&bsp, b"previous BSP").unwrap();
        std::fs::write(&lin, b"previous pointfile").unwrap();
        let mut session = Session::new(Ok(game));
        session.open(actor, &source_path, true).unwrap();
        session
            .create_block(
                actor,
                DVec3::splat(16.0),
                DVec3::splat(32.0),
                "test/checker",
                false,
            )
            .unwrap();
        session
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    pack_sky_assets: Some(region),
                    ..Default::default()
                },
            )
            .unwrap();
        let source = session.document().vmf().clone();
        let history = session.history_snapshot();
        let undo = session
            .document()
            .undo_labels()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let selection = session.selected_ids();
        let camera = session.camera();
        session.run_map_checks(actor);
        let issues = session
            .map_checks_json(Some("error"), "instance expansion", 0, 50)
            .unwrap();
        assert_eq!(issues["matching"], 1);
        assert_eq!(issues["issues"][0]["code"], "compiler_entity");
        assert_eq!(issues["issues"][0]["object"], format!("entity:{id}"));
        let warnings = session
            .map_checks_json(Some("warning"), "native compiler currently omits", 0, 50)
            .unwrap();
        assert_eq!(warnings["matching"], 1);
        assert_eq!(
            warnings["issues"][0]["object"],
            format!("entity:{}", id + 2)
        );
        let result = session.begin_compile(actor, None).unwrap().run();
        let output = &result.as_ref().unwrap().output;
        assert!(output.bsp.is_none());
        assert!(output.leak.is_none());
        assert_eq!(output.stats.brushes, 0);
        session.finish_compile(actor, &result);
        let status = session.compile_status();
        assert_eq!(status["job"]["status"], "failed");
        assert_eq!(status["job"]["result"]["success"], false);
        assert_eq!(status["job"]["result"]["published"], false);
        assert_eq!(status["job"]["result"]["diagnostics"][0]["level"], "error");
        assert_eq!(
            status["job"]["result"]["diagnostics"][0]["object"],
            format!("entity:{id}")
        );
        assert_eq!(status["launch"]["ready"], false);
        assert!(!session.compiling());
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.render_document().vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(
            session
                .document()
                .undo_labels()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            undo
        );
        assert_eq!(session.selected_ids(), selection);
        assert_eq!(session.camera(), camera);
        assert_eq!(std::fs::read_to_string(&source_path).unwrap(), source_bytes);
        assert_eq!(std::fs::read(&bsp).unwrap(), b"previous BSP");
        assert_eq!(std::fs::read(&lin).unwrap(), b"previous pointfile");
    }
}
