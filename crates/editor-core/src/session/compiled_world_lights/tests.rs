use std::path::PathBuf;
use std::sync::Arc;

use compiler::{Control, LightingMode};
use glam::DVec3;
use map_core::{ObjectId, transform::Transform};
use serde_json::Value;
use source_formats::bsp::Bsp;

use super::*;
use crate::session::{
    Actor, CompiledMap, PropertyChange, PropertyTarget, Snapshot, StaticPropSources, baked_build,
};

const LDR_LUMP: usize = 15;
const HDR_LUMP: usize = 54;
const RECORD_BYTES: usize = 88;
const FLOAT_OFFSETS: [usize; 16] = [0, 4, 8, 12, 16, 20, 24, 28, 32, 48, 52, 56, 60, 64, 68, 72];
const LITERAL_WORDS: [u32; 22] = [
    0x8000_0000,
    0x3fa0_0000,
    0xc020_0000,
    0x0000_0001,
    0x3f80_0001,
    0x7f7f_ffff,
    0xbf40_0000,
    0x3f00_0000,
    0xbf80_0000,
    0xffff_fff9,
    0xffff_fb2e,
    0xffff_fffd,
    0xbe00_0000,
    0x3f60_0000,
    0xc090_0000,
    0x42f7_0000,
    0xc000_0000,
    0x3d00_0000,
    0x8000_0000,
    0x8000_0005,
    0xffff_ffed,
    0x075b_cd15,
];

fn literal_records(kinds: &[i32]) -> Vec<u8> {
    let mut result = Vec::with_capacity(kinds.len() * RECORD_BYTES);
    for &kind in kinds {
        let start = result.len();
        result.extend(LITERAL_WORDS.iter().flat_map(|word| word.to_le_bytes()));
        result[start + 40..start + 44].copy_from_slice(&kind.to_le_bytes());
    }
    result
}

fn literal_bsp() -> Bsp {
    let mut bsp = Bsp::new();
    bsp.lumps[LDR_LUMP].data = literal_records(&[-1234, 4, 0]);
    bsp.lumps[HDR_LUMP].data = literal_records(&[2]);
    bsp.lumps[HDR_LUMP].data[12..16].copy_from_slice(&0x40a0_0000_u32.to_le_bytes());
    bsp
}

struct OwnedFixture {
    directory: tempfile::TempDir,
    session: Session,
    source: PathBuf,
    output: PathBuf,
}

impl OwnedFixture {
    fn new() -> Self {
        let (directory, game, vmf) = test_support::fixture();
        let source = directory.path().join("maps/literal.vmf");
        let output = source.with_extension("bsp");
        std::fs::write(&source, vmf.write(&Default::default())).unwrap();
        std::fs::write(source.with_extension("lin"), b"prior pointfile").unwrap();
        let mut session = Session::new(Ok(game));
        session.open(Actor::User, &source, false).unwrap();
        Self {
            directory,
            session,
            source,
            output,
        }
    }

    fn install(&mut self, bytes: &[u8], mode: LightingMode, token: u64) {
        self.session
            .update_compile_settings(Actor::User, Some(mode.name()), Some("none"))
            .unwrap();
        std::fs::write(&self.output, bytes).unwrap();
        let output = compiler::FileIdentity::capture(&self.output, &Control::default()).unwrap();
        let source = compiler::FileIdentity::capture(&self.source, &Control::default()).unwrap();
        self.session.compiled = Some(CompiledMap {
            claim: None,
            output_identity: Ok(output.clone()),
            face_origins: Err("Literal world-light fixture has no authored face mapping".into()),
            path: self.output.clone(),
            snapshot: Snapshot {
                generation: self.session.document_generation,
                revision: self.session.document().revision(),
            },
            options: self.session.compile_options(),
            content_epoch: self.session.content_epoch,
            lightmap_identity: baked_build::BuildIdentity {
                token,
                output: Ok(output),
                source: Ok(Some(source)),
            },
            prop_lighting_guard: None,
            static_prop_sources: None,
            instance_dependencies: None,
            instance_source_path: None,
        });
    }

