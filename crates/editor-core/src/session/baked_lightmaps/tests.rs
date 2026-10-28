use super::*;
use crate::session::{Actor, CompiledMap, Snapshot};
use source_formats::bsp::{Edge, Model, Plane, lump, surf};

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
        let leaf_helpers = document
            .vmf()
            .entities
            .iter()
            .map(|entity| entity.id)
            .collect::<Vec<_>>();
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
                            ..
                        } if *instance == root_owner && file == "leaf.vmf" => {
                            Some(SourceReference::Instance {
                                instance: *instance,
                                file: file.clone(),
                                solid: *solid,
                                side: *side,
                                occurrence: None,
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
        let mut exact_queries = Vec::new();
        for helper in leaf_helpers {
            let occurrence = vec![
                OccurrenceReference {
                    helper: root_owner,
                    file: "child.vmf".into(),
                },
                OccurrenceReference {
                    helper,
                    file: "leaf.vmf".into(),
                },
            ];
            let mut exact = query.clone();
            let Some(SourceReference::Instance {
                occurrence: route, ..
            }) = &mut exact.source
            else {
                unreachable!();
            };
            *route = Some(occurrence.clone());
            let inspected = session.inspect_baked_lightmaps(&exact).unwrap();
            assert!(
                inspected.report["mapping"]["match_page"]["total"]
                    .as_u64()
                    .unwrap()
                    > 0
            );
            assert_eq!(
                inspected.report["mapping"]["source"]["occurrence"],
                json!(occurrence)
            );
            assert!(
                inspected.report["mapping"]["contributors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|origin| origin["occurrence"] == json!(occurrence))
            );
            assert!(
                inspected.report["mapping"]["source_occurrence_label"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("entity:{helper} → leaf.vmf"))
            );
            exact_queries.push((exact, inspected.report["mapping"].clone()));
        }
        let mut absent = exact_queries[0].0.clone();
        let Some(SourceReference::Instance {
            occurrence: Some(route),
            ..
        }) = &mut absent.source
        else {
            unreachable!();
        };
        route[1].helper = u32::MAX;
        assert_eq!(
            session.inspect_baked_lightmaps(&absent).unwrap().report["mapping"]["match_page"]["total"],
            0
        );
        for mismatch in [0, 1] {
            let mut invalid = exact_queries[0].0.clone();
            let Some(SourceReference::Instance {
                occurrence: Some(route),
                ..
            }) = &mut invalid.source
            else {
                unreachable!();
            };
            if mismatch == 0 {
                route[0].helper = u32::MAX;
            } else {
                route[1].file = "foreign.vmf".into();
            }
            assert!(
                session
                    .inspect_baked_lightmaps(&invalid)
                    .err()
                    .unwrap()
                    .contains("outer helper")
            );
        }
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
        for (exact, mapping) in exact_queries {
            let historical = session.inspect_baked_lightmaps(&exact).unwrap();
            assert_eq!(historical.report["build"]["status"], "stale");
            assert_eq!(historical.report["mapping"], mapping);
        }
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
    bsp.set_records(&[Plane {
        normal: [0.0, 0.0, 1.0],
        dist: 32.0,
        kind: 2,
    }]);
    bsp.set_values(
        lump::VERTEXES,
        &[
            [-40.0_f32, 12.0, 32.0],
            [24.0, 12.0, 32.0],
            [24.0, 60.0, 32.0],
            [-40.0, 60.0, 32.0],
        ],
    );
    bsp.set_records(&[Edge([0, 1]), Edge([2, 1]), Edge([2, 3]), Edge([0, 3])]);
    bsp.set_values(lump::SURFEDGES, &[0_i32, -1, 2, -3]);
    bsp.set_records(&[Model {
        first_face: 0,
        face_count: 1,
        ..Default::default()
    }]);
    bsp.set_records(&[TexInfo {
        flags: surf::BUMPLIGHT,
        ..TexInfo::default()
    }]);
    let face = Face {
        first_edge: 0,
        edge_count: 4,
        dispinfo: -1,
        area: 3072.0,
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
    install_build_with_bsp(session, path, source, &fixture());
}

fn install_build_with_bsp(session: &mut Session, path: &Path, source: &Path, bsp: &Bsp) {
    let source_identity =
        compiler::FileIdentity::capture(source, &compiler::Control::default()).unwrap();
    session.compiled = Some(CompiledMap {
        visibility_inspection: Err("Literal fixture has no compiler visibility mapping".into()),
        world_light_origins: None,
        static_prop_origins: Err("Prop source metadata not retained in this fixture".into()),
        prop_lighting_guard: None,
        material_sources: None,
        static_prop_sources: None,
        claim: None,
        output_identity: super::super::baked_build::capture_output_identity(
            path,
            Some(bsp),
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
        lightmap_identity: BuildIdentity::capture(17, path, Ok(Some(source_identity)), Some(bsp)),
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
    for channel in [Channel::Ldr, Channel::Hdr] {
        let framed = session
            .frame_compiled_face(Actor::User, "17".into(), channel, 0, 1.6)
            .unwrap();
        assert_eq!(framed.bounds, [[-40.0, 12.0, 32.0], [24.0, 60.0, 32.0]]);
        assert_eq!(framed.camera.projection, camera.projection);
    }
    session.set_camera(camera);
    session
        .update_compile_settings(Actor::User, Some("direct_ldr"), None)
        .unwrap();
    assert!(
        session
            .frame_compiled_face(Actor::Agent, "17".into(), Channel::Ldr, 0, 1.0)
            .is_err()
    );
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
    assert!(
        session
            .frame_compiled_face(Actor::Agent, "17".into(), Channel::Ldr, 0, 1.0)
            .is_err()
    );
    let changed = session.inspect_baked_lightmaps(&Query::default()).unwrap();
    assert_eq!(changed.report["build"]["status"], "stale");
    assert!(changed.image.is_some());
    std::fs::write(&path, b"changed BSP").unwrap();
    assert!(
        session
            .frame_compiled_face(Actor::Agent, "17".into(), Channel::Ldr, 0, 1.0)
            .is_err()
    );
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
fn compiled_face_framing_refuses_invalid_geometry_and_active_work_atomically() {
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
    session
        .begin_property_edit(crate::session::PropertyTarget::World)
        .unwrap();
    let token = session.property_draft().unwrap().token;
    session
        .append_property_change(
            token,
            crate::session::PropertyChange::Set {
                key: "skyname".into(),
                value: "unapplied_face_frame".into(),
            },
        )
        .unwrap();
    let preserved = (
        session.camera(),
        session.document().vmf().clone(),
        session.history_snapshot(),
        session.selected_ids(),
        json!(session.property_draft()),
    );
    for (token, face, aspect) in [("foreign", 0, 1.0), ("17", 99, 1.0), ("17", 0, f32::NAN)] {
        assert!(
            session
                .frame_compiled_face(Actor::Agent, token.into(), Channel::Ldr, face, aspect)
                .is_err()
        );
    }
    session
        .preview_block(glam::DVec3::ZERO, glam::DVec3::splat(32.0))
        .unwrap();
    let preview = session.block_preview;
    assert!(
        session
            .frame_compiled_face(Actor::User, "17".into(), Channel::Ldr, 0, 1.0)
            .unwrap_err()
            .contains("viewport")
    );
    assert_eq!(session.block_preview, preview);
    session.cancel_block_preview();
    let mut malformed_edge = fixture();
    malformed_edge.set_values(lump::SURFEDGES, &[0_i32, i32::MIN, 2, -3]);
    let mut disconnected_edge = fixture();
    disconnected_edge.set_values(lump::SURFEDGES, &[0_i32, 1, 2, -3]);
    let mut nonplanar = fixture();
    let mut vertices = nonplanar.vertices().unwrap();
    vertices[0][2] = 33.0;
    nonplanar.set_values(lump::VERTEXES, &vertices);
    let mut terrain = fixture();
    let mut faces = terrain.lighting_faces(LightingKind::Ldr).unwrap();
    faces[0].dispinfo = 0;
    terrain
        .set_lighting_faces(LightingKind::Ldr, &faces)
        .unwrap();
    let mut model_local = fixture();
    model_local.set_records(&[
        Model::default(),
        Model {
            first_face: 0,
            face_count: 2,
            ..Default::default()
        },
    ]);
    for (bsp, message) in [
        (malformed_edge, "out-of-range edge"),
        (disconnected_edge, "connected"),
        (nonplanar, "flat plane"),
        (terrain, "displacement"),
        (model_local, "model"),
    ] {
        bsp.write(&output).unwrap();
        install_build_with_bsp(&mut session, &output, &source, &bsp);
        assert!(
            session
                .frame_compiled_face(Actor::Agent, "17".into(), Channel::Ldr, 0, 1.0)
                .unwrap_err()
                .contains(message)
        );
        assert_eq!(
            (
                session.camera(),
                session.document().vmf().clone(),
                session.history_snapshot(),
                session.selected_ids(),
                json!(session.property_draft()),
            ),
            preserved
        );
    }
}

#[test]
fn baked_surfaces_preserve_atomic_options_hidden_settings_and_retire_late_or_stale_work() {
    use crate::session::{PropertyTarget, compilation::BuildSettingsUpdate, working_region};
    use render::compiled_faces::{LightingSettings, StyleGain};

    let (directory, game) = crate::test_assets::game();
    let maps = directory.path().join("maps");
    std::fs::create_dir(&maps).unwrap();
    let source = maps.join("surfaces.vmf");
    let mut vmf = crate::test_assets::room();
    for side in vmf
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = "test/checker".into();
    }
    std::fs::write(&source, vmf.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::Agent, &source, false).unwrap();
    session
        .update_build_settings(
            Actor::Agent,
            BuildSettingsUpdate {
                lighting: Some("direct_ldr_hdr"),
                physics: Some("none"),
                generate_reflections: Some(false),
                pack_sky_assets: Some(false),
                pack_water_assets: Some(false),
                pack_static_prop_assets: Some(false),
                bake_static_prop_vertices: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let compiled = session.begin_compile(Actor::Agent, None).unwrap().run();
    session.finish_compile(Actor::Agent, &compiled);
    assert!(compiled.as_ref().unwrap().published());
    let preserved = (
        session.document().vmf().clone(),
        session.history_snapshot(),
        session.selected_ids(),
        session.camera(),
    );
    let state = super::super::baked_build::build_state_controlled(
        &session,
        super::super::baked_build::SourceVerification::OriginalMetadata {
            maximum_bytes: super::super::baked_surfaces::MAX_METADATA_BYTES,
        },
        &compiler::Control::default(),
    )
    .unwrap();
    assert_eq!(state["current"], true, "{state}");
    let request = session.begin_baked_surfaces(Actor::Agent).unwrap();
    let token = request.token();
    assert!(session.begin_baked_surfaces(Actor::User).is_err());
    let prepared = session
        .finish_baked_surfaces(Actor::Agent, request.run())
        .unwrap();
    assert_eq!(prepared["status"], "prepared", "{prepared}");
    assert_eq!(prepared["complete"], true);
    assert!(prepared["page"]["total"].as_u64().unwrap() > 0);
    session
        .set_baked_surfaces_visible(Actor::Agent, token, true)
        .unwrap();
    let shown = session.build_scene();
    assert!(shown.baked_faces.is_some());
    assert!(shown.baked_face_visible_surfaces > 0);
    assert!(
        shown
            .vertices
            .iter()
            .filter(|vertex| vertex.lighting_receiver & 15 == 4)
            .count()
            > 0
    );
    for surface in render::SurfaceView::ALL {
        let view = render::ViewOptions {
            surface,
            ..session.view()
        };
        let mesh = session.build_scene_for_view(view);
        assert!(mesh.baked_faces.is_some(), "{surface:?}");
        assert_eq!(session.baked_surfaces_render_report(view)["applied"], true);
    }
    let options = LightingSettings {
        channel: render::LightingChannel::Hdr,
        exposure_ev: 1.5,
        style_gains: vec![StyleGain {
            style: 0,
            gain: 0.25,
        }],
    };
    session
        .set_baked_surfaces_visible(Actor::User, token, false)
        .unwrap();
    session
        .set_baked_surfaces_options(Actor::Agent, token, options.clone())
        .unwrap();
    let before = session.baked_surfaces_observation();
    for invalid in [
        LightingSettings {
            exposure_ev: f64::NAN,
            ..options.clone()
        },
        LightingSettings {
            style_gains: vec![
                StyleGain {
                    style: 0,
                    gain: 1.0
                };
                2
            ],
            ..options.clone()
        },
        LightingSettings {
            style_gains: vec![StyleGain {
                style: 63,
                gain: 1.0,
            }],
            ..options.clone()
        },
    ] {
        assert!(
            session
                .set_baked_surfaces_options(Actor::User, token, invalid)
                .is_err()
        );
        assert_eq!(session.baked_surfaces_observation(), before);
    }
    session
        .set_baked_surfaces_visible(Actor::User, token, true)
        .unwrap();
    assert_eq!(
        session.baked_surfaces_observation()["lighting_options"],
        json!(options)
    );
    session.begin_property_edit(PropertyTarget::World).unwrap();
    session.preflight_baked_surfaces();
    assert_eq!(session.baked_surfaces_observation()["displayed"], false);
    session
        .cancel_property_edit(session.property_draft().unwrap().token)
        .unwrap();
    session.preflight_baked_surfaces();
    assert_eq!(session.baked_surfaces_observation()["displayed"], true);
    session
        .set_working_region(
            Actor::User,
            working_region::Action::Set {
                bounds: [glam::DVec3::splat(-1.0), glam::DVec3::splat(1.0)],
                enabled: true,
            },
            &session.working_region_stamp(),
        )
        .unwrap();
    let region = session.build_scene();
    assert!(region.baked_faces.is_some());
    assert!(region.baked_face_visible_surfaces < shown.baked_face_visible_surfaces);
    session
        .set_working_region(
            Actor::User,
            working_region::Action::Clear,
            &session.working_region_stamp(),
        )
        .unwrap();
    assert_eq!(
        (
            session.document().vmf().clone(),
            session.history_snapshot(),
            session.selected_ids(),
            session.camera()
        ),
        preserved
    );
    let face = prepared["page"]["rows"][0]["face"].as_u64().unwrap() as usize;
    assert!(
        session
            .frame_baked_surface(Actor::Agent, token, face, 1.5)
            .is_ok()
    );
    session.set_camera(preserved.3);
    let request = session.begin_baked_surfaces(Actor::Agent).unwrap();
    let hidden = session
        .set_baked_surfaces_visible(Actor::User, request.token(), false)
        .unwrap();
    assert_eq!(hidden["status"], "loading");
    assert_eq!(hidden["complete"], false);
    session
        .cancel_baked_surfaces(Actor::Agent, request.token())
        .unwrap();
    let hidden = session
        .set_baked_surfaces_visible(Actor::User, request.token(), false)
        .unwrap();
    assert_eq!(hidden["status"], "cancelling");
    let cancelled = session
        .finish_baked_surfaces(Actor::Agent, request.run())
        .unwrap();
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(cancelled["complete"], false);
    let request = session.begin_baked_surfaces(Actor::Agent).unwrap();
    session
        .clear_baked_surfaces(Actor::Agent, request.token())
        .unwrap();
    assert!(
        session
            .finish_baked_surfaces(Actor::Agent, request.run())
            .is_err()
    );
    let request = session.begin_baked_surfaces(Actor::Agent).unwrap();
    let token = request.token();
    session
        .finish_baked_surfaces(Actor::Agent, request.run())
        .unwrap();
    session
        .set_baked_surfaces_visible(Actor::User, token, true)
        .unwrap();
    std::fs::write(source.with_extension("bsp"), b"replaced owned output").unwrap();
    let retired = session.inspect_baked_surfaces(Some(token), 0, 64).unwrap();
    assert_eq!(retired["status"], "unavailable");
    assert_eq!(retired["displayed"], false);
    assert!(retired["page"]["total"].is_null());
    assert!(
        session
            .set_baked_surfaces_visible(Actor::User, token, true)
            .is_err()
    );
}

#[test]
fn authored_queries_retain_build_origins_and_preserve_the_selected_face_draft() {
    use crate::session::SelectMode;
    use map_core::surface::{FaceId, SurfaceEdit};
    let (directory, game) = crate::test_assets::game();
    let source = directory.path().join("source.vmf");
    let mut document = map_core::Document::new(crate::test_assets::room());
    let mut tops = Vec::new();
    document
        .edit("merged detail plates", |tx| {
            for x in [-128.0, 0.0] {
                let map_core::ObjectId::Solid(id) = map_core::edit::create_block(
                    tx,
                    glam::DVec3::new(x, -64.0, 16.0),
                    glam::DVec3::new(x + 128.0, 64.0, 32.0),
                    "test/checker",
                )?
                else {
                    unreachable!()
                };
                for side in &mut tx.solid_mut(id)?.sides {
                    side.lightmap_scale = 1;
                    if map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99 {
                        tops.push(side.id);
                    }
                }
                map_core::edit::tie_to_entity(tx, &[id], "func_detail", &[])?;
            }
            Ok(())
        })
        .unwrap();
    std::fs::write(&source, document.vmf().write(&Default::default())).unwrap();
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
    let origins = session
        .compiled
        .as_ref()
        .unwrap()
        .face_origins
        .as_ref()
        .unwrap();
    let merged = (0..origins.len()).find(|&face| {
        let mut contributors = origins.contributors(face).unwrap();
        contributors.len() == tops.len() && contributors.all(|origin| {
            matches!(origin, compiler::face_origins::Origin::Root { side, .. } if tops.contains(side))
        })
    }).expect("the adjacent detail tops retain both authored contributors");
    let build_token = first.report["build"]["token"].as_str().unwrap().to_owned();
    let mut framed_bounds = None;
    for (channel, projection) in [
        (Channel::Ldr, render::Projection::Perspective),
        (Channel::Hdr, render::Projection::Top),
    ] {
        session.set_projection(Actor::User, projection);
        let frame = session
            .frame_compiled_face(Actor::Agent, build_token.clone(), channel, merged, 1.7)
            .unwrap();
        assert_eq!(frame.face, merged);
        assert_eq!(frame.camera.projection, projection);
        assert_eq!(frame.bounds[0][2], 32.0);
        assert_eq!(frame.bounds[1][2], 32.0);
        assert!(frame.bounds[0][0] >= -128.0 && frame.bounds[1][0] <= 128.0);
        assert!(frame.bounds[0][1] >= -64.0 && frame.bounds[1][1] <= 64.0);
        if let Some(bounds) = framed_bounds {
            assert_eq!(frame.bounds, bounds);
        }
        framed_bounds = Some(frame.bounds);
        let matrix = frame.camera.view_projection(1.7);
        for bounds in frame.bounds {
            let clip = matrix * glam::Vec3::from_array(bounds).extend(1.0);
            let ndc = clip.truncate() / clip.w;
            assert!(
                clip.w > 0.0 && ndc.is_finite() && ndc.x.abs() <= 1.001 && ndc.y.abs() <= 1.001
            );
        }
        assert_eq!(session.render_document().vmf(), &rendered);
        assert_eq!(session.face_preview_token(), token);
    }
    session.set_camera(camera);
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
    assert!(
        session
            .frame_compiled_face(Actor::Agent, build_token, Channel::Ldr, merged, 1.7)
            .is_err()
    );
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
                        ..
                    } => Some(SourceReference::Instance {
                        instance: *instance,
                        file: file.clone(),
                        solid: *solid,
                        side: *side,
                        occurrence: None,
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
