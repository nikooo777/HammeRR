use super::*;
use crate::session::prefabs::thumbnails::tests::{cache_count, clear_cache, seed_style_cache};
use crate::session::{Actor, prefabs::fixtures::fixture};

fn saved(count: usize) -> (Session, tempfile::TempDir, map_core::ObjectId) {
    let (mut session, directory, brush) = fixture();
    for index in 0..count {
        let stamp = session.prefab_snapshot().unwrap().1;
        session
            .save_prefab(
                Actor::User,
                &format!("Assembly {index:02}"),
                &[brush],
                &stamp,
            )
            .unwrap();
    }
    (session, directory, brush)
}

#[test]
fn bounded_cached_gallery_uses_exact_page_and_preserves_document_clipboard_and_lazy_graphics() {
    let (mut session, _directory, brush) = saved(10);
    for index in 2..10 {
        seed_style_cache(
            &session,
            &format!("Assembly {index:02}"),
            ThumbnailStyle::Flat,
        );
    }
    session
        .capture_clipboard(
            Actor::Agent,
            &[brush],
            &session.clipboard_stamp(),
            false,
            false,
        )
        .unwrap();
    session
        .preview_paste(
            map_core::transfer::Options::default(),
            &session.clipboard_stamp(),
        )
        .unwrap();
    let source = session.document().vmf().clone();
    let rendered = session.render_document().vmf().clone();
    let history = session.history_snapshot();
    let clipboard = session.clipboard_report();
    let camera = session.camera();
    let serials = (
        session.scene_serial(),
        session.view_serial(),
        session.camera_serial(),
    );
    let stamp = session.prefab_snapshot().unwrap().1;
    let gallery = session
        .render_prefab_gallery(
            "ASSEMBLY",
            2,
            MAX_GALLERY_ENTRIES,
            ThumbnailStyle::Flat,
            &stamp,
        )
        .unwrap();
    assert_eq!(gallery.entries.len(), MAX_GALLERY_ENTRIES);
    assert_eq!(gallery.entries[0].name, "Assembly 02");
    assert_eq!(gallery.entries[7].name, "Assembly 09");
    assert!(
        gallery
            .entries
            .iter()
            .all(|entry| entry.thumbnail.as_ref().unwrap().report["cached"] == true)
    );
    assert_eq!(gallery.report["total"], 10);
    assert!(gallery.report["next_offset"].is_null());
    session
        .validate_prefab_gallery(
            &gallery.review,
            "assembly",
            2,
            MAX_GALLERY_ENTRIES,
            ThumbnailStyle::Flat,
        )
        .unwrap();
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.render_document().vmf(), &rendered);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.clipboard_report(), clipboard);
    assert_eq!(session.camera(), camera);
    assert_eq!(
        (
            session.scene_serial(),
            session.view_serial(),
            session.camera_serial()
        ),
        serials
    );
    assert_eq!(
        session.prefab_thumbnail_report("Assembly 02")["graphics"]["initialized"],
        false
    );
    assert_eq!(cache_count(&session), MAX_GALLERY_ENTRIES);
}

#[test]
fn invalid_limits_and_stale_reviews_reject_before_any_graphics_or_cache_changes() {
    let (mut session, directory, _) = saved(1);
    let stamp = session.prefab_snapshot().unwrap().1;
    let serial = session.prefab_thumbnail_serial();
    for limit in [0, MAX_GALLERY_ENTRIES + 1, usize::MAX] {
        assert!(
            session
                .render_prefab_gallery("", 0, limit, ThumbnailStyle::Flat, &stamp)
                .is_err()
        );
    }
    std::fs::write(directory.path().join("External.vmf"), "malformed").unwrap();
    assert!(
        session
            .render_prefab_gallery("", 0, MAX_GALLERY_ENTRIES, ThumbnailStyle::Flat, &stamp)
            .is_err()
    );
    assert_eq!(session.prefab_thumbnail_serial(), serial);
    assert_eq!(
        session.prefab_thumbnail_report("Assembly 00")["graphics"]["initialized"],
        false
    );
}

#[test]
fn unsupported_entries_return_per_entry_errors_without_flat_substitution_or_graphics() {
    let (mut session, directory, _) = saved(1);
    std::fs::write(directory.path().join("Broken.vmf"), "invalid source").unwrap();
    let stamp = session.prefab_snapshot().unwrap().1;
    let gallery = session
        .render_prefab_gallery("", 0, MAX_GALLERY_ENTRIES, ThumbnailStyle::Textured, &stamp)
        .unwrap();
    assert_eq!(gallery.entries.len(), 2);
    assert!(gallery.entries.iter().all(|entry| entry.thumbnail.is_err()));
    assert!(
        gallery.report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["style"] == "textured" && entry["error"].is_string())
    );
    assert_eq!(
        session.prefab_thumbnail_report("Assembly 00")["graphics"]["initialized"],
        false
    );
}

