use super::*;
use crate::session::{Actor, compilation::BuildSettingsUpdate};

fn compiled(dual: bool) -> (tempfile::TempDir, Session, std::path::PathBuf) {
    compiled_with(dual, 0)
}

fn compiled_with(dual: bool, bounces: u8) -> (tempfile::TempDir, Session, std::path::PathBuf) {
    compiled_options(dual, bounces, true)
}

fn compiled_options(
    dual: bool,
    bounces: u8,
    bake: bool,
) -> (tempfile::TempDir, Session, std::path::PathBuf) {
    let (directory, game, vmf) = test_support::fixture();
    let source = directory.path().join("maps/source.vmf");
    std::fs::write(&source, vmf.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source, false).unwrap();
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                lighting: Some(if dual { "direct_ldr_hdr" } else { "direct_ldr" }),
                physics: Some("none"),
                bounces: Some(bounces),
                bake_static_prop_vertices: Some(bake),
                pack_sky_assets: Some(false),
                generate_reflections: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    session.finish_compile(Actor::User, &result);
    assert!(
        result.as_ref().unwrap().published(),
        "{:?}",
        session.compile_status()
    );
    (directory, session, source)
}

fn token(report: &Value) -> String {
    report["build"]["token"].as_str().unwrap().to_owned()
}

#[test]
fn actual_publication_pages_native_holes_all_lods_duplicates_and_exact_bytes() {
    let (_directory, mut session, _source) = compiled(true);
    let vmf = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let first = session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report;
    assert_eq!(first["build"]["status"], "current", "{first:#}");
    assert_eq!(first["bake"]["source_closure"], "verified");
    assert_eq!(first["props"]["total"], 3);
    assert_eq!(first["props"]["rows"][0]["status"], "disabled_by_prop");
    assert_eq!(first["props"]["rows"][1]["ordinal"], 1);
    assert_eq!(first["source_associations"]["status"], "available");
    for (ordinal, entity) in [9100, 9101, 9102].into_iter().enumerate() {
        assert_eq!(
            first["props"]["rows"][ordinal]["source_association"]["origin"],
            json!({"kind":"root","entity":entity})
        );
    }
    assert_eq!(
        first["selected_prop"]["source_association"]["origin"],
        json!({"kind":"root","entity":9101})
    );
    assert_eq!(first["selected"]["ordinal"], 1);
    assert_eq!(first["selected"]["vertex_count"], 306);
    assert_eq!(first["sections"]["rows"][0]["vertex_count"], 300);
    assert_eq!(first["sections"]["rows"][1]["lod"], 1);
    assert_eq!(first["vertices"]["returned"], 256);
    assert_eq!(first["vertices"]["next_offset"], 256);
    assert_eq!(
        first["vertices"]["rows"][0]["bgra8"],
        first["vertices"]["rows"][2]["bgra8"]
    );
    assert_eq!(first["statistics"]["scope"], "returned_vertex_page");
    assert_eq!(first["statistics"]["count"], 256);
    let build = session.compiled.as_ref().unwrap();
    let bsp = Bsp::parse(&std::fs::read(&build.path).unwrap()).unwrap();
    let archive = bsp.pak_data(PakLimits::default()).unwrap();
    let raw = &archive
        .entries
        .iter()
        .find(|entry| entry.name == "sp_1.vhv")
        .unwrap()
        .data;
    let joined = session
        .inspect_baked_prop_vertices(&Query {
            build_token: Some(token(&first)),
            prop: Some(1),
            vertex_offset: 255,
            vertex_limit: 2,
            ..Query::default()
        })
        .unwrap()
        .report;
    for offset in 0..2 {
        let bytes = &raw[512 + (255 + offset) * 4..512 + (256 + offset) * 4];
        assert_eq!(joined["vertices"]["rows"][offset]["bgra8"], json!(bytes));
        assert_eq!(
            joined["vertices"]["rows"][offset]["hardware_index"],
            255 + offset
        );
    }
    let lower = session
        .inspect_baked_prop_vertices(&Query {
            build_token: Some(token(&first)),
            prop: Some(1),
            section: Some(1),
            ..Query::default()
        })
        .unwrap()
        .report;
    assert_eq!(lower["vertices"]["lod"], 1);
    assert_eq!(lower["vertices"]["total"], 6);
    assert_eq!(lower["vertices"]["rows"][0]["hardware_index"], 300);
    assert_eq!(
        lower["vertices"]["rows"][0]["bgra8"],
        json!(&raw[1712..1716])
    );
    let hdr = session
        .inspect_baked_prop_vertices(&Query {
            build_token: Some(token(&first)),
            channel: Channel::Hdr,
            prop: Some(1),
            ..Query::default()
        })
        .unwrap()
        .report;
    assert_ne!(
        first["vertices"]["rows"][0]["bgra8"],
        hdr["vertices"]["rows"][0]["bgra8"]
    );
    let end = session
        .inspect_baked_prop_vertices(&Query {
            build_token: Some(token(&first)),
            vertex_offset: 300,
            prop_offset: 3,
            section_offset: 2,
            ..Query::default()
        })
        .unwrap()
        .report;
    for key in ["props", "sections", "vertices"] {
        assert_eq!(end[key]["returned"], 0);
        assert!(end[key]["next_offset"].is_null());
    }
    assert!(end["statistics"].is_null());
    assert_eq!(session.document().vmf(), &vmf);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    session.compiled.as_mut().unwrap().static_prop_origins =
        Err("Captured source metadata is unavailable".into());
    let missing = session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report;
    assert_eq!(missing["source_associations"]["status"], "unavailable");
    assert_eq!(missing["bake"]["status"], "available");
    assert_eq!(missing["vertices"]["rows"], first["vertices"]["rows"]);
}