    fn inspect(&self, query: &Query) -> Result<Inspection, String> {
        self.inspect_controlled(query, &Control::default())
    }

    fn inspect_controlled(&self, query: &Query, control: &Control) -> Result<Inspection, String> {
        let state = || {
            (
                self.session.document().vmf().clone(),
                self.session.render_document().vmf().clone(),
                self.session.history_snapshot(),
                self.session.camera(),
                self.session.selected_ids(),
                self.session.compile_settings(),
                serde_json::to_value(self.session.property_draft()).unwrap(),
                [
                    &self.source,
                    &self.output,
                    &self.source.with_extension("lin"),
                ]
                .map(|path| std::fs::read(path).ok()),
            )
        };
        let before = state();
        let result = self
            .session
            .inspect_compiled_world_lights_controlled(query, control);
        assert_eq!(state(), before);
        if let Ok(inspection) = &result {
            assert!(serde_json::to_vec(inspection).unwrap().len() <= 256 * 1024);
            assert!(inspection.limits.conservative_live_bytes <= 384 * 1024 * 1024);
        }
        result
    }
}

fn token(inspection: &Inspection) -> String {
    inspection.build["token"].as_str().unwrap().to_owned()
}

fn rows(inspection: &Inspection) -> Value {
    serde_json::to_value(&inspection.page.rows).unwrap()
}

fn assert_unavailable(inspection: &Inspection) {
    assert_eq!(inspection.build["status"], "unavailable");
    assert!(inspection.build["token"].is_null());
    assert!(!inspection.historical);
    assert_eq!(inspection.page.returned, 0);
    assert!(inspection.page.rows.is_empty());
    assert!(inspection.page.total.is_none() && inspection.page.next_offset.is_none());
    for channel in &inspection.channels {
        assert_eq!(channel.status, "unavailable");
        assert!(channel.bytes.is_none() && channel.count.is_none());
    }
}

fn assert_literal(row: &WorldLightRow) {
    assert_eq!(row.ordinal, 0);
    assert_eq!((row.kind, row.kind_name), (-1234, "unknown"));
    assert_eq!(
        row.origin.map(f32::to_bits),
        [0x8000_0000, 0x3fa0_0000, 0xc020_0000]
    );
    assert_eq!(
        row.intensity.map(f32::to_bits),
        [0x0000_0001, 0x3f80_0001, 0x7f7f_ffff]
    );
    assert_eq!(
        row.normal.map(f32::to_bits),
        [0xbf40_0000, 0x3f00_0000, 0xbf80_0000]
    );
    assert_eq!((row.cluster, row.style), (-7, -3));
    assert_eq!(
        [row.stop_dot, row.stop_dot2, row.exponent, row.radius].map(f32::to_bits),
        [0xbe00_0000, 0x3f60_0000, 0xc090_0000, 0x42f7_0000]
    );
    assert_eq!(
        [
            row.constant_attenuation,
            row.linear_attenuation,
            row.quadratic_attenuation
        ]
        .map(f32::to_bits),
        [0xc000_0000, 0x3d00_0000, 0x8000_0000]
    );
    assert_eq!(
        (row.flags, row.texinfo, row.owner),
        (i32::MIN + 5, -19, 123_456_789)
    );
    let serialized = serde_json::to_value(row).unwrap();
    assert_eq!(
        serialized["origin"][0].as_f64().unwrap().to_bits(),
        (-0.0_f64).to_bits()
    );
    assert_eq!(
        (serialized["intensity"][0].as_f64().unwrap() as f32).to_bits(),
        1
    );
    assert_eq!(
        (serialized["intensity"][1].as_f64().unwrap() as f32).to_bits(),
        0x3f80_0001
    );
    assert_eq!(
        (serialized["intensity"][2].as_f64().unwrap() as f32).to_bits(),
        0x7f7f_ffff
    );
    assert_eq!(serialized["flags"], i32::MIN + 5);
}