#[test]
fn mixed_gallery_preserves_order_and_successful_siblings_without_error_images() {
    let (mut session, directory, _) = saved(2);
    for index in 0..2 {
        seed_style_cache(
            &session,
            &format!("Assembly {index:02}"),
            ThumbnailStyle::Flat,
        );
    }
    std::fs::write(directory.path().join("A broken.vmf"), "invalid source").unwrap();
    let stamp = session.prefab_snapshot().unwrap().1;
    let gallery = session
        .render_prefab_gallery("", 0, MAX_GALLERY_ENTRIES, ThumbnailStyle::Flat, &stamp)
        .unwrap();
    assert_eq!(gallery.entries[0].name, "A broken");
    assert!(gallery.entries[0].thumbnail.is_err());
    assert!(gallery.entries[1].thumbnail.is_ok());
    assert!(gallery.entries[2].thumbnail.is_ok());
    assert_eq!(
        gallery.report["entries"][0]["name"],
        gallery.entries[0].name
    );
    assert!(gallery.report["entries"][0]["error"].is_string());
    assert_eq!(gallery.report["entries"][1]["cached"], true);
    session
        .validate_prefab_gallery(
            &gallery.review,
            "",
            0,
            MAX_GALLERY_ENTRIES,
            ThumbnailStyle::Flat,
        )
        .unwrap();
}

#[test]
fn textured_gallery_rechecks_exact_assets_while_flat_review_stays_independent() {
    let (mut session, _library, assets, brush) = crate::test_assets::textured_prefabs();
    let stamp = session.prefab_snapshot().unwrap().1;
    session
        .save_prefab(Actor::User, "Texture", &[brush], &stamp)
        .unwrap();
    seed_style_cache(&session, "Texture", ThumbnailStyle::Flat);
    seed_style_cache(&session, "Texture", ThumbnailStyle::Textured);
    let stamp = session.prefab_snapshot().unwrap().1;
    let flat = session
        .render_prefab_gallery("", 0, 1, ThumbnailStyle::Flat, &stamp)
        .unwrap();
    let textured = session
        .render_prefab_gallery("", 0, 1, ThumbnailStyle::Textured, &stamp)
        .unwrap();
    let path = assets.path().join("materials/test/checker.vtf");
    let modified = path.metadata().unwrap().modified().unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[80] ^= 0xff;
    std::fs::write(&path, bytes).unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let serial = session.prefab_thumbnail_serial();
    assert!(
        session
            .validate_prefab_gallery(&textured.review, "", 0, 1, ThumbnailStyle::Textured)
            .is_err()
    );
    assert!(session.prefab_thumbnail_serial() > serial);
    session
        .validate_prefab_gallery(&flat.review, "", 0, 1, ThumbnailStyle::Flat)
        .unwrap();
    assert_eq!(
        session.prefab_thumbnail_report("Texture")["graphics"]["initialized"],
        false
    );
}

#[test]
fn gallery_review_binds_page_style_epochs_exact_source_and_cache_publication() {
    let (mut session, directory, _) = saved(1);
    seed_style_cache(&session, "Assembly 00", ThumbnailStyle::Flat);
    let stamp = session.prefab_snapshot().unwrap().1;
    let gallery = session
        .render_prefab_gallery("", 0, 1, ThumbnailStyle::Flat, &stamp)
        .unwrap();
    for (query, offset, limit, style) in [
        ("different", 0, 1, ThumbnailStyle::Flat),
        ("", 1, 1, ThumbnailStyle::Flat),
        ("", 0, 2, ThumbnailStyle::Flat),
        ("", 0, 1, ThumbnailStyle::Textured),
    ] {
        assert!(
            session
                .validate_prefab_gallery(&gallery.review, query, offset, limit, style)
                .is_err()
        );
    }
    session.content_epoch += 1;
    assert!(
        session
            .validate_prefab_gallery(&gallery.review, "", 0, 1, ThumbnailStyle::Flat)
            .is_err()
    );
    session.content_epoch -= 1;
    session.definitions_serial += 1;
    assert!(
        session
            .validate_prefab_gallery(&gallery.review, "", 0, 1, ThumbnailStyle::Flat)
            .is_err()
    );
    session.definitions_serial -= 1;
    clear_cache(&session);
    seed_style_cache(&session, "Assembly 00", ThumbnailStyle::Flat);
    assert!(
        session
            .validate_prefab_gallery(&gallery.review, "", 0, 1, ThumbnailStyle::Flat)
            .unwrap_err()
            .contains("image changed")
    );
    let current = session
        .render_prefab_gallery("", 0, 1, ThumbnailStyle::Flat, &stamp)
        .unwrap();
    let path = directory.path().join("Assembly 00.vmf");
    let modified = path.metadata().unwrap().modified().unwrap();
    let original = std::fs::read_to_string(&path).unwrap();
    let changed = original.replace("BRICK/TEST", "BRICK/BEST");
    assert_ne!(changed, original);
    assert_eq!(changed.len(), original.len());
    std::fs::write(&path, changed).unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert!(
        session
            .validate_prefab_gallery(&current.review, "", 0, 1, ThumbnailStyle::Flat)
            .is_err()
    );
}