#[test]
fn repeated_nested_prop_addresses_survive_child_edits_without_changing_native_bytes() {
    use source_formats::vmf::Entity;

    let (directory, game, mut vmf) = test_support::fixture();
    let mut normal = vmf
        .entities
        .iter()
        .find(|entity| entity.id == 9101)
        .unwrap()
        .clone();
    normal.id = 2;
    let mut disabled = vmf
        .entities
        .iter()
        .find(|entity| entity.id == 9100)
        .unwrap()
        .clone();
    disabled.id = 3;
    disabled.properties.set("origin", "0 0 96");
    let mut leaf = map_core::Document::empty().into_vmf();
    leaf.entities = vec![normal, disabled];
    let helper = |id, file: &str, origin: &str| {
        let mut helper = Entity::new(id, "func_instance");
        helper.properties.set("file", file);
        helper.properties.set("origin", origin);
        helper
    };
    let mut pair = map_core::Document::empty().into_vmf();
    pair.entities = vec![
        helper(2, "leaf.vmf", "-64 0 0"),
        helper(3, "leaf.vmf", "64 0 0"),
    ];
    vmf.entities
        .retain(|entity| entity.classname() != Some("prop_static") || entity.id == 9101);
    vmf.entities.extend([
        helper(9200, "pair.vmf", "0 -64 0"),
        helper(9201, "pair.vmf", "0 64 0"),
    ]);
    let source = directory.path().join("maps/source.vmf");
    let leaf_path = directory.path().join("maps/leaf.vmf");
    let pair_path = directory.path().join("maps/pair.vmf");
    for (path, map) in [(&source, &vmf), (&leaf_path, &leaf), (&pair_path, &pair)] {
        std::fs::write(path, map.write(&Default::default())).unwrap();
    }
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source, false).unwrap();
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                lighting: Some("direct_ldr_hdr"),
                physics: Some("none"),
                bake_static_prop_vertices: Some(true),
                pack_sky_assets: Some(false),
                generate_reflections: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    session.finish_compile(Actor::User, &result);
    assert!(
        result.as_ref().unwrap().published(),
        "{:?}",
        session.compile_status()
    );
    let output = session.compiled.as_ref().unwrap().path.clone();
    let native_before = std::fs::read(&output).unwrap();
    let before = session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report;
    assert_eq!(before["props"]["total"], 9);
    assert_eq!(before["bake"]["baked_receivers"], 5);
    assert_eq!(
        before["props"]["rows"][0]["source_association"]["origin"],
        json!({"kind":"root","entity":9101})
    );
    let bsp = Bsp::parse(&native_before).unwrap();
    let props = bsp.static_props().unwrap().unwrap();
    for (index, (outer, inner, x, y)) in [
        (9200, 2, -64., -64.),
        (9200, 3, 64., -64.),
        (9201, 2, -64., 64.),
        (9201, 3, 64., 64.),
    ]
    .into_iter()
    .enumerate()
    {
        for (offset, entity, z) in [(0, 2, 32.), (1, 3, 96.)] {
            let ordinal = 1 + index * 2 + offset;
            let row = &before["props"]["rows"][ordinal];
            let origin = &row["source_association"]["origin"];
            assert_eq!(origin["kind"], "instance");
            assert_eq!(origin["instance"], outer);
            assert_eq!(origin["file"], "leaf.vmf");
            assert_eq!(origin["entity"], entity);
            assert_eq!(
                row["source_association"]["label"],
                format!("entity:{outer} → pair.vmf / entity:{inner} → leaf.vmf / entity:{entity}")
            );
            assert_eq!(
                origin["occurrence"],
                json!([{"helper":outer,"file":"pair.vmf"},{"helper":inner,"file":"leaf.vmf"}])
            );
            assert_eq!(props.props[ordinal].origin, [x, y, z]);
            assert_eq!(row["origin"], json!(props.props[ordinal].origin));
            assert_eq!(
                row["status"],
                if entity == 3 {
                    "disabled_by_prop"
                } else {
                    "baked"
                }
            );
        }
    }
    let query = Query {
        build_token: Some(token(&before)),
        prop: Some(7),
        channel: Channel::Hdr,
        ..Query::default()
    };
    let selected = session.inspect_baked_prop_vertices(&query).unwrap().report;
    assert_eq!(
        selected["selected_prop"]["source_association"],
        before["props"]["rows"][7]["source_association"]
    );
    assert_eq!(selected["selected"]["ordinal"], 7);
    let source_camera = session.camera();
    let source_map = session.document().vmf().clone();
    let source_history = session.history_snapshot();
    let face = map_core::surface::FaceId {
        solid: source_map.world.solids[0].id,
        side: source_map.world.solids[0].sides[0].id,
    };

    for (actor, ordinal, expected) in [
        (Actor::User, 0, 9101),
        (Actor::Agent, 1, 9200),
        (Actor::User, 4, 9200),
        (Actor::Agent, 7, 9201),
        (Actor::User, 8, 9201),
    ] {
        session
            .select_faces(&[face], crate::session::SelectMode::Replace)
            .unwrap();
        assert!(!session.selected_faces().is_empty());
        let result = session
            .select_compiled_prop_source(actor, token(&before), ordinal)
            .unwrap();
        assert_eq!(result["target"], format!("entity:{expected}"));
        assert_eq!(
            session.selected_ids(),
            [map_core::ObjectId::Entity(expected)]
        );
        assert!(!session.face_mode());
        assert!(session.selected_faces().is_empty());
        assert_eq!(session.camera(), source_camera);
        assert_eq!(session.document().vmf(), &source_map);
        assert_eq!(session.history_snapshot(), source_history);
    }
    let request = session
        .begin_instance_collapse(Actor::User, &session.instance_collapse_stamp())
        .unwrap();
    let collapse = session.get_instance_collapse();
    let selection = session.selected_ids();
    assert!(
        session
            .select_compiled_prop_source(Actor::Agent, token(&before), 7)
            .is_err()
    );
    assert_eq!(session.get_instance_collapse(), collapse);
    assert_eq!(session.selected_ids(), selection);
    session
        .cancel_instance_collapse(Actor::User, request.token())
        .unwrap();
    drop(request);
    leaf.entities[0].id = 44;
    std::fs::write(&leaf_path, leaf.write(&Default::default())).unwrap();
    let historical = session.inspect_baked_prop_vertices(&query).unwrap().report;
    assert_eq!(historical["historical"], true);
    assert_eq!(historical["selected_prop"], selected["selected_prop"]);
    let selection = session.selected_ids();
    assert!(
        session
            .select_compiled_prop_source(Actor::Agent, token(&before), 7)
            .is_err()
    );
    assert_eq!(session.selected_ids(), selection);
    assert_eq!(session.camera(), source_camera);

    assert_eq!(historical["props"]["rows"], before["props"]["rows"]);
    assert_eq!(std::fs::read(&output).unwrap(), native_before);
}

