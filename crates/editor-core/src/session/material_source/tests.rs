use super::*;
use crate::session::Actor;
use source_formats::gamefs::{GameFs, Mount, MountedPath};

fn fixture() -> (tempfile::TempDir, Session) {
    let root = tempfile::tempdir().unwrap();
    let mounts = ["first", "second"]
        .into_iter()
        .map(|name| {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.join("materials/test")).unwrap();
            MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Directory(path),
            }
        })
        .collect();
    let game = GameFs {
        game_dir: root.path().to_owned(),
        base_dir: root.path().to_owned(),
        mounts,
    };
    (root, Session::new(Ok(game)))
}

#[test]
fn material_source_preserves_winning_paths_raw_text_patch_declarations_and_session_state() {
    let (root, mut session) = fixture();
    let selected = root.path().join("first/materials/test/selected.vmt");
    let base = root.path().join("second/materials/test/base.vmt");
    let text = "Patch { include materials/test/base.vmt replace { $basetexture test/replacement } insert { $detail test/detail } }";
    std::fs::write(&selected, text).unwrap();
    std::fs::write(
        &base,
        "UnlitGeneric { $basetexture test/original $bumpmap test/normal }",
    )
    .unwrap();
    let id = session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(64.0),
            "test/selected",
            false,
        )
        .unwrap();
    session
        .preview_transform(
            Actor::Agent,
            &[id],
            map_core::edit::Transform {
                translation: glam::DVec3::X * 32.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let source = session.document.vmf().clone();
    let draft = session.render_document().vmf().clone();
    let snapshot = session.history_snapshot();
    let camera = session.camera();
    let selection = session.selected_ids();
    let scene = session.scene_serial;
    let log = session.log_serial;
    let generation = session.materials.generation();
    let report = session
        .inspect_material_source("MATERIALS\\TEST\\SELECTED.VMT")
        .unwrap();
    assert_eq!(report.material, "test/selected");
    assert_eq!(report.vmt_path, "materials/test/selected.vmt");
    assert_eq!(report.raw_source.as_deref(), Some(text));
    assert_eq!(report.raw_source_bytes, text.len());
    assert!(!report.raw_source_truncated);
    assert!(report.error.is_none());
    assert_eq!(report.files.len(), 2);
    assert_eq!(
        report.files[0].origin.as_ref().unwrap().file.as_deref(),
        selected.to_str()
    );
    assert_eq!(
        report.files[1].origin.as_ref().unwrap().file.as_deref(),
        base.to_str()
    );
    let resolved = report.resolved.unwrap();
    assert_eq!(resolved.shader, "unlitgeneric");
    let textures = resolved
        .texture_parameters
        .iter()
        .map(|texture| (texture.parameter, texture.value.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        textures,
        [
            ("$basetexture", "test/replacement"),
            ("$bumpmap", "test/normal"),
            ("$detail", "test/detail")
        ]
    );
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.render_document().vmf(), &draft);
    assert_eq!(session.history_snapshot(), snapshot);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.selected_ids(), selection);
    assert_eq!(session.scene_serial, scene);
    assert_eq!(session.log_serial, log);
    assert_eq!(session.materials.generation(), generation);
}

#[test]
fn material_source_reports_failures_without_lower_priority_or_display_fallback() {
    let (root, session) = fixture();
    let first = root.path().join("first/materials/test/selected.vmt");
    std::fs::write(
        root.path().join("second/materials/test/selected.vmt"),
        "UnlitGeneric { $basetexture lower_priority }",
    )
    .unwrap();
    for text in [
        b"invalid".as_slice(),
        b"Patch { include materials/test/absent.vmt }",
        b"UnlitGeneric { $basetexture \xff }",
    ] {
        std::fs::write(&first, text).unwrap();
        let report = session.inspect_material_source("test/selected").unwrap();
        assert!(report.error.is_some());
        assert!(report.resolved.is_none());
        assert_eq!(
            report.files[0].origin.as_ref().unwrap().file.as_deref(),
            first.to_str()
        );
        assert!(
            report
                .files
                .iter()
                .filter_map(|file| file.origin.as_ref())
                .all(|origin| origin.mount.ends_with("first"))
        );
        if text.contains(&0xff) {
            assert!(report.error.unwrap().contains("UTF-8"));
            assert!(report.raw_source.is_none());
        } else {
            assert_eq!(report.raw_source.as_deref(), std::str::from_utf8(text).ok());
        }
    }
    std::fs::remove_file(&first).unwrap();
    let report = session.inspect_material_source("test/selected").unwrap();
    assert!(report.error.is_none());
    assert!(
        report.files[0]
            .origin
            .as_ref()
            .unwrap()
            .mount
            .ends_with("second")
    );
    let report = session.inspect_material_source("test/missing").unwrap();
    assert!(report.resolved.is_none());
    assert!(report.error.unwrap().contains("not found"));
    assert!(report.files[0].origin.is_none());
    for name in [
        "",
        "materials/",
        "../outside",
        "/absolute",
        "test/../../outside",
        &"x".repeat(MAX_PATH_BYTES + 1),
    ] {
        assert!(session.inspect_material_source(name).is_err());
    }
}

