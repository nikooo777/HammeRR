use super::*;
use crate::session::Session;
use source_formats::gamefs::MountedPath;

fn archive(path: &std::path::Path) -> source_formats::vpk::Vpk {
    let payload = b"LightmappedGeneric {}";
    let mut tree = b"vmt\0materials/test\0wall\0".to_vec();
    tree.extend(0u32.to_le_bytes());
    tree.extend(0u16.to_le_bytes());
    tree.extend(0x7fffu16.to_le_bytes());
    tree.extend(0u32.to_le_bytes());
    tree.extend((payload.len() as u32).to_le_bytes());
    tree.extend(0xffffu16.to_le_bytes());
    tree.extend(b"\0\0\0");
    let mut bytes = 0x55aa_1234u32.to_le_bytes().to_vec();
    bytes.extend(1u32.to_le_bytes());
    bytes.extend((tree.len() as u32).to_le_bytes());
    bytes.extend(tree);
    bytes.extend(payload);
    std::fs::write(path, bytes).unwrap();
    source_formats::vpk::Vpk::open(path).unwrap()
}

#[test]
fn material_catalog_filters_winners_pages_and_every_maps_path_without_changing_drafts() {
    let directory = tempfile::tempdir().unwrap();
    let higher = directory.path().join("custom");
    let lower = directory.path().join("fallback");
    for root in [&higher, &lower] {
        std::fs::create_dir_all(root.join("materials/test")).unwrap();
        std::fs::write(
            root.join("materials/test/wall.vmt"),
            "LightmappedGeneric {}",
        )
        .unwrap();
    }
    for name in ["maps/demo/wall_wvt_patch", "maps/authored/wall"] {
        let path = higher.join(format!("materials/{name}.vmt"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "LightmappedGeneric {}").unwrap();
    }
    let index = directory.path().join("cstrike_named_custom_dir.vpk");
    let game = GameFs {
        game_dir: higher.clone(),
        base_dir: directory.path().to_owned(),
        mounts: vec![
            MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Directory(higher.clone()),
            },
            MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Pak(archive(&index)),
            },
            MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Directory(lower),
            },
        ],
    };
    let mut session = Session::new(Ok(game));
    session
        .favorite_material("unmounted/favorite", true)
        .unwrap();
    session
        .begin_property_edit(crate::session::PropertyTarget::World)
        .unwrap();
    let draft = session.property_draft().unwrap().token;
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let serials = (
        session.scene_serial(),
        session.view_serial(),
        session.camera_serial(),
    );
    let initial = session
        .material_catalog_filtered("wall", "all", 0, 1, &CatalogFilter::default())
        .unwrap();
    assert_eq!(initial.total, 3);
    assert_eq!(initial.entries.len(), 1);
    let directory_id = initial.sources[0].id.clone();
    let vpk_id = initial.sources[1].id.clone();
    let only_vpk = CatalogFilter {
        source: Some(vpk_id.clone()),
        ..Default::default()
    };
    assert_eq!(
        session
            .material_catalog_filtered("test/wall", "all", 0, 36, &only_vpk)
            .unwrap()
            .total,
        0
    );
    let higher_wall = session
        .material_catalog_filtered("test/wall", "all", 0, 36, &CatalogFilter::default())
        .unwrap();
    assert_eq!(
        higher_wall.entries[0].provenance.source_id.as_deref(),
        Some(directory_id.as_str())
    );
    assert_eq!(
        higher_wall.entries[0]
            .provenance
            .source
            .as_ref()
            .unwrap()
            .kind,
        "directory"
    );
    std::fs::remove_file(higher.join("materials/test/wall.vmt")).unwrap();
    let winner = session
        .material_catalog_filtered("test/wall", "all", 0, 36, &only_vpk)
        .unwrap();
    assert_eq!(winner.total, 1);
    assert_eq!(
        winner.entries[0].provenance.source_id.as_deref(),
        Some(vpk_id.as_str())
    );
    let origin = winner.entries[0].provenance.source.as_ref().unwrap();
    assert_eq!(origin.kind, "vpk");
    assert_eq!(origin.mount, index.to_str().unwrap());
    assert!(
        origin
            .storage_files
            .iter()
            .any(|file| file == index.to_str().unwrap())
    );
    let excluded = CatalogFilter {
        exclude_generated_map_materials: true,
        ..Default::default()
    };
    let filtered = session
        .material_catalog_filtered("wall", "all", 0, 36, &excluded)
        .unwrap();
    assert_eq!(filtered.total, 1);
    assert!(
        filtered
            .groups
            .iter()
            .all(|group| group.id != "folder:maps")
    );
    let unavailable = CatalogFilter {
        source: Some("unavailable".into()),
        ..Default::default()
    };
    let absent = session
        .material_catalog_filtered("", "favorites", 0, 36, &unavailable)
        .unwrap();
    assert_eq!(absent.total, 1);
    assert!(absent.entries[0].provenance.source.is_none());
    assert!(
        session
            .material_catalog_filtered("", "all", 0, 201, &excluded)
            .is_err()
    );
    assert!(
        session
            .material_catalog_filtered(
                "",
                "all",
                0,
                36,
                &CatalogFilter {
                    source: Some("stock".into()),
                    ..Default::default()
                }
            )
            .is_err()
    );
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.property_draft().unwrap().token, draft);
    assert_eq!(
        (
            session.scene_serial(),
            session.view_serial(),
            session.camera_serial()
        ),
        serials
    );
}

#[cfg(unix)]
#[test]
fn source_ids_preserve_raw_mount_bytes_when_display_paths_are_lossy() {
    use std::os::unix::ffi::OsStringExt;
    let origins = [0xfe, 0xff]
        .map(|byte| AssetOrigin::Directory {
            mount: std::ffi::OsString::from_vec(vec![b'/', byte]).into(),
            file: "/file".into(),
        })
        .map(provenance);
    assert_ne!(origins[0].source_id, origins[1].source_id);
    assert_eq!(
        origins[0].source.as_ref().unwrap().mount,
        origins[1].source.as_ref().unwrap().mount
    );
}