#[test]
fn retained_token_ranges_and_missing_hdr_never_select_another_file() {
    let (_directory, session, _source) = compiled(false);
    let initial = session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report;
    let captured = token(&initial);
    for query in [
        Query {
            prop: Some(1),
            ..Query::default()
        },
        Query {
            build_token: Some("wrong".into()),
            ..Query::default()
        },
        Query {
            build_token: Some(captured.clone()),
            prop: Some(3),
            ..Query::default()
        },
        Query {
            build_token: Some(captured.clone()),
            section: Some(2),
            ..Query::default()
        },
        Query {
            build_token: Some(captured.clone()),
            vertex_offset: 301,
            ..Query::default()
        },
        Query {
            vertex_limit: 257,
            ..Query::default()
        },
        Query {
            prop_offset: usize::MAX,
            ..Query::default()
        },
    ] {
        assert!(session.inspect_baked_prop_vertices(&query).is_err());
    }
    let missing = session
        .inspect_baked_prop_vertices(&Query {
            channel: Channel::Hdr,
            ..Query::default()
        })
        .unwrap()
        .report;
    assert_eq!(missing["channels"][1]["available"], false);
    assert!(missing["selected"].is_null());
    assert_eq!(missing["vertices"]["returned"], 0);
    let disabled = session
        .inspect_baked_prop_vertices(&Query {
            build_token: Some(captured),
            prop: Some(0),
            ..Query::default()
        })
        .unwrap()
        .report;
    assert!(disabled["selected"].is_null());
    assert_eq!(disabled["selected_prop"]["ordinal"], 0);
    assert_eq!(
        disabled["selected_prop"]["source_association"]["origin"],
        json!({"kind":"root","entity":9100})
    );
    assert_eq!(disabled["props"]["rows"][0]["status"], "disabled_by_prop");
}