#[test]
fn material_source_bounds_utf8_source_files_include_chains_and_declarations() {
    let (root, session) = fixture();
    let path = root.path().join("first/materials/test/selected.vmt");
    let text =
        "UnlitGeneric { $basetexture test/exact }\n// ".to_owned() + &"雪".repeat(MAX_SOURCE_BYTES);
    std::fs::write(&path, text.as_bytes()).unwrap();
    let report = session.inspect_material_source("test/selected").unwrap();
    assert!(report.error.is_none());
    assert!(report.raw_source_truncated);
    assert_eq!(report.raw_source_bytes, text.len());
    assert!(report.raw_source.as_ref().unwrap().len() <= MAX_SOURCE_BYTES);
    assert!(text.starts_with(report.raw_source.as_ref().unwrap()));
    std::fs::write(&path, vec![b' '; MAX_FILE_BYTES + 1]).unwrap();
    let report = session.inspect_material_source("test/selected").unwrap();
    assert!(report.error.unwrap().contains("limit"));
    assert!(report.resolved.is_none());
    for index in 0..MAX_FILES {
        let next = if index + 1 == MAX_FILES {
            "UnlitGeneric {}".into()
        } else {
            format!("Patch {{ include materials/test/chain_{}.vmt }}", index + 1)
        };
        std::fs::write(
            root.path()
                .join(format!("first/materials/test/chain_{index}.vmt")),
            next,
        )
        .unwrap();
    }
    let report = session.inspect_material_source("test/chain_0").unwrap();
    assert!(report.error.is_none());
    assert_eq!(report.files.len(), MAX_FILES);
    std::fs::write(
        root.path().join("first/materials/test/chain_8.vmt"),
        "Patch { include materials/test/chain_0.vmt }",
    )
    .unwrap();
    let report = session.inspect_material_source("test/chain_0").unwrap();
    assert!(report.error.unwrap().contains("deeply"));
    assert_eq!(report.files.len(), MAX_FILES);
    std::fs::write(
        &path,
        format!(
            "{} {{ $basetexture {} }}",
            "s".repeat(MAX_SHADER_CHARS + 1),
            "t".repeat(MAX_TEXTURE_CHARS + 1)
        ),
    )
    .unwrap();
    let resolved = session
        .inspect_material_source("test/selected")
        .unwrap()
        .resolved
        .unwrap();
    assert!(resolved.shader_truncated);
    assert!(resolved.texture_parameters[0].truncated);
    assert_eq!(resolved.shader.len(), MAX_SHADER_CHARS);
    assert_eq!(
        resolved.texture_parameters[0].value.len(),
        MAX_TEXTURE_CHARS
    );
}

#[test]
fn material_source_enforces_total_read_budget_across_patch_files() {
    let (root, session) = fixture();
    for index in 0..5 {
        let mut text = if index == 4 {
            "UnlitGeneric {}".into()
        } else {
            format!(
                "Patch {{ include materials/test/budget_{}.vmt }}",
                index + 1
            )
        };
        text.push_str(&" ".repeat(MAX_FILE_BYTES - text.len()));
        std::fs::write(
            root.path()
                .join(format!("first/materials/test/budget_{index}.vmt")),
            text,
        )
        .unwrap();
    }
    let report = session.inspect_material_source("test/budget_0").unwrap();
    assert!(report.error.unwrap().contains("limit"));
    assert!(report.resolved.is_none());
    assert_eq!(
        report.files.iter().map(|file| file.bytes).sum::<usize>(),
        MAX_TOTAL_BYTES
    );
    assert_eq!(report.files.len(), 4);
}

#[test]
#[cfg(unix)]
fn material_source_refuses_dangling_higher_priority_paths_instead_of_showing_lower_source() {
    use std::os::unix::fs::symlink;
    let (root, session) = fixture();
    std::fs::write(
        root.path().join("second/materials/test/selected.vmt"),
        "UnlitGeneric { $basetexture wrong_mount }",
    )
    .unwrap();
    let first = root.path().join("first/materials/test/selected.vmt");
    symlink("missing.vmt", &first).unwrap();
    let report = session.inspect_material_source("test/selected").unwrap();
    assert!(report.error.unwrap().contains("dangling"));
    assert!(report.resolved.is_none());
    assert!(report.raw_source.is_none());
    std::fs::remove_file(&first).unwrap();
    std::fs::remove_dir_all(root.path().join("first/materials")).unwrap();
    symlink("missing-directory", root.path().join("first/materials")).unwrap();
    let report = session.inspect_material_source("test/selected").unwrap();
    assert!(report.error.unwrap().contains("dangling"));
    assert!(report.resolved.is_none());
}
