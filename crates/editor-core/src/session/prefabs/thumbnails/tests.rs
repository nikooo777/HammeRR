use super::*;
use crate::session::{Actor, prefabs::fixtures::fixture};
use glam::DVec3;

fn saved() -> (Session, tempfile::TempDir) {
    let (mut session, directory, brush) = fixture();
    let stamp = session.prefab_report("", 0, 64, None)["snapshot"]
        .as_str()
        .unwrap()
        .to_string();
    session
        .save_prefab(Actor::User, "Fixture", &[brush], &stamp)
        .unwrap();
    (session, directory)
}

fn seed_cache(session: &Session, name: &str) -> Arc<RgbaImage> {
    let source = session.thumbnail_source(name).unwrap();
    let report = session.prepare_prefab_thumbnail(&source).unwrap().report;
    let image = Arc::new(RgbaImage {
        width: WIDTH,
        height: HEIGHT,
        pixels: vec![0; (WIDTH * HEIGHT * 4) as usize],
    });
    session
        .prefabs
        .thumbnails
        .cache
        .borrow_mut()
        .push_back(Cached {
            source: source.entry,
            text: source.text,
            epoch: (session.content_epoch, session.definitions_serial),
            thumbnail: Thumbnail {
                image: image.clone(),
                report,
            },
        });
    image
}

#[test]
fn thumbnail_inspection_is_headless_and_preserves_map_clipboard_preview_and_history() {
    let (mut session, _directory) = saved();
    let stamp = session.prefab_report("", 0, 64, None)["snapshot"]
        .as_str()
        .unwrap()
        .to_string();
    session
        .preview_prefab(Actor::Agent, "Fixture", DVec3::X * 80.0, false, &stamp)
        .unwrap();
    let document = session.document().vmf().clone();
    let rendered = session.render_document().vmf().clone();
    let clipboard = session.clipboard_report();
    let history = session.history_snapshot();
    let camera = session.camera();
    let serials = (
        session.scene_serial(),
        session.view_serial(),
        session.log_serial(),
    );
    let report = session.prefab_thumbnail_report("Fixture");
    assert!(report["error"].is_null(), "{report}");
    assert_eq!(report["brushes"], 1);
    assert_eq!(report["sides"], 6);
    assert_eq!(report["style"], "flat_geometry");
    assert_eq!(report["textures_loaded"], false);
    assert_eq!(report["graphics"]["initialized"], false);
    assert!(session.prefabs.thumbnails.cache.borrow().is_empty());
    assert_eq!(session.document().vmf(), &document);
    assert_eq!(session.render_document().vmf(), &rendered);
    assert_eq!(session.clipboard_report(), clipboard);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(
        (
            session.scene_serial(),
            session.view_serial(),
            session.log_serial()
        ),
        serials
    );
}

#[test]
fn cached_pixels_require_exact_current_bytes_and_epochs_and_disappear_on_source_failure() {
    let (mut session, directory) = saved();
    let image = seed_cache(&session, "Fixture");
    let cached = session.cached_prefab_thumbnail("Fixture").unwrap().unwrap();
    assert!(Arc::ptr_eq(&image, &cached.image));
    assert_eq!(cached.report["cached"], true);
    let path = directory.path().join("Fixture.vmf");
    let modified = path.metadata().unwrap().modified().unwrap();
    let original = std::fs::read_to_string(&path).unwrap();
    let changed = original.replace("BRICK/TEST", "BRICK/BEST");
    assert_ne!(original, changed);
    assert_eq!(original.len(), changed.len());
    std::fs::write(&path, changed).unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(path.metadata().unwrap().len(), original.len() as u64);
    assert_eq!(path.metadata().unwrap().modified().unwrap(), modified);
    assert!(
        session
            .cached_prefab_thumbnail("Fixture")
            .unwrap()
            .is_none()
    );
    assert!(session.prefabs.thumbnails.cache.borrow().is_empty());
    seed_cache(&session, "Fixture");
    session.content_epoch += 1;
    assert!(
        session
            .cached_prefab_thumbnail("Fixture")
            .unwrap()
            .is_none()
    );
    seed_cache(&session, "Fixture");
    session.definitions_serial += 1;
    assert!(
        session
            .cached_prefab_thumbnail("Fixture")
            .unwrap()
            .is_none()
    );
    seed_cache(&session, "Fixture");
    std::fs::remove_file(&path).unwrap();
    assert!(
        session
            .cached_prefab_thumbnail("Fixture")
            .unwrap_err()
            .contains("no longer exists")
    );
    assert!(session.prefabs.thumbnails.cache.borrow().is_empty());
    assert!(!session.prefabs.thumbnails.renderer.initialized());
}