#[test]
fn no_owner_and_a_successful_non_baked_owner_have_explicit_empty_states() {
    let session = Session::new(Err("No mounted assets needed".into()));
    let unavailable = session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report;
    assert_eq!(unavailable["build"]["status"], "unavailable");
    assert!(unavailable["selected"].is_null());
    let (_directory, mut session, _source) = compiled_options(false, 0, false);
    let report = session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report;
    assert_eq!(report["build"]["status"], "current");
    assert_eq!(report["bake"]["status"], "not_baked");
    assert_eq!(report["bake"]["source_closure"], "not_applicable");
    assert_eq!(report["props"]["total"], 3);
    assert_eq!(report["props"]["rows"][1]["status"], "not_baked");
    assert!(report["selected"].is_null());
    assert_eq!(report["vertices"]["returned"], 0);
    assert_eq!(report["source_associations"]["status"], "available");
    assert_eq!(report["selected_prop"]["ordinal"], 0);
    let camera = session.camera();
    let framed = session
        .frame_compiled_prop_origin(Actor::User, token(&report), 0, 1.)
        .unwrap();
    assert_eq!(
        framed.inspection.report["props"]["rows"][0]["status"],
        "disabled_by_prop"
    );
    session
        .frame_compiled_prop_origin(Actor::Agent, token(&report), 1, 1.)
        .unwrap();
    session.set_camera(camera);
    for (actor, ordinal, entity) in [(Actor::User, 0, 9100), (Actor::Agent, 1, 9101)] {
        session
            .select_compiled_prop_source(actor, token(&report), ordinal)
            .unwrap();
        assert_eq!(session.selected_ids(), [map_core::ObjectId::Entity(entity)]);
        assert_eq!(session.camera(), camera);
    }

    assert!(
        report["channels"]
            .as_array()
            .unwrap()
            .iter()
            .all(|channel| channel["available"] == false)
    );
    session.compiled.as_mut().unwrap().static_prop_origins =
        Err("Captured source metadata exceeds its storage budget".into());
    let missing = session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report;
    assert_eq!(missing["build"]["status"], "current");
    assert_eq!(missing["props"]["total"], 3);
    assert_eq!(missing["source_associations"]["status"], "unavailable");
    assert_eq!(
        missing["selected_prop"]["source_association"]["status"],
        "unavailable"
    );
    assert_eq!(
        missing["props"]["rows"][1]["origin"],
        report["props"]["rows"][1]["origin"]
    );
}

