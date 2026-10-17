use super::*;
use crate::session::{Actor, CompiledMap, Snapshot};
use source_formats::bsp::surf;

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
    session.compilation.options.lighting = compiler::LightingMode::DirectLdr;
    assert_eq!(
        session
            .inspect_baked_lightmaps(&Query::default())
            .unwrap()
            .report["build"]["status"],
        "stale"
    );
    session.compilation.options.lighting = compiler::LightingMode::Fullbright;
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
