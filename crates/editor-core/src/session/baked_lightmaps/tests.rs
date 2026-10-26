use super::*;
use crate::session::{Actor, CompiledMap, Snapshot};
use source_formats::bsp::surf;

#[test]
fn repeated_grandchild_origins_remain_grouped_and_stale_after_same_byte_replacement() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, game, source, middle) = crate::test_assets::instance_map(false);
        let leaf = directory.path().join("leaf.vmf");
        std::fs::copy(&middle, &leaf).unwrap();
        let mut document = map_core::Document::empty();
        document
            .edit("repeated grandchild", |tx| {
                for x in [0.0, 64.0] {
                    map_core::edit::create_entity(
                        tx,
                        "func_instance",
                        Some(glam::DVec3::new(x, 0.0, 0.0)),
                        &[
                            ("file".into(), "leaf.vmf".into()),
                            ("angles".into(), "0 0 0".into()),
                        ],
                        Vec::new(),
                    )?;
                }
                Ok(())
            })
            .unwrap();
        std::fs::write(&middle, document.vmf().write(&Default::default())).unwrap();
        let mut session = Session::new(Ok(game));
        session.open(actor, &source, true).unwrap();
        let root_owner = session.document().vmf().entities.last().unwrap().id;
        let memory = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let disk = std::fs::read(&source).unwrap();
        let result = session.begin_compile(actor, None).unwrap().run();
        session.finish_compile(actor, &result);
        assert!(result.as_ref().unwrap().published());
        let origins = session
            .compiled
            .as_ref()
            .unwrap()
            .face_origins
            .as_ref()
            .unwrap();
        let source_ref = (0..origins.len())
            .find_map(|face| {
                origins
                    .contributors(face)
                    .unwrap()
                    .find_map(|origin| match origin {
                        compiler::face_origins::Origin::Instance {
                            instance,
                            file,
                            solid,
                            side,
                        } if *instance == root_owner && file == "leaf.vmf" => {
                            Some(SourceReference::Instance {
                                instance: *instance,
                                file: file.clone(),
                                solid: *solid,
                                side: *side,
                            })
                        }
                        _ => None,
                    })
            })
            .unwrap();
        let query = Query {
            source: Some(source_ref),
            ..Default::default()
        };
        let before = session.inspect_baked_lightmaps(&query).unwrap();
        assert_eq!(before.report["build"]["status"], "current");
        assert!(
            before.report["mapping"]["match_page"]["total"]
                .as_u64()
                .unwrap()
                >= 2
        );
        let output = std::fs::read(source.with_extension("bsp")).unwrap();
        let replacement = directory.path().join("same-byte-leaf.vmf");
        std::fs::copy(&leaf, &replacement).unwrap();
        std::fs::rename(replacement, &leaf).unwrap();
        assert_eq!(session.compile_status()["job"]["stale"], true);
        let after = session.inspect_baked_lightmaps(&query).unwrap();
        assert_eq!(after.report["build"]["status"], "stale");
        assert_eq!(
            after.report["mapping"]["matches"],
            before.report["mapping"]["matches"]
        );
        assert_eq!(
            after.report["mapping"]["contributors"],
            before.report["mapping"]["contributors"]
        );
        assert!(
            after.report["build"]["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value
                    .as_str()
                    .unwrap()
                    .contains("Instance source/dependencies changed"))
        );
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(std::fs::read(&source).unwrap(), disk);
        assert_eq!(std::fs::read(source.with_extension("bsp")).unwrap(), output);
        assert_eq!(
            std::fs::read(source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
    }
}

fn color(rgb: [u8; 3], exponent: i8) -> RgbExp32 {
    RgbExp32 { rgb, exponent }
}