#[test]
fn thumbnail_source_errors_are_explicit_and_never_initialize_graphics() {
    let (mut session, directory) = saved();
    let path = directory.path().join("Fixture.vmf");
    let original = std::fs::read_to_string(&path).unwrap();
    for (text, expected) in [
        ("not a VMF".to_string(), "source"),
        (" ".repeat(MAX_SOURCE_BYTES + 1), "2 MiB"),
        (
            format!(
                "{original}\nentity {{\n\"id\" \"1000\"\n\"classname\" \"func_instance\"\n\"origin\" \"0 0 0\"\n\"file\" \"missing.vmf\"\n}}"
            ),
            "func_instance",
        ),
        (
            format!(
                "{original}\nentity {{\n\"id\" \"1000\"\n\"classname\" \"prop_dynamic\"\n\"origin\" \"0 0 0\"\n\"model\" \"models/missing.mdl\"\n}}"
            ),
            "model",
        ),
    ] {
        std::fs::write(&path, text).unwrap();
        let report = session.prefab_thumbnail_report("Fixture");
        assert!(
            report["error"].as_str().unwrap().contains(expected),
            "{report}"
        );
        assert!(session.prefab_thumbnail("Fixture", false).is_err());
        assert!(session.prefabs.thumbnails.cache.borrow().is_empty());
        assert!(!session.prefabs.thumbnails.renderer.initialized());
    }
    std::fs::write(&path, format!("{original}\nentity {{\n\"id\" \"1000\"\n\"classname\" \"custom_unknown_point\"\n\"origin\" \"80 0 0\"\n}}" )).unwrap();
    let report = session.prefab_thumbnail_report("Fixture");
    assert!(report["error"].is_null(), "{report}");
    assert_eq!(report["point_helpers"], 1);
    assert!(!session.prefabs.thumbnails.renderer.initialized());
}

#[test]
fn thumbnail_flat_scene_uses_real_geometry_without_external_texture_reads() {
    let (session, _directory) = saved();
    let source = session.thumbnail_source("Fixture").unwrap();
    let prepared = session.prepare_prefab_thumbnail(&source).unwrap();
    assert_eq!(prepared.mesh.stats.brushes, 1);
    assert_eq!(prepared.mesh.stats.missing_materials, 0);
    assert!(prepared.mesh.skybox.is_none());
    assert!(prepared.mesh.indices.len() >= 36);
    for id in 0..prepared.materials.len() {
        let material = prepared.materials.get(id);
        assert!(material.base.is_none() && material.blend.is_none());
        assert!(material.error.is_none());
    }
}

#[test]
fn thumbnail_includes_authored_hidden_geometry_and_preflights_brush_work_limits() {
    let (session, directory) = saved();
    let source = session.thumbnail_source("Fixture").unwrap();
    let payload = transfer::Payload::decode(&source.text, &session.definitions).unwrap();
    let mut vmf = payload.document().vmf().clone();
    vmf.world.solids[0].hidden = true;
    let path = directory.path().join("Fixture.vmf");
    std::fs::write(
        &path,
        vmf.write(&source_formats::keyvalues::WriteOptions::default()),
    )
    .unwrap();
    let report = session.prefab_thumbnail_report("Fixture");
    assert!(report["error"].is_null(), "{report}");
    assert_eq!(report["hidden_objects_included"], 1);
    assert!(report["vertices"].as_u64().unwrap() > 0);
    let sides = vmf.world.solids[0].sides.clone();
    vmf.world.solids[0].sides = (0..MAX_BRUSH_SIDES + 1)
        .map(|index| {
            let mut side = sides[index % sides.len()].clone();
            side.id = 1000 + index as u32;
            side
        })
        .collect();
    std::fs::write(
        &path,
        vmf.write(&source_formats::keyvalues::WriteOptions::default()),
    )
    .unwrap();
    let report = session.prefab_thumbnail_report("Fixture");
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .contains("64 sides per brush"),
        "{report}"
    );
    assert!(!session.prefabs.thumbnails.renderer.initialized());
}