#[test]
fn changed_sources_remain_historical_raw_while_replaced_output_clears_every_page() {
    let (directory, mut session, source) = compiled(true);
    let first = session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report;
    let query = Query {
        build_token: Some(token(&first)),
        ..Query::default()
    };
    let material = directory.path().join("materials/models/test/one.vmt");
    let original = std::fs::read(&material).unwrap();
    let mut changed = original.clone();
    changed[0] = b'!';
    std::fs::write(&material, &changed).unwrap();
    let stale = session.inspect_baked_prop_vertices(&query).unwrap().report;
    assert_eq!(stale["build"]["status"], "stale");
    assert_eq!(stale["historical"], true);
    assert_eq!(
        stale["build"]["render_context"]["current_for_display"],
        false
    );
    assert_eq!(stale["bake"]["status"], "available");
    assert_eq!(stale["bake"]["source_closure"], "changed");
    assert_eq!(stale["vertices"]["rows"], first["vertices"]["rows"]);
    assert_eq!(stale["selected_prop"], first["selected_prop"]);
    assert_eq!(stale["props"]["rows"], first["props"]["rows"]);
    session.compiled.as_mut().unwrap().prop_lighting_guard = None;
    let unguarded = session.inspect_baked_prop_vertices(&query).unwrap().report;
    assert_eq!(unguarded["bake"]["source_closure"], "unverifiable");
    assert_eq!(unguarded["build"]["status"], "stale");
    assert!(
        session
            .compiled
            .as_ref()
            .unwrap()
            .prop_lighting_guard
            .is_none()
    );
    let mut vmf = std::fs::read(&source).unwrap();
    vmf[0] = b'!';
    std::fs::write(&source, vmf).unwrap();
    assert_eq!(
        session.inspect_baked_prop_vertices(&query).unwrap().report["historical"],
        true
    );
    let output = session.compiled.as_ref().unwrap().path.clone();
    let mut bytes = std::fs::read(&output).unwrap();
    let end = bytes.len() - 1;
    bytes[end] ^= 1;
    std::fs::write(&output, bytes).unwrap();
    let unavailable = session.inspect_baked_prop_vertices(&query).unwrap().report;
    assert_eq!(unavailable["build"]["status"], "unavailable");
    assert_eq!(unavailable["bake"]["status"], "unavailable");
    assert!(unavailable["selected"].is_null());
    assert!(unavailable["selected_prop"].is_null());
    assert_eq!(unavailable["source_associations"]["status"], "unavailable");
    assert!(unavailable["statistics"].is_null());
    for key in ["props", "sections", "vertices"] {
        assert!(unavailable[key]["rows"].as_array().unwrap().is_empty());
    }
}

#[test]
fn complete_archive_validation_precedes_any_selected_page() {
    let (_directory, session, _source) = compiled(true);
    let bytes = std::fs::read(&session.compiled.as_ref().unwrap().path).unwrap();
    let bsp = Bsp::parse(&bytes).unwrap();
    for name in [
        "sp_1.vhv",
        "sp_hdr_2.vhv",
        prop_lighting_assets::MANIFEST_PATH,
    ] {
        let mut changed = bsp.clone();
        let mut archive = changed.pak_data(PakLimits::default()).unwrap();
        archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == name)
            .unwrap()
            .data[0] ^= 1;
        changed
            .set_pak_data(&archive, PakLimits::default())
            .unwrap();
        assert!(
            prop_lighting_assets::read_validated_prop_lighting(
                &changed,
                &compiler::Control::default()
            )
            .is_err()
        );
    }
    let mut changed = bsp.clone();
    changed.lumps[source_formats::bsp::lump::MAP_FLAGS].data = 0u32.to_le_bytes().to_vec();
    assert!(
        prop_lighting_assets::read_validated_prop_lighting(&changed, &compiler::Control::default())
            .is_err()
    );
    let mut duplicate = bsp.clone();
    let sprp = duplicate
        .game_lumps
        .iter()
        .find(|lump| lump.id == *b"sprp")
        .unwrap()
        .clone();
    duplicate.game_lumps.push(sprp);
    assert!(
        prop_lighting_assets::read_validated_prop_lighting(
            &duplicate,
            &compiler::Control::default()
        )
        .is_err()
    );
}