fn fixture() -> Bsp {
    let mut bsp = Bsp::new();
    bsp.set_records(&[TexInfo {
        flags: surf::BUMPLIGHT,
        ..TexInfo::default()
    }]);
    let face = Face {
        texinfo: 0,
        styles: [0, 255, 255, 255],
        light_offset: 4,
        lightmap_size: [1, 1],
        ..Face::default()
    };
    let ldr = vec![color([10, 20, 30], -1); 17];
    bsp.set_lighting_faces(
        LightingKind::Ldr,
        &[
            Face {
                light_offset: -1,
                ..face
            },
            face,
        ],
    )
    .unwrap();
    bsp.set_lighting(LightingKind::Ldr, &ldr).unwrap();
    let mut hdr = vec![color([15, 25, 35], 2), color([2, 4, 6], 4)];
    for style in 0..2 {
        for map in 0..4 {
            hdr.extend(vec![color([style + 2, map + 2, 7], 3); 4]);
        }
    }
    bsp.set_lighting_faces(
        LightingKind::Hdr,
        &[Face {
            styles: [0, 32, 255, 255],
            light_offset: 8,
            ..face
        }],
    )
    .unwrap();
    bsp.set_lighting(LightingKind::Hdr, &hdr).unwrap();
    bsp
}

#[test]
fn baked_lightmap_channel_style_and_bump_indices_decode_exact_ranges() {
    let bsp = fixture();
    let (ldr, image) = inspect_bsp(&bsp, &Query::default()).unwrap();
    assert_eq!(ldr["selected"]["face"], 1);
    assert_eq!(
        ldr["selected"]["linear_rgb"]["mean"],
        json!([5.0, 10.0, 15.0])
    );
    assert_eq!(ldr["selected"]["sample_bytes"], 64);
    assert_eq!(ldr["faces"][0]["status"], "unlit");
    assert!(image.is_some());
    let query = Query {
        channel: Channel::Hdr,
        face: Some(0),
        style_index: 1,
        map_index: 3,
        ..Query::default()
    };
    let (hdr, image) = inspect_bsp(&bsp, &query).unwrap();
    let selected = &hdr["selected"];
    assert_eq!(selected["style"], 32);
    assert_eq!(selected["map_kind"], "bump_3");
    assert_eq!(selected["dimensions"], json!([2, 2]));
    assert_eq!(selected["selected_sample_bytes"], 16);
    assert_eq!(selected["linear_rgb"]["minimum"], json!([24.0, 40.0, 56.0]));
    assert_eq!(selected["linear_rgb"]["mean"], json!([24.0, 40.0, 56.0]));
    assert_eq!(selected["linear_rgb"]["maximum"], json!([24.0, 40.0, 56.0]));
    assert_eq!(
        selected["linear_rgb"]["stored_base_average"],
        json!([60.0, 100.0, 140.0])
    );
    let image = image.unwrap();
    assert_eq!((image.width, image.height), (2, 2));
    assert_eq!(image.pixels.len(), 16);
    assert_eq!(&image.pixels[..4], &[84, 103, 117, 255]);
    assert_eq!(hdr["channels"][0]["lighting_bytes"], 68);
    assert_eq!(hdr["channels"][1]["lighting_bytes"], 136);
}

#[test]
fn baked_lightmap_invalid_ranges_and_absent_channel_never_produce_an_image() {
    let mut bsp = fixture();
    for offset in [-2, 3, 0, 4096] {
        let face = Face {
            texinfo: 0,
            styles: [0, 255, 255, 255],
            light_offset: offset,
            lightmap_size: [1, 1],
            ..Face::default()
        };
        bsp.set_lighting_faces(LightingKind::Ldr, &[face]).unwrap();
        let (report, image) = inspect_bsp(
            &bsp,
            &Query {
                face: Some(0),
                ..Query::default()
            },
        )
        .unwrap();
        assert_eq!(report["selected"]["status"], "invalid");
        assert!(report["selected"]["diagnostics"][0].is_string());
        assert!(image.is_none());
    }
    let (report, image) = inspect_bsp(&Bsp::new(), &Query::default()).unwrap();
    assert_eq!(report["channels"][0]["available"], false);
    assert_eq!(report["channels"][1]["available"], false);
    assert!(
        report["diagnostics"][0]
            .as_str()
            .unwrap()
            .contains("Fullbright")
    );
    assert!(image.is_none());
    assert!(
        inspect_bsp(
            &fixture(),
            &Query {
                face: Some(99),
                ..Query::default()
            }
        )
        .is_err()
    );
    assert!(
        inspect_bsp(
            &fixture(),
            &Query {
                style_index: 1,
                ..Query::default()
            }
        )
        .is_err()
    );
}

#[test]
fn baked_lightmap_preview_is_bounded_and_handles_extreme_rgbexp_values() {
    let samples = vec![color([255, 0, 1], 127); 512 * 2];
    let image = preview(&samples, [512, 2]);
    assert_eq!((image.width, image.height), (128, 1));
    assert_eq!(image.pixels.len(), 128 * 4);
    assert_eq!(&image.pixels[..4], &[255, 0, 255, 255]);
}