#[test]
fn thumbnail_real_models_use_shared_instances_without_texture_assets_or_helper_substitution() {
    let Ok(game) = crate::startup::mount_game(None) else {
        eprintln!("CS:S not installed; skipping");
        return;
    };
    let mut session = Session::new(Ok(game));
    let directory = tempfile::tempdir().unwrap();
    session.prefabs.store = Ok(prefabs::Store {
        directory: directory.path().into(),
    });
    let entity = session
        .create_entity(
            Actor::User,
            "prop_static",
            Some(DVec3::new(64.0, 32.0, 16.0)),
            std::collections::BTreeMap::from([
                (
                    "model".into(),
                    "models/props_junk/wood_crate001a.mdl".into(),
                ),
                ("angles".into(), "0 90 0".into()),
            ]),
        )
        .unwrap();
    let stamp = session.prefab_report("", 0, 64, None)["snapshot"]
        .as_str()
        .unwrap()
        .to_string();
    session
        .save_prefab(Actor::User, "Real model", &[entity], &stamp)
        .unwrap();
    let history = session.history_snapshot();
    let report = session.prefab_thumbnail_report("Real model");
    assert!(report["error"].is_null(), "{report}");
    assert_eq!(report["model_instances"], 1);
    assert_eq!(report["point_helpers"], 0);
    assert!(report["vertices"].as_u64().unwrap() > 24);
    let prepared = session
        .prepare_prefab_thumbnail(&session.thumbnail_source("Real model").unwrap())
        .unwrap();
    assert_eq!(prepared.mesh.stats.model_entities, 1);
    for id in 0..prepared.materials.len() {
        assert!(prepared.materials.get(id).base.is_none());
        assert!(prepared.materials.get(id).blend.is_none());
    }
    assert_eq!(session.history_snapshot(), history);
    assert!(!session.prefabs.thumbnails.renderer.initialized());
}

#[test]
fn thumbnail_framing_ignores_inactive_bodygroup_vertices_and_disabled_rope_batches() {
    let (session, _directory) = saved();
    let source = session.thumbnail_source("Fixture").unwrap();
    let mut prepared = session.prepare_prefab_thumbnail(&source).unwrap();
    let expected = geometry_bounds(&prepared.mesh).unwrap();
    let mut inactive = prepared.mesh.vertices[0];
    inactive.position = [32768.0; 3];
    let inactive_index = prepared.mesh.vertices.len() as u32;
    prepared.mesh.vertices.push(inactive);
    prepared.mesh.maxs = Vec3::splat(32768.0);
    assert_eq!(geometry_bounds(&prepared.mesh).unwrap(), expected);
    let first_index = prepared.mesh.indices.len() as u32;
    prepared.mesh.indices.push(inactive_index);
    prepared.mesh.batches.push(render::scene::Batch {
        material: 0,
        first_index,
        index_count: 1,
        rope: true,
    });
    assert_eq!(geometry_bounds(&prepared.mesh).unwrap(), expected);
    prepared.mesh.batches.last_mut().unwrap().rope = false;
    assert_eq!(
        geometry_bounds(&prepared.mesh).unwrap().1,
        Vec3::splat(32768.0)
    );
}