#[test]
fn peer_catalog_change_invalidates_the_entire_page_without_touching_local_paste() {
    let (mut session, directory, brush) = saved(1);
    let authority = crate::prefabs::Authority::from_directory(Ok(directory.path().into()));
    session.use_application_prefabs(authority.clone());
    let mut peer = Session::with_prefabs(Err("No assets required".into()), authority);
    seed_style_cache(&session, "Assembly 00", ThumbnailStyle::Flat);
    let stamp = session.prefab_snapshot().unwrap().1;
    let gallery = session
        .render_prefab_gallery("", 0, 1, ThumbnailStyle::Flat, &stamp)
        .unwrap();
    session
        .capture_clipboard(
            Actor::User,
            &[brush],
            &session.clipboard_stamp(),
            false,
            false,
        )
        .unwrap();
    session
        .preview_paste(
            map_core::transfer::Options::default(),
            &session.clipboard_stamp(),
        )
        .unwrap();
    let clipboard = session.clipboard_report();
    let stamp = peer.prefab_report("", 0, 8, None)["snapshot"]
        .as_str()
        .unwrap()
        .to_string();
    peer.rename_prefab(Actor::Agent, "Assembly 00", "Renamed", &stamp)
        .unwrap();
    assert!(
        session
            .validate_prefab_gallery(&gallery.review, "", 0, 1, ThumbnailStyle::Flat)
            .is_err()
    );
    assert_eq!(session.clipboard_report(), clipboard);
}

#[test]
fn late_catalog_change_discards_prepared_page_and_preserves_the_previous_paste() {
    let (mut session, directory, brush) = saved(1);
    let authority = crate::prefabs::Authority::from_directory(Ok(directory.path().into()));
    session.use_application_prefabs(authority.clone());
    let mut peer = Session::with_prefabs(Err("No assets required".into()), authority);
    session
        .capture_clipboard(
            Actor::User,
            &[brush],
            &session.clipboard_stamp(),
            false,
            false,
        )
        .unwrap();
    session
        .preview_paste(
            map_core::transfer::Options::default(),
            &session.clipboard_stamp(),
        )
        .unwrap();
    let clipboard = session.clipboard_report();
    let rendered = session.render_document().vmf().clone();
    seed_style_cache(&session, "Assembly 00", ThumbnailStyle::Flat);
    let stamp = session.prefab_snapshot().unwrap().1;
    let gallery = session
        .render_prefab_gallery("", 0, 1, ThumbnailStyle::Flat, &stamp)
        .unwrap();
    let peer_stamp = peer.prefab_report("", 0, 8, None)["snapshot"]
        .as_str()
        .unwrap()
        .to_string();
    peer.rename_prefab(Actor::Agent, "Assembly 00", "Renamed", &peer_stamp)
        .unwrap();
    assert!(
        session
            .finish_prefab_gallery(gallery.entries, gallery.review, 1, &stamp)
            .is_err()
    );
    assert_eq!(session.clipboard_report(), clipboard);
    assert_eq!(session.render_document().vmf(), &rendered);
}

#[test]
fn late_exact_source_change_omits_stale_pixels_and_retains_valid_sibling_images() {
    let (mut session, directory, _) = saved(2);
    for index in 0..2 {
        seed_style_cache(
            &session,
            &format!("Assembly {index:02}"),
            ThumbnailStyle::Flat,
        );
    }
    let stamp = session.prefab_snapshot().unwrap().1;
    let gallery = session
        .render_prefab_gallery("", 0, 2, ThumbnailStyle::Flat, &stamp)
        .unwrap();
    let path = directory.path().join("Assembly 00.vmf");
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
    let finished = session
        .finish_prefab_gallery(gallery.entries, gallery.review, 2, &stamp)
        .unwrap();
    assert!(finished.entries[0].thumbnail.is_err());
    assert!(finished.entries[1].thumbnail.is_ok());
    assert!(finished.report["entries"][0]["error"].is_string());
    assert_eq!(
        finished.report["registry_revision"],
        gallery.report["registry_revision"]
    );
}