fn install_build(session: &mut Session, path: &Path, source: &Path) {
    let source_identity =
        compiler::FileIdentity::capture(source, &compiler::Control::default()).unwrap();
    session.compiled = Some(CompiledMap {
        world_light_origins: None,
        prop_lighting_guard: None,
        static_prop_sources: None,
        claim: None,
        output_identity: super::super::baked_build::capture_output_identity(
            path,
            Some(&fixture()),
            source_formats::bsp::PakLimits::default().max_bsp_bytes as u64,
            "Run",
        ),
        face_origins: Err("Fixture has no retained source mapping".into()),
        instance_dependencies: None,
        instance_source_path: None,
        path: path.to_owned(),
        snapshot: Snapshot {
            generation: session.document_generation,
            revision: session.document.revision(),
        },
        options: session.compile_options(),
        content_epoch: session.content_epoch,
        lightmap_identity: BuildIdentity::capture(
            17,
            path,
            Ok(Some(source_identity)),
            Some(&fixture()),
        ),
    });
}

#[test]
fn baked_lightmap_state_guards_source_settings_content_and_owned_bsp_without_edits() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.vmf");
    let path = directory.path().join("owned.bsp");
    std::fs::write(
        &source,
        map_core::Document::empty().vmf().write(&Default::default()),
    )
    .unwrap();
    fixture().write(&path).unwrap();
    let mut session = Session::new(Err("no assets".into()));
    session.open(Actor::User, &source, false).unwrap();
    let vmf = session.document().vmf().clone();
    let camera = session.camera();
    let history = session.document().undo_labels().count();
    let selection = session.selection().clone();
    let absent = session.inspect_baked_lightmaps(&Query::default()).unwrap();
    assert_eq!(absent.report["build"]["status"], "unavailable");
    install_build(&mut session, &path, &source);
    let inspected = session.inspect_baked_lightmaps(&Query::default()).unwrap();
    assert_eq!(inspected.report["build"]["status"], "current");
    assert!(inspected.image.is_some());
    session
        .update_compile_settings(Actor::User, Some("direct_ldr"), None)
        .unwrap();
    assert_eq!(
        session
            .inspect_baked_lightmaps(&Query::default())
            .unwrap()
            .report["build"]["status"],
        "stale"
    );
    session
        .update_compile_settings(Actor::User, Some("fullbright"), None)
        .unwrap();
    session.content_epoch += 1;
    assert_eq!(session.baked_lightmap_build_state()["status"], "stale");
    session.content_epoch -= 1;
    std::fs::write(&source, "external change").unwrap();
    let changed = session.inspect_baked_lightmaps(&Query::default()).unwrap();
    assert_eq!(changed.report["build"]["status"], "stale");
    assert!(changed.image.is_some());
    std::fs::write(&path, b"changed BSP").unwrap();
    let changed = session.inspect_baked_lightmaps(&Query::default()).unwrap();
    assert_eq!(changed.report["build"]["status"], "unavailable");
    assert!(changed.image.is_none());
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        session
            .inspect_baked_lightmaps(&Query::default())
            .unwrap()
            .report["build"]["status"],
        "unavailable"
    );
    assert_eq!(session.document().vmf(), &vmf);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.selection(), &selection);
    assert_eq!(session.camera(), camera);
    assert!(
        session
            .inspect_baked_lightmaps(&Query {
                limit: MAX_PAGE_FACES + 1,
                ..Query::default()
            })
            .is_err()
    );
}

#[test]
fn baked_lightmap_publication_identity_rejects_replacement_before_completion() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("replaced.bsp");
    Bsp::new().write(&path).unwrap();
    let identity = BuildIdentity::capture(1, &path, Ok(None), Some(&fixture()));
    assert!(identity.output.unwrap_err().contains("differ"));
    let oversized = std::fs::File::create(&path).unwrap();
    oversized.set_len(MAX_BSP_BYTES + 1).unwrap();
    let identity = BuildIdentity::capture(2, &path, Ok(None), Some(&fixture()));
    assert!(identity.output.unwrap_err().contains("budget"));
}

