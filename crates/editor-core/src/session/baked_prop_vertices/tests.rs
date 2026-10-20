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
    let (_directory, session, _source) = compiled(true);
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
    let (_directory, session, _source) = compiled_options(false, 0, false);
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
    assert!(
        report["channels"]
            .as_array()
            .unwrap()
            .iter()
            .all(|channel| channel["available"] == false)
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