#[test]
fn diagnostics_serialization_and_decoded_allocation_limits_are_explicit() {
    let origins = [compiler::entity_origins::Origin::Root { entity: 42 }];
    let associations = Ok(origins.as_slice());
    let mut source_bytes = MAX_PAGE_SOURCE_BYTES - origins[0].response_copy_bytes();
    assert!(source_association(&associations, 0, &mut source_bytes).is_ok());
    assert_eq!(source_bytes, MAX_PAGE_SOURCE_BYTES);
    assert!(source_association(&associations, 0, &mut source_bytes).is_err());
    let mut report = empty_report(&Query::default(), json!({"diagnostics":[]}));
    report["diagnostics"] = json!(vec!["π".repeat(600); 100]);
    bound_diagnostics(&mut report);
    let report = finish(report).unwrap().report;
    assert_eq!(report["diagnostics_total"], 100);
    assert_eq!(report["diagnostics_truncated"], true);
    assert_eq!(report["diagnostics"].as_array().unwrap().len(), 64);
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .all(|value| value.as_str().unwrap().len() <= 512)
    );
    let mut nested =
        json!({"build":{"diagnostics":vec!["build";40]},"diagnostics":vec!["page";40]});
    bound_diagnostics(&mut nested);
    assert_eq!(
        nested["build"]["diagnostics"].as_array().unwrap().len()
            + nested["diagnostics"].as_array().unwrap().len(),
        64
    );
    nested["diagnostics"]
        .as_array_mut()
        .unwrap()
        .push(json!("new"));
    bound_diagnostics(&mut nested);
    assert_eq!(nested["diagnostics_total"], 41);
    let mut huge = report;
    huge["oversized"] = json!("x".repeat(MAX_REPORT_BYTES));
    assert!(finish(huge).is_err());
    let mut overlapping = vec![0u8; 1036 + 3 * 1024 * 1024];
    overlapping[..4].copy_from_slice(b"VBSP");
    for index in 0..64 {
        overlapping[8 + index * 16..12 + index * 16].copy_from_slice(&1036i32.to_le_bytes());
        overlapping[12 + index * 16..16 + index * 16]
            .copy_from_slice(&(3 * 1024i32 * 1024).to_le_bytes());
    }
    assert!(baked_build::decoded_bsp_bound(&overlapping).is_err());
    let control = compiler::Control::default();
    control.cancel().unwrap();
    let session = Session::new(Err("No mounted assets needed".into()));
    assert!(
        session
            .inspect_baked_prop_vertices_controlled(&Query::default(), &control)
            .is_err()
    );
}

#[test]
fn a_failed_lookup_candidate_appearing_invalidates_the_original_observation() {
    let (directory, session, _source) = compiled(true);
    let before = session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report;
    let bytes = std::fs::read(&session.compiled.as_ref().unwrap().path).unwrap();
    let bsp = Bsp::parse(&bytes).unwrap();
    let bundle =
        prop_lighting_assets::read_validated_prop_lighting(&bsp, &compiler::Control::default())
            .unwrap();
    let missing = bundle
        .manifest()
        .unwrap()
        .missing_sources
        .first()
        .expect("Fixture retains failed material candidates");
    let path = directory.path().join(missing);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"VertexLitGeneric {}").unwrap();
    let after = session
        .inspect_baked_prop_vertices(&Query {
            build_token: Some(token(&before)),
            ..Query::default()
        })
        .unwrap()
        .report;
    assert_eq!(after["bake"]["source_closure"], "changed");
    assert_eq!(after["historical"], true);
    assert_eq!(after["vertices"]["rows"], before["vertices"]["rows"]);
}