#[test]
fn literal_native_records_and_real_dual_publication_preserve_fields_and_channels() {
    let mut fixture = OwnedFixture::new();
    fixture.install(&literal_bsp().to_bytes(), LightingMode::DirectLdrHdr, 41);
    let inspected = fixture.inspect(&Query::default()).unwrap();
    assert_eq!(inspected.build["status"], "current");
    assert!(!inspected.historical);
    assert_eq!(
        (inspected.channels[0].lump, inspected.channels[1].lump),
        (15, 54)
    );
    assert_eq!(
        (inspected.channels[0].count, inspected.channels[1].count),
        (Some(3), Some(1))
    );
    assert_eq!(
        (inspected.channels[0].bytes, inspected.channels[1].bytes),
        (Some(264), Some(88))
    );
    assert_literal(&inspected.page.rows[0]);
    assert_eq!(
        (
            inspected.page.rows[1].ordinal,
            inspected.page.rows[1].kind_name
        ),
        (1, "quake")
    );
    assert_eq!(
        (
            inspected.page.rows[2].ordinal,
            inspected.page.rows[2].kind_name
        ),
        (2, "surface")
    );
    let hdr = fixture
        .inspect(&Query {
            build_token: Some(token(&inspected)),
            channel: Channel::Hdr,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(hdr.channel, "hdr");
    assert_eq!(hdr.page.rows[0].intensity[0].to_bits(), 0x40a0_0000);
    assert_eq!(hdr.page.rows[0].kind_name, "spot");

    let (_directory, session, source) = test_support::compiled(true);
    let before = (
        session.document().vmf().clone(),
        session.history_snapshot(),
        session.camera(),
        session.selected_ids(),
        session.compile_settings(),
        std::fs::read(&source).unwrap(),
        std::fs::read(source.with_extension("bsp")).unwrap(),
    );
    for (channel, intensities) in [
        (
            Channel::Ldr,
            [
                [2.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, 4.0, 0.0],
                [0.0, 0.0, 0.5],
            ],
        ),
        (
            Channel::Hdr,
            [
                [0.0, 6.0, 0.0],
                [3.0, 0.0, 3.0],
                [4.0, 0.0, 0.0],
                [0.5, 0.5, 0.0],
            ],
        ),
    ] {
        let inspected = session
            .inspect_compiled_world_lights(&Query {
                channel,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(inspected.build["status"], "current");
        assert_eq!(inspected.page.total, Some(4));
        assert_eq!(inspected.page.rows.len(), 4);
        for (ordinal, row) in inspected.page.rows.iter().enumerate() {
            assert_eq!(row.ordinal, ordinal);
            assert_eq!(row.intensity, intensities[ordinal]);
            assert_eq!(
                (row.kind, row.kind_name),
                [(1, "point"), (2, "spot"), (3, "sky"), (5, "sky_ambient")][ordinal]
            );
            assert_eq!(
                row.origin,
                [
                    [-64.0, 0.0, 160.0],
                    [64.0, 32.0, 192.0],
                    [-16.0, 48.0, 224.0],
                    [-16.0, 48.0, 224.0]
                ][ordinal]
            );
            assert_eq!(row.cluster, [0, 0, -1, -1][ordinal]);
            assert_eq!(row.style, [3, 7, 0, 0][ordinal]);
            assert_eq!((row.flags, row.texinfo, row.owner), (0, -1, -1));
            assert_eq!(row.radius, [512.0, 384.0, 0.0, 0.0][ordinal]);
            assert_eq!(
                [
                    row.constant_attenuation,
                    row.linear_attenuation,
                    row.quadratic_attenuation
                ],
                [[1.0, 0.0, 0.0], [2.0, 0.01, 0.0001], [0.0; 3], [0.0; 3]][ordinal]
            );
            assert_eq!(
                [row.stop_dot, row.stop_dot2, row.exponent],
                if ordinal == 1 {
                    [0.5, 0.5, 2.0]
                } else {
                    [0.0; 3]
                }
            );
            let normal = if matches!(ordinal, 1 | 2) {
                [0.0, 0.0, -1.0]
            } else {
                [0.0; 3]
            };
            for (&actual, expected) in row.normal.iter().zip(normal) {
                assert!((actual - expected).abs() < 1e-6);
            }
        }
    }
    assert_eq!(
        (
            session.document().vmf().clone(),
            session.history_snapshot(),
            session.camera(),
            session.selected_ids(),
            session.compile_settings(),
            std::fs::read(&source).unwrap(),
            std::fs::read(source.with_extension("bsp")).unwrap()
        ),
        before
    );
    assert_eq!(
        std::fs::read(source.with_extension("lin")).unwrap(),
        b"previous pointfile"
    );
}

#[test]
fn owned_pages_preserve_drafts_and_historical_sources_without_adopting_replacements() {
    let mut fixture = OwnedFixture::new();
    assert_unavailable(&fixture.inspect(&Query::default()).unwrap());
    assert!(
        fixture
            .inspect(&Query {
                build_token: Some("41".into()),
                ..Default::default()
            })
            .is_err()
    );
    fixture.install(&literal_bsp().to_bytes(), LightingMode::DirectLdrHdr, 41);
    let first = fixture
        .inspect(&Query {
            limit: 2,
            ..Default::default()
        })
        .unwrap();
    let captured = token(&first);
    assert_eq!(
        (
            first.page.offset,
            first.page.limit,
            first.page.returned,
            first.page.total,
            first.page.next_offset
        ),
        (0, 2, 2, Some(3), Some(2))
    );
    let tail = fixture
        .inspect(&Query {
            build_token: Some(captured.clone()),
            offset: 2,
            limit: 2,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        (
            tail.page.returned,
            tail.page.rows[0].ordinal,
            tail.page.next_offset
        ),
        (1, 2, None)
    );
    let end = fixture
        .inspect(&Query {
            build_token: Some(captured.clone()),
            offset: 3,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        (end.page.returned, end.page.total, end.page.next_offset),
        (0, Some(3), None)
    );
    assert!(end.page.rows.is_empty());
    for bad in [
        Query {
            offset: 1,
            ..Default::default()
        },
        Query {
            limit: 0,
            ..Default::default()
        },
        Query {
            limit: 65,
            ..Default::default()
        },
        Query {
            build_token: Some(captured.clone()),
            offset: 4,
            ..Default::default()
        },
        Query {
            build_token: Some(captured.clone()),
            offset: usize::MAX,
            limit: 2,
            ..Default::default()
        },
    ] {
        assert!(fixture.inspect(&bad).is_err());
    }
    for bad_token in [
        "",
        "wrong",
        "42",
        "041",
        " 41",
        "+41",
        "18446744073709551616",
    ] {
        assert!(
            fixture
                .inspect(&Query {
                    build_token: Some(bad_token.into()),
                    ..Default::default()
                })
                .is_err()
        );
    }
    let baseline = fixture.inspect(&Query::default()).unwrap();
    fixture
        .session
        .begin_property_edit(PropertyTarget::Entities(vec![9001]))
        .unwrap();
    let draft = fixture.session.property_draft().unwrap().token;
    fixture
        .session
        .append_property_change(
            draft,
            PropertyChange::Set {
                key: "_light".into(),
                value: "0 255 0 999".into(),
            },
        )
        .unwrap();
    let drafted = fixture.inspect(&Query::default()).unwrap();
    assert_eq!(drafted.build["status"], "current");
    assert_eq!(drafted.build["render_context"]["pending_preview"], false);
    assert_eq!(rows(&drafted), rows(&baseline));
    fixture.session.check_property_draft(draft).unwrap();
    fixture.session.cancel_property_edit(draft).unwrap();
    let solid = fixture.session.document().vmf().world.solids[0].id;
    fixture
        .session
        .preview_transform(
            Actor::User,
            &[ObjectId::Solid(solid)],
            Transform {
                translation: DVec3::new(8.0, 0.0, 0.0),
                ..Default::default()
            },
            Some(DVec3::ZERO),
        )
        .unwrap();
    let geometry_draft = fixture.inspect(&Query::default()).unwrap();
    assert_eq!(geometry_draft.build["status"], "current");
    assert_eq!(
        geometry_draft.build["render_context"]["pending_preview"],
        true
    );
    assert_eq!(
        geometry_draft.build["render_context"]["current_for_display"],
        false
    );
    assert_eq!(rows(&geometry_draft), rows(&baseline));
    assert!(fixture.session.transform_preview().is_some());
    fixture.session.cancel_transform_preview();

    fixture
        .session
        .update_compile_settings(Actor::User, Some("fullbright"), None)
        .unwrap();
    let stale_settings = fixture.inspect(&Query::default()).unwrap();
    assert_eq!(stale_settings.build["status"], "stale");
    assert!(stale_settings.historical);
    assert_eq!(stale_settings.channels[0].status, "available");
    assert_eq!(rows(&stale_settings), rows(&baseline));
    fixture
        .session
        .update_compile_settings(Actor::User, Some("direct_ldr_hdr"), None)
        .unwrap();
    fixture.session.content_epoch += 1;
    assert_eq!(
        fixture.inspect(&Query::default()).unwrap().build["status"],
        "stale"
    );
    fixture.session.content_epoch -= 1;

    let sources = compiler::asset_sources::BuildMaterialSources::default();
    sources
        .read_asset(
            fixture.session.game(),
            "materials/test/checker.vmt",
            1024 * 1024,
        )
        .unwrap()
        .unwrap();
    let guard = sources
        .source_guard(fixture.session.game(), &Control::default())
        .unwrap();
    fixture
        .session
        .compiled
        .as_mut()
        .unwrap()
        .static_prop_sources = Some(StaticPropSources {
        guard,
        game: Arc::clone(&fixture.session.game),
        content_epoch: fixture.session.content_epoch,
    });
    assert_eq!(
        fixture.inspect(&Query::default()).unwrap().build["status"],
        "current"
    );
    std::fs::write(
        fixture
            .directory
            .path()
            .join("materials/test/unrelated.vmt"),
        "UnlitGeneric {}",
    )
    .unwrap();
    let metadata_stale = fixture.inspect(&Query::default()).unwrap();
    assert_eq!(metadata_stale.build["status"], "stale");
    assert!(metadata_stale.historical);
    assert_eq!(rows(&metadata_stale), rows(&baseline));
    assert_eq!(
        fixture.session.baked_lightmap_build_state()["status"],
        "current"
    );

    fixture.install(&literal_bsp().to_bytes(), LightingMode::DirectLdrHdr, 42);
    assert!(
        fixture
            .inspect(&Query {
                build_token: Some(captured),
                ..Default::default()
            })
            .is_err()
    );
    let replacement_owner = fixture.inspect(&Query::default()).unwrap();
    assert_eq!(token(&replacement_owner), "42");
    let replacement = fixture.directory.path().join("same-source.vmf");
    std::fs::copy(&fixture.source, &replacement).unwrap();
    std::fs::rename(replacement, &fixture.source).unwrap();
    let source_stale = fixture.inspect(&Query::default()).unwrap();
    assert_eq!(source_stale.build["status"], "stale");
    assert!(source_stale.historical);
    assert_eq!(rows(&source_stale), rows(&baseline));
    let replacement = fixture.directory.path().join("same-output.bsp");
    std::fs::copy(&fixture.output, &replacement).unwrap();
    std::fs::rename(replacement, &fixture.output).unwrap();
    assert_unavailable(&fixture.inspect(&Query::default()).unwrap());
    assert_unavailable(
        &fixture
            .inspect(&Query {
                build_token: Some("42".into()),
                offset: 1,
                ..Default::default()
            })
            .unwrap(),
    );
    std::fs::remove_file(&fixture.output).unwrap();
    assert_unavailable(&fixture.inspect(&Query::default()).unwrap());

    let mut edited = OwnedFixture::new();
    edited.install(&literal_bsp().to_bytes(), LightingMode::DirectLdrHdr, 43);
    edited
        .session
        .begin_property_edit(PropertyTarget::Entities(vec![9001]))
        .unwrap();
    let draft = edited.session.property_draft().unwrap().token;
    edited
        .session
        .append_property_change(
            draft,
            PropertyChange::Set {
                key: "_light".into(),
                value: "0 255 0 999".into(),
            },
        )
        .unwrap();
    edited
        .session
        .commit_property_edit(Actor::User, draft)
        .unwrap();
    let committed = edited.inspect(&Query::default()).unwrap();
    assert_eq!(committed.build["status"], "stale");
    assert!(committed.historical);
    assert_eq!(rows(&committed), rows(&baseline));

    #[cfg(unix)]
    for source_path in [true, false] {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let mut encoded = OwnedFixture::new();
        let parent = encoded
            .directory
            .path()
            .join(OsString::from_vec(b"native-path-\xff".to_vec()));
        std::fs::create_dir(&parent).unwrap();
        if source_path {
            let source = parent.join("literal.vmf");
            std::fs::rename(&encoded.source, &source).unwrap();
            std::fs::rename(
                encoded.source.with_extension("lin"),
                source.with_extension("lin"),
            )
            .unwrap();
            encoded.session.path = Some(source.clone());
            encoded.source = source;
        } else {
            encoded.output = parent.join("literal.bsp");
        }
        let observed = encoded.session.compiled_world_light_observation();
        assert_eq!(observed["path_omitted"], source_path);
        assert_eq!(observed["path"].is_null(), source_path);
        assert!(observed["token"].is_null());
        encoded.install(&literal_bsp().to_bytes(), LightingMode::DirectLdrHdr, 44);
        let custody = || {
            (
                encoded.session.document().vmf().clone(),
                encoded.session.history_snapshot(),
                encoded.session.camera(),
                encoded.session.selected_ids(),
                encoded.session.compile_settings(),
                encoded.session.path.clone(),
                std::fs::read(&encoded.source).unwrap(),
                std::fs::read(&encoded.output).unwrap(),
                std::fs::read(encoded.source.with_extension("lin")).unwrap(),
            )
        };
        let before = custody();
        let observed = encoded.session.compiled_world_light_observation();
        assert_eq!(observed["path_omitted"], source_path);
        assert_eq!(observed["path"].is_null(), source_path);
        assert_eq!(observed["token"], "44");
        let build = encoded.session.compiled_world_light_build_state();
        let inspected = encoded.inspect(&Query::default()).unwrap();
        for build in [&build, &inspected.build] {
            assert_eq!(build["status"], "stale");
            assert_eq!(build["current"], false);
            assert!(build["bsp"].is_null() && build["source"].is_null());
            assert!(
                build["diagnostics"]
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
        assert!(inspected.historical);
        assert_eq!(token(&inspected), "44");
        assert_eq!(rows(&inspected), rows(&baseline));
        assert_eq!(custody(), before);
    }
}

#[test]
fn captured_modes_and_complete_table_validation_bound_empty_corrupt_and_cancelled_reads() {
    let mut fixture = OwnedFixture::new();
    let mut empty = Bsp::new();
    empty.lumps[LDR_LUMP].version = 91;
    empty.lumps[HDR_LUMP].version = 92;
    for (mode, available) in [
        (LightingMode::Fullbright, [false, false]),
        (LightingMode::DirectLdr, [true, false]),
        (LightingMode::DirectLdrHdr, [true, true]),
    ] {
        fixture.install(&empty.to_bytes(), mode, 51);
        for channel in [Channel::Ldr, Channel::Hdr] {
            let inspection = fixture
                .inspect(&Query {
                    channel,
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(inspection.build["status"], "current");
            assert!(inspection.page.rows.is_empty());
            assert_eq!(inspection.page.returned, 0);
            assert!(inspection.page.next_offset.is_none());
            for (summary, available) in inspection.channels.iter().zip(available) {
                assert_eq!(
                    summary.status,
                    if available { "available" } else { "not_baked" }
                );
                assert_eq!((summary.count, summary.bytes), (Some(0), Some(0)));
            }
            assert_eq!(inspection.page.total, Some(0));
        }
    }
    let mut ldr = literal_bsp();
    ldr.lumps[HDR_LUMP].data.clear();
    fixture.install(&ldr.to_bytes(), LightingMode::DirectLdr, 52);
    let absent = fixture
        .inspect(&Query {
            channel: Channel::Hdr,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(absent.channels[1].status, "not_baked");
    assert_eq!(absent.page.total, Some(0));
    assert!(absent.page.rows.is_empty());

    for (mode, bsp) in [
        (LightingMode::Fullbright, ldr),
        (LightingMode::DirectLdr, literal_bsp()),
    ] {
        fixture.install(&bsp.to_bytes(), mode, 53);
        assert_unavailable(&fixture.inspect(&Query::default()).unwrap());
    }
    for offset in FLOAT_OFFSETS {
        let mut bsp = literal_bsp();
        let start = 2 * RECORD_BYTES + offset;
        bsp.lumps[LDR_LUMP].data[start..start + 4].copy_from_slice(&0x7fc0_0001_u32.to_le_bytes());
        fixture.install(&bsp.to_bytes(), LightingMode::DirectLdrHdr, 54);
        let inspection = fixture
            .inspect(&Query {
                limit: 1,
                ..Default::default()
            })
            .unwrap();
        assert_unavailable(&inspection);
        assert!(
            inspection
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.to_ascii_lowercase().contains("finite")),
            "float byte offset {offset}"
        );
    }
    for case in [
        "other-channel infinity",
        "version",
        "stride",
        "compression",
        "count",
    ] {
        let mut bsp = literal_bsp();
        match case {
            "other-channel infinity" => {
                bsp.lumps[HDR_LUMP].data[60..64].copy_from_slice(&0xff80_0000_u32.to_le_bytes())
            }
            "version" => bsp.lumps[HDR_LUMP].version = 1,
            "stride" => {
                bsp.lumps[HDR_LUMP].data.pop();
            }
            "compression" => bsp.lumps[HDR_LUMP].fourcc = [1, 0, 0, 0],
            "count" => bsp.lumps[LDR_LUMP].data = literal_records(&vec![1; 8193]),
            _ => unreachable!(),
        }
        fixture.install(&bsp.to_bytes(), LightingMode::DirectLdrHdr, 55);
        assert_unavailable(
            &fixture
                .inspect(&Query {
                    limit: 1,
                    ..Default::default()
                })
                .unwrap(),
        );
    }
    let mut maximum = Bsp::new();
    maximum.lumps[LDR_LUMP].data = literal_records(&vec![1; 8192]);
    fixture.install(&maximum.to_bytes(), LightingMode::DirectLdr, 56);
    let page = fixture.inspect(&Query::default()).unwrap();
    assert_eq!(
        (page.page.returned, page.page.total, page.page.next_offset),
        (64, Some(8192), Some(64))
    );
    assert_eq!(page.page.rows[63].ordinal, 63);
    let end = fixture
        .inspect(&Query {
            build_token: Some(token(&page)),
            offset: 8192,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        (end.page.returned, end.page.total, end.page.next_offset),
        (0, Some(8192), None)
    );

    let mut overlapping = vec![0_u8; 1036 + 3 * 1024 * 1024];
    overlapping[..4].copy_from_slice(b"VBSP");
    overlapping[4..8].copy_from_slice(&20_i32.to_le_bytes());
    for lump in 0..64 {
        if lump == 35 {
            continue;
        }
        let offset = 8 + lump * 16;
        overlapping[offset..offset + 4].copy_from_slice(&1036_i32.to_le_bytes());
        overlapping[offset + 4..offset + 8].copy_from_slice(&(3 * 1024 * 1024_i32).to_le_bytes());
    }
    fixture.install(&overlapping, LightingMode::DirectLdrHdr, 57);
    let bounded = fixture.inspect(&Query::default()).unwrap();
    assert_unavailable(&bounded);
    assert!(
        bounded
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("decoded") && diagnostic.contains("128"))
    );
    let control = Control::default();
    control.cancel().unwrap();
    assert!(
        fixture
            .inspect_controlled(&Query::default(), &control)
            .is_err()
    );
}
