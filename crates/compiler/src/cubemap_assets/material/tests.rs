use super::*;
use source_formats::gamefs::{Mount, MountedPath};

fn game(directory: &std::path::Path) -> GameFs {
    GameFs {
        game_dir: directory.to_owned(),
        base_dir: directory.to_owned(),
        mounts: vec![MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.to_owned()),
        }],
    }
}

#[test]
fn material_and_patch_parsing_use_recorded_bytes_after_source_replacement() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    let base = "LightmappedGeneric { $envmap env_cubemap $bottommaterial base $unknown recorded Proxies { TextureScroll { textureScrollRate 0.2 } } }";
    let patch = "Patch { include materials/base.vmt replace { $bottommaterial PATCH $unknown patched } insert { $surfaceprop ice } }";
    for patched in [false, true] {
        std::fs::write(directory.path().join("materials/base.vmt"), base).unwrap();
        std::fs::write(directory.path().join("materials/patch.vmt"), patch).unwrap();
        let game = game(directory.path());
        let control = Control::default();
        let mut graph = Graph::new(&game, &control);
        let name = if patched { "patch" } else { "base" };
        graph
            .record_patch_chain(&vmt::material_path(name), 0, &mut BTreeSet::new())
            .unwrap();
        std::fs::write(
            directory.path().join("materials/base.vmt"),
            "UnlitGeneric { $unknown replacement }",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("materials/patch.vmt"),
            "Patch { include materials/missing.vmt }",
        )
        .unwrap();
        let material = graph.load(name).unwrap();
        assert_eq!(material.shader, "lightmappedgeneric");
        assert_eq!(material.get("$envmap"), Some("env_cubemap"));
        assert_eq!(
            material.get("$unknown"),
            Some(if patched { "patched" } else { "recorded" })
        );
        assert_eq!(material.get("$surfaceprop"), patched.then_some("ice"));
        assert_eq!(material.blocks.len(), 1);
        assert_eq!(material.blocks[0].key, "Proxies");
        assert!(graph.needs_alias(name).unwrap());
        assert_eq!(graph.sources["materials/base.vmt"], hash(base.as_bytes()));
        if patched {
            assert_eq!(graph.sources["materials/patch.vmt"], hash(patch.as_bytes()));
        }
    }
    for (first, second) in [
        (
            "LightmappedGeneric { $fallbackmaterial cycle_a }",
            "UnlitGeneric {}",
        ),
        (
            "LightmappedGeneric { $crackmaterial cycle_a }",
            "UnlitGeneric {}",
        ),
        (
            "LightmappedGeneric { $bottommaterial cycle_b }",
            "LightmappedGeneric { $bottommaterial cycle_a }",
        ),
        (
            "LightmappedGeneric { $bottommaterial materials/cycle_a.vmt }",
            "UnlitGeneric {}",
        ),
        ("Patch { include materials/cycle_a.vmt }", "UnlitGeneric {}"),
    ] {
        std::fs::write(directory.path().join("materials/cycle_a.vmt"), first).unwrap();
        std::fs::write(directory.path().join("materials/cycle_b.vmt"), second).unwrap();
        assert!(
            Graph::new(&game(directory.path()), &Control::default())
                .needs_alias("cycle_a")
                .unwrap_err()
                .contains("cycle/depth")
        );
    }
}

fn manifest(graph: &Graph<'_>) -> CubemapAssetManifest {
    CubemapAssetManifest {
        version: 1,
        phase: CubemapAssetPhase::Pending,
        logical_namespace: "lookup".into(),
        namespace: "lookup".into(),
        channel: "ldr".into(),
        samples: Vec::new(),
        hdr_samples: Vec::new(),
        assignments: Vec::new(),
        aliases: Vec::new(),
        sources: graph
            .sources
            .iter()
            .map(|(path, sha256)| CubemapSourceAsset {
                path: path.clone(),
                sha256: sha256.clone(),
            })
            .collect(),
        missing_sources: graph.missing_sources.iter().cloned().collect(),
        default_texture: None,
        hdr_default_texture: None,
        texinfo_bindings: Vec::new(),
        face_bindings: Vec::new(),
    }
}

#[test]
fn earlier_model_material_appearance_rejects_source_and_metadata_guards() {
    let directory = tempfile::tempdir().unwrap();
    for path in ["materials/first", "materials/second"] {
        std::fs::create_dir_all(directory.path().join(path)).unwrap();
    }
    let fallback = directory.path().join("materials/second/model.vmt");
    let bytes = b"LightmappedGeneric { $unknown fallback }";
    std::fs::write(&fallback, bytes).unwrap();
    let game = game(directory.path());
    let control = Control::default();
    let mut graph = Graph::new(&game, &control);
    assert!(
        graph
            .record_model_material("model", &["first".into(), "second".into()])
            .unwrap()
    );
    assert_eq!(
        graph.missing_sources,
        BTreeSet::from(["materials/first/model.vmt".into()])
    );
    let manifest = manifest(&graph);
    let before = capture_reflection_source_guard(&manifest, &game, &control).unwrap();
    super::super::finalize::verify_sources(&manifest, &game, &control).unwrap();
    std::fs::write(
        directory.path().join("materials/first/model.vmt"),
        "UnlitGeneric { $unknown higher_priority }",
    )
    .unwrap();
    assert_eq!(std::fs::read(fallback).unwrap(), bytes);
    assert!(before.verify_metadata().is_err());
    assert!(
        capture_reflection_source_guard(&manifest, &game, &control)
            .unwrap_err()
            .to_string()
            .contains("candidate appeared")
    );
    assert!(
        super::super::finalize::verify_sources(&manifest, &game, &control)
            .unwrap_err()
            .to_string()
            .contains("candidate appeared")
    );
}