#[test]
#[ignore = "requires a graphics adapter for actual prefab thumbnail validation"]
fn prefab_thumbnail_gpu_cache_refresh_and_source_changes_preserve_editor_state() {
    let (mut session, directory) = saved();
    let before = session.document().vmf().clone();
    let clipboard = session.clipboard_report();
    let history = session.history_snapshot();
    let camera = session.camera();
    let first = session.prefab_thumbnail("Fixture", false).unwrap();
    assert_eq!(first.image.pixels.len(), (WIDTH * HEIGHT * 4) as usize);
    let colors = first
        .image
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        colors.len() > 3,
        "thumbnail must contain actual shaded geometry"
    );
    let cached = session.prefab_thumbnail("Fixture", false).unwrap();
    assert!(Arc::ptr_eq(&first.image, &cached.image));
    assert_eq!(cached.report["cached"], true);
    let refreshed = session.prefab_thumbnail("Fixture", true).unwrap();
    assert!(!Arc::ptr_eq(&cached.image, &refreshed.image));
    assert_eq!(first.image.pixels, refreshed.image.pixels);
    let source = session.thumbnail_source("Fixture").unwrap();
    let payload = transfer::Payload::decode(&source.text, &session.definitions).unwrap();
    let changed = payload
        .transformed(
            DVec3::ZERO,
            DVec3::new(2.0, 0.5, 1.0),
            DVec3::ZERO,
            &session.definitions,
        )
        .unwrap();
    std::fs::write(directory.path().join("Fixture.vmf"), changed.text()).unwrap();
    let changed = session.prefab_thumbnail("Fixture", false).unwrap();
    assert_ne!(changed.image.pixels, first.image.pixels);
    for index in 0..MAX_CACHE + 2 {
        let name = format!("Cache {index}");
        std::fs::write(directory.path().join(format!("{name}.vmf")), &source.text).unwrap();
        session.prefab_thumbnail(&name, false).unwrap();
    }
    assert_eq!(session.prefabs.thumbnails.cache.borrow().len(), MAX_CACHE);
    assert_eq!(session.document().vmf(), &before);
    assert_eq!(session.clipboard_report(), clipboard);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
}

#[test]
fn thumbnail_point_helpers_share_fgd_geometry_color_and_reload_invalidation() {
    let (mut session, _directory, _) = fixture();
    session.set_definitions(
        Actor::User,
        source_formats::fgd::Catalog::parse(
            "@BaseClass size(-24 -12 0, 40 20 72) color(12 34 56) = sized [] @PointClass base(sized) = sample []",
        )
        .unwrap(),
    );
    let point = session
        .create_entity(
            Actor::User,
            "sample",
            Some(DVec3::new(100.0, 200.0, 300.0)),
            std::collections::BTreeMap::from([("angles".into(), "30 45 60".into())]),
        )
        .unwrap();
    let stamp = session.prefab_report("", 0, 64, None)["snapshot"]
        .as_str()
        .unwrap()
        .to_string();
    session
        .save_prefab(Actor::User, "Sized helper", &[point], &stamp)
        .unwrap();
    let source = session.thumbnail_source("Sized helper").unwrap();
    let document = session.document().vmf().clone();
    let history = session.history_snapshot();
    let prepared = session.prepare_prefab_thumbnail(&source).unwrap();
    assert_eq!(prepared.mesh.stats.point_entities, 1);
    assert_eq!(prepared.mesh.stats.brushes, 0);
    assert_eq!(
        geometry_bounds(&prepared.mesh).unwrap(),
        (
            Vec3::new(76.0, 188.0, 300.0),
            Vec3::new(140.0, 220.0, 372.0)
        )
    );
    assert_eq!(
        prepared.report["bounds"],
        json!([[76.0, 188.0, 300.0], [140.0, 220.0, 372.0]])
    );
    assert!(
        prepared
            .mesh
            .vertices
            .iter()
            .all(|vertex| vertex.color[..3] == [12, 34, 56])
    );
    seed_cache(&session, "Sized helper");
    session.set_definitions(
        Actor::Agent,
        source_formats::fgd::Catalog::parse(
            "@PointClass size(-8 -4 0, 16 12 24) color(100 110 120) = sample []",
        )
        .unwrap(),
    );
    assert!(
        session
            .cached_prefab_thumbnail("Sized helper")
            .unwrap()
            .is_none()
    );
    let reloaded = session.prepare_prefab_thumbnail(&source).unwrap();
    assert_eq!(
        geometry_bounds(&reloaded.mesh).unwrap(),
        (
            Vec3::new(92.0, 196.0, 300.0),
            Vec3::new(116.0, 212.0, 324.0)
        )
    );
    assert!(
        reloaded
            .mesh
            .vertices
            .iter()
            .all(|vertex| vertex.color[..3] == [100, 110, 120])
    );
    assert_eq!(
        session.thumbnail_source("Sized helper").unwrap().text,
        source.text
    );
    assert_eq!(session.document().vmf(), &document);
    assert_eq!(session.history_snapshot(), history);
    assert!(!session.prefabs.thumbnails.renderer.initialized());
}