#[test]
fn one_bounce_world_vtf_mutation_keeps_old_rows_and_original_captured_settings() {
    let (directory, mut session, _source) = compiled_with(true, 1);
    let before = session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report;
    assert_eq!(before["build"]["bounces"], 1);
    let path = directory.path().join("materials/test/checker.vtf");
    let mut bytes = std::fs::read(&path).unwrap();
    let length = bytes.len();
    bytes[32..36].copy_from_slice(&0.5f32.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(std::fs::metadata(path).unwrap().len() as usize, length);
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                bounces: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
    let after = session
        .inspect_baked_prop_vertices(&Query {
            build_token: Some(token(&before)),
            ..Query::default()
        })
        .unwrap()
        .report;
    assert_eq!(after["build"]["bounces"], 1);
    assert_eq!(after["bake"]["source_closure"], "changed");
    assert_eq!(after["historical"], true);
    assert_eq!(after["vertices"]["rows"], before["vertices"]["rows"]);
}

#[test]
fn compiled_prop_origin_framing_preserves_projection_native_identity_and_owned_state() {
    let (_directory, mut session, source) = compiled(false);
    let report = session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report;
    let build_token = token(&report);
    let source_origins = session
        .compiled
        .as_ref()
        .unwrap()
        .static_prop_origins
        .clone();
    session.compiled.as_mut().unwrap().static_prop_origins =
        Err("Association intentionally unavailable".into());
    session
        .begin_property_edit(crate::session::PropertyTarget::World)
        .unwrap();
    let draft = session.property_draft().unwrap().token;
    session
        .append_property_change(
            draft,
            crate::session::PropertyChange::Set {
                key: "skyname".into(),
                value: "unapplied_prop_frame".into(),
            },
        )
        .unwrap();
    let source_camera = session.camera();
    let selection_before = session.selected_ids();
    assert!(
        session
            .select_compiled_prop_source(Actor::User, build_token.clone(), 0)
            .is_err()
    );
    assert_eq!(session.selected_ids(), selection_before);
    session.compiled.as_mut().unwrap().static_prop_origins = source_origins.clone();
    session
        .select_compiled_prop_source(Actor::User, build_token.clone(), 0)
        .unwrap();
    assert_eq!(session.selected_ids(), [map_core::ObjectId::Entity(9100)]);
    assert_eq!(session.camera(), source_camera);
    assert_eq!(session.property_draft().unwrap().token, draft);
    session
        .select(&selection_before, crate::session::SelectMode::Replace)
        .unwrap();
    session.compiled.as_mut().unwrap().static_prop_origins =
        Err("Association intentionally unavailable".into());
    let preserved = (
        session.document.vmf().clone(),
        session.history_snapshot(),
        session.selected_ids(),
        json!(session.property_draft()),
        std::fs::read(&source).unwrap(),
    );
    for (index, projection) in [
        render::Projection::Perspective,
        render::Projection::Top,
        render::Projection::Front,
        render::Projection::Side,
    ]
    .into_iter()
    .enumerate()
    {
        session.set_projection(Actor::User, projection);
        let ordinal = [0, 1, 2, 0][index];
        let origin = Vec3::new([-64., 0., 64., -64.][index], 0., 32.);
        let actor = if index % 2 == 0 {
            Actor::User
        } else {
            Actor::Agent
        };
        let frame = session
            .frame_compiled_prop_origin(actor, build_token.clone(), ordinal, 1.7)
            .unwrap();
        assert_eq!(
            frame.inspection.report["props"]["rows"][0]["ordinal"],
            ordinal
        );
        assert_eq!(
            frame.inspection.report["props"]["rows"][0]["source_association"]["status"],
            "unavailable"
        );
        assert_eq!(
            frame.bounds,
            [
                (origin - Vec3::splat(16.)).to_array(),
                (origin + Vec3::splat(16.)).to_array()
            ]
        );
        assert_eq!(frame.camera.projection, projection);
        let matrix = frame.camera.view_projection(1.7);
        let center = matrix * origin.extend(1.);
        let ndc = center.truncate() / center.w;
        assert!(center.w > 0. && ndc.x.abs() < 0.001 && ndc.y.abs() < 0.001);
        for bits in 0..8 {
            let point = Vec3::new(
                frame.bounds[usize::from(bits & 1 != 0)][0],
                frame.bounds[usize::from(bits & 2 != 0)][1],
                frame.bounds[usize::from(bits & 4 != 0)][2],
            );
            let clip = matrix * point.extend(1.);
            let ndc = clip.truncate() / clip.w;
            assert!(clip.w > 0. && ndc.is_finite() && ndc.x.abs() <= 1.001 && ndc.y.abs() <= 1.001);
        }
        assert_eq!(
            (
                session.document.vmf().clone(),
                session.history_snapshot(),
                session.selected_ids(),
                json!(session.property_draft()),
                std::fs::read(&source).unwrap()
            ),
            preserved
        );
    }
    let camera = session.camera();
    session.compiled.as_mut().unwrap().static_prop_origins = source_origins;
    session
        .preview_transform(
            Actor::User,
            &[map_core::ObjectId::Entity(9101)],
            map_core::transform::Transform {
                translation: glam::DVec3::X,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let geometry = session.render_document().vmf().clone();
    let selection = session.selected_ids();
    assert!(
        session
            .select_compiled_prop_source(Actor::User, build_token.clone(), 0)
            .unwrap_err()
            .contains("geometry")
    );
    assert_eq!(session.render_document().vmf(), &geometry);
    assert!(session.transform_preview().is_some());
    assert_eq!(session.selected_ids(), selection);
    session.cancel_transform_preview();
    for (requested, prop) in [("foreign".into(), 0), (build_token.clone(), 3)] {
        assert!(
            session
                .select_compiled_prop_source(Actor::Agent, requested, prop)
                .is_err()
        );
        assert_eq!(session.selected_ids(), selection);
        assert_eq!(session.camera(), camera);
    }
    for (requested, prop, aspect) in [
        ("foreign".to_owned(), 0, 1.),
        (build_token.clone(), 3, 1.),
        (build_token.clone(), 0, f32::NAN),
        (build_token.clone(), 0, 0.),
        (build_token.clone(), 0, 101.),
    ] {
        assert!(
            session
                .frame_compiled_prop_origin(Actor::Agent, requested, prop, aspect)
                .is_err()
        );
        assert_eq!(session.camera(), camera);
    }
    session
        .preview_block(glam::DVec3::ZERO, glam::DVec3::splat(32.))
        .unwrap();
    let preview = session.block_preview;
    assert!(
        session
            .select_compiled_prop_source(Actor::Agent, build_token.clone(), 0)
            .is_err()
    );
    assert_eq!(session.block_preview, preview);
    assert_eq!(session.selected_ids(), selection);

    assert!(
        session
            .frame_compiled_prop_origin(Actor::User, build_token.clone(), 0, 1.)
            .unwrap_err()
            .contains("viewport")
    );
    assert_eq!(session.block_preview, preview);
    assert_eq!(session.camera(), camera);
    session.cancel_block_preview();
    let output = session.compiled.as_ref().unwrap().path.clone();
    let bytes = std::fs::read(&output).unwrap();
    std::fs::write(&output, b"replaced").unwrap();
    assert!(
        session
            .select_compiled_prop_source(Actor::Agent, build_token.clone(), 0)
            .is_err()
    );
    assert_eq!(session.selected_ids(), selection);

    assert!(
        session
            .frame_compiled_prop_origin(Actor::User, build_token.clone(), 0, 1.)
            .is_err()
    );
    assert_eq!(session.camera(), camera);
    std::fs::write(output, bytes).unwrap();
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                bounces: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        session
            .select_compiled_prop_source(Actor::Agent, build_token.clone(), 0)
            .is_err()
    );
    assert_eq!(session.selected_ids(), selection);
    assert!(
        session
            .frame_compiled_prop_origin(Actor::User, build_token, 0, 1.)
            .is_err()
    );
    assert_eq!(session.camera(), camera);
}