#[test]
fn authored_queries_retain_build_origins_and_preserve_the_selected_face_draft() {
    use crate::session::SelectMode;
    use map_core::surface::{FaceId, SurfaceEdit};
    let (directory, game) = crate::test_assets::game();
    let source = directory.path().join("source.vmf");
    std::fs::write(
        &source,
        crate::test_assets::room().write(&Default::default()),
    )
    .unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::Agent, &source, false).unwrap();
    session
        .update_compile_settings(Actor::Agent, Some("direct_ldr_hdr"), Some("none"))
        .unwrap();
    let result = session.begin_compile(Actor::Agent, None).unwrap().run();
    session.finish_compile(Actor::Agent, &result);
    assert_eq!(session.compile_status()["job"]["status"], "completed");
    let first = session.inspect_baked_lightmaps(&Query::default()).unwrap();
    assert_eq!(first.report["mapping"]["status"], "available");
    let origin = &first.report["mapping"]["contributors"][0];
    assert_eq!(origin["kind"], "root");
    let face = FaceId {
        solid: origin["solid"].as_u64().unwrap() as u32,
        side: origin["side"].as_u64().unwrap() as u32,
    };
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    session
        .edit_faces(
            Actor::Agent,
            &[face],
            SurfaceEdit {
                shift_u: Some(17.0),
                ..Default::default()
            },
            false,
            true,
        )
        .unwrap();
    let vmf = session.document().vmf().clone();
    let rendered = session.render_document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let token = session.face_preview_token();
    let selection = session.selected_faces();
    let query = Query {
        selected_authored_face: true,
        limit: 1,
        ..Default::default()
    };
    let matched = session.inspect_baked_lightmaps(&query).unwrap();
    assert_eq!(matched.report["build"]["current"], true);
    assert_eq!(
        matched.report["build"]["render_context"]["current_for_display"],
        false
    );
    assert_eq!(matched.report["mapping"]["match_page"]["limit"], 1);
    assert!(
        matched.report["mapping"]["match_page"]["total"]
            .as_u64()
            .unwrap()
            > 0
    );
    let compiled = matched.report["mapping"]["matches"][0]["face"]
        .as_u64()
        .unwrap() as usize;
    assert_eq!(matched.report["selected"]["face"], compiled);
    let hdr = session
        .inspect_baked_lightmaps(&Query {
            channel: Channel::Hdr,
            face: Some(compiled),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        hdr.report["mapping"]["contributors"],
        first.report["mapping"]["contributors"]
    );
    let no_match = session
        .inspect_baked_lightmaps(&Query {
            source: Some(SourceReference::Root {
                solid: u32::MAX,
                side: u32::MAX,
            }),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(no_match.report["mapping"]["match_page"]["total"], 0);
    assert!(no_match.image.is_none());
    assert!(
        session
            .inspect_baked_lightmaps(&Query {
                face: Some(compiled),
                selected_authored_face: true,
                ..Default::default()
            })
            .is_err()
    );
    std::fs::write(
        &source,
        b"externally changed source with reused numeric IDs",
    )
    .unwrap();
    let stale = session.inspect_baked_lightmaps(&query).unwrap();
    assert_eq!(stale.report["build"]["status"], "stale");
    assert_eq!(
        stale.report["mapping"]["source"],
        matched.report["mapping"]["source"]
    );
    session.compiled.as_mut().unwrap().face_origins =
        Err("Authored mapping budget exhausted".into());
    let compiled_only = session
        .inspect_baked_lightmaps(&Query {
            face: Some(compiled),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(compiled_only.report["mapping"]["status"], "unavailable");
    assert!(compiled_only.image.is_some());
    assert!(
        session
            .inspect_baked_lightmaps(&query)
            .unwrap()
            .image
            .is_none()
    );
    assert_eq!(session.document().vmf(), &vmf);
    assert_eq!(session.render_document().vmf(), &rendered);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.selected_faces(), selection);
    assert_eq!(session.face_preview_token(), token);
}

#[test]
fn instance_origins_remain_scoped_to_retained_child_bytes_after_external_changes() {
    let (_directory, game, source, child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.open(Actor::Agent, &source, false).unwrap();
    let result = session.begin_compile(Actor::Agent, None).unwrap().run();
    session.finish_compile(Actor::Agent, &result);
    assert_eq!(session.compile_status()["job"]["status"], "completed");
    let origins = session
        .compiled
        .as_ref()
        .unwrap()
        .face_origins
        .as_ref()
        .unwrap();
    let source_ref = (0..origins.len())
        .find_map(|face| {
            origins
                .contributors(face)
                .unwrap()
                .find_map(|origin| match origin {
                    compiler::face_origins::Origin::Instance {
                        instance,
                        file,
                        solid,
                        side,
                    } => Some(SourceReference::Instance {
                        instance: *instance,
                        file: file.clone(),
                        solid: *solid,
                        side: *side,
                    }),
                    _ => None,
                })
        })
        .unwrap();
    let query = Query {
        source: Some(source_ref),
        ..Default::default()
    };
    let before = session.inspect_baked_lightmaps(&query).unwrap();
    assert_eq!(before.report["build"]["status"], "current");
    assert!(
        before.report["mapping"]["match_page"]["total"]
            .as_u64()
            .unwrap()
            > 0
    );
    std::fs::write(&child, b"external replacement").unwrap();
    let after = session.inspect_baked_lightmaps(&query).unwrap();
    assert_eq!(after.report["build"]["status"], "stale");
    assert_eq!(
        after.report["mapping"]["matches"],
        before.report["mapping"]["matches"]
    );
    assert_eq!(
        after.report["mapping"]["contributors"],
        before.report["mapping"]["contributors"]
    );
    assert!(
        after.report["build"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value
                .as_str()
                .unwrap()
                .contains("Instance source/dependencies changed"))
    );
}

#[test]
fn output_changed_after_quick_state_clears_display_currentness_and_mapped_samples() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.vmf");
    let output = directory.path().join("owned.bsp");
    std::fs::write(
        &source,
        map_core::Document::empty().vmf().write(&Default::default()),
    )
    .unwrap();
    fixture().write(&output).unwrap();
    let mut session = Session::new(Err("no assets".into()));
    session.open(Actor::User, &source, false).unwrap();
    install_build(&mut session, &output, &source);
    let mut retained = session
        .inspect_baked_lightmaps(&Query::default())
        .unwrap()
        .report;
    assert_eq!(
        retained["build"]["render_context"]["current_for_display"],
        true
    );
    assert!(retained["selected"].is_object());
    std::fs::write(&output, b"changed between quick state and publication").unwrap();
    recheck(session.compiled.as_ref().unwrap(), &mut retained);
    assert_eq!(retained["build"]["status"], "unavailable");
    assert_eq!(
        retained["build"]["render_context"]["current_for_display"],
        false
    );
    assert!(retained["selected"].is_null());
    assert_eq!(retained["mapping"]["status"], "unavailable");
    assert!(retained["faces"].as_array().unwrap().is_empty());
}

#[test]
fn baked_reports_share_checked_paths_for_raw_physical_directory_aliases() {
    use std::ffi::OsString;
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let directory = tempfile::tempdir().unwrap();
    let physical = directory
        .path()
        .join(OsString::from_vec(b"source-\xff".to_vec()));
    std::fs::create_dir(&physical).unwrap();
    std::fs::write(
        physical.join("source.vmf"),
        map_core::Document::empty().vmf().write(&Default::default()),
    )
    .unwrap();
    let alias = directory.path().join("authored");
    symlink(&physical, &alias).unwrap();
    let source = alias.join("source.vmf");
    let output = directory.path().join("owned.bsp");
    fixture().write(&output).unwrap();
    let mut session = Session::new(Err("no assets".into()));
    session.open(Actor::User, &source, false).unwrap();
    install_build(&mut session, &output, &source);
    let before = (session.document().vmf().clone(), session.history_snapshot());
    for state in [
        session.baked_lightmap_build_state(),
        session.baked_prop_vertex_build_state(),
        session.compiled_world_light_build_state(),
        session
            .inspect_baked_lightmaps(&Query::default())
            .unwrap()
            .report["build"]
            .clone(),
    ] {
        assert_eq!(state["status"], "stale");
        assert_eq!(state["current"], false);
        assert!(state["source"].is_null() && state["bsp"].is_null());
        assert!(
            state["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| {
                    message.as_str().is_some_and(|message| {
                        message.contains("cannot be represented as UTF-8 JSON")
                    })
                })
        );
    }
    assert_eq!(
        (session.document().vmf().clone(), session.history_snapshot()),
        before
    );
}
