use super::*;

const SHADERS: [&str; 3] = [
    "LightmappedGeneric",
    "WorldVertexTransition",
    "WorldTwoTextureBlend",
];

#[test]
fn primary_diffuse_selection_is_shared_by_the_three_canonical_families() {
    for shader in SHADERS {
        for (suppression, key, expected) in [
            ("", "$albedo", "selected/albedo"),
            (
                "$NoDiffuseBumpLighting \" 0 \"",
                "$albedo",
                "selected/albedo",
            ),
            ("$nodiffusebumplighting 1", "$basetexture", "raw/base"),
        ] {
            let material = Material::parse(&format!("{shader} {{ $BaseTexture raw/base $BumpMap missing/normal $ALBEDO selected/albedo $basetexture2 second/base {suppression} }}")).unwrap();
            let original = material.clone();
            let semantics = material.compile_semantics();
            assert_eq!(material.primary_base_texture_key().unwrap(), Some(key));
            assert_eq!(material.primary_base_texture().unwrap(), Some(expected));
            assert_eq!(material.get("$basetexture2"), Some("second/base"));
            assert_eq!(material.compile_semantics(), semantics);
            assert_eq!(material, original);
        }
    }
}

#[test]
fn missing_declarations_unknown_shaders_and_suppressed_unused_values_keep_optional_base() {
    for (parameters, expected) in [
        ("$basetexture raw/base", Some("raw/base")),
        (
            "$basetexture raw/base $albedo selected/albedo",
            Some("raw/base"),
        ),
        (
            "$basetexture raw/base $bumpmap missing/normal",
            Some("raw/base"),
        ),
        (
            "$basetexture raw/base $bumpmap2 missing/secondary $albedo selected/albedo",
            Some("raw/base"),
        ),
        ("$bumpmap missing/normal $albedo selected/albedo", None),
        (
            "$basetexture raw/base $bumpmap \"\" $albedo \"\" $nodiffusebumplighting 1",
            Some("raw/base"),
        ),
        (
            "$basetexture \"\" $bumpmap \"\" $albedo ../unused $nodiffusebumplighting 1",
            Some(""),
        ),
    ] {
        let material = Material::parse(&format!("LightmappedGeneric {{ {parameters} }}")).unwrap();
        assert_eq!(
            material.primary_base_texture().unwrap(),
            expected,
            "{parameters}"
        );
    }
    for shader in [
        "UnknownShader",
        "LightmappedGeneric_dx9",
        "WorldVertexTransition_dx9",
        "WorldTwoTextureBlend_dx9",
    ] {
        let material = Material::parse(&format!("{shader} {{ $basetexture raw/base $bumpmap normal $albedo \"\" $nodiffusebumplighting true }}")).unwrap();
        assert_eq!(material.primary_base_texture().unwrap(), Some("raw/base"));
    }
}

#[test]
fn active_blank_and_unproven_suppression_declarations_are_explicit_errors() {
    for shader in SHADERS {
        for value in [
            "true", "01", "+1", "1.0", "2", "-1", "0.0", "1e0", "", "NaN",
        ] {
            let material = Material::parse(&format!("{shader} {{ $basetexture raw/base $bumpmap normal $albedo selected/albedo $nodiffusebumplighting \"{value}\" }}")).unwrap();
            assert!(
                material
                    .primary_base_texture()
                    .unwrap_err()
                    .contains("literal 0 or 1")
            );
            assert!(material.editor_textures().is_err());
        }
        for (key, parameters) in [
            (
                "$basetexture",
                "$basetexture \" \" $bumpmap normal $albedo selected/albedo",
            ),
            (
                "$bumpmap",
                "$basetexture raw/base $bumpmap \" \" $albedo selected/albedo",
            ),
            (
                "$albedo",
                "$basetexture raw/base $bumpmap normal $albedo \" \"",
            ),
        ] {
            let material = Material::parse(&format!("{shader} {{ {parameters} }}")).unwrap();
            assert!(material.primary_base_texture().unwrap_err().contains(key));
        }
    }
    for path in ["../outside", "/absolute", "C:\\absolute"] {
        let material = Material::parse(&format!(
            "LightmappedGeneric {{ $basetexture raw/base $bumpmap normal $albedo \"{path}\" }}"
        ))
        .unwrap();
        assert!(
            material
                .primary_base_texture()
                .unwrap_err()
                .contains("$albedo texture path")
        );
    }
}

#[test]
fn display_candidates_keep_tool_priority_without_unused_raw_base_fallback() {
    let material = Material::parse("LightmappedGeneric { $basetexture raw/base $bumpmap normal $albedo selected/albedo %tooltexture editor/tool }").unwrap();
    assert_eq!(
        material.editor_textures().unwrap(),
        ["materials/editor/tool.vtf", "materials/selected/albedo.vtf"]
    );
    let active = Material::parse(
        "LightmappedGeneric { $basetexture raw/base $bumpmap normal $albedo selected/albedo }",
    )
    .unwrap();
    assert_eq!(
        active.editor_textures().unwrap(),
        ["materials/selected/albedo.vtf"]
    );
}

#[test]
fn nested_patch_selection_preserves_resolved_raw_parameters_blocks_and_read_closure() {
    let mut reads = Vec::new();
    let material = Material::load_with_reader("materials/outer.vmt", &mut |path| {
        reads.push(path.to_owned());
        Ok(Some(match path {
            "materials/outer.vmt" => b"Patch { include materials/inner.vmt replace { $albedo selected/albedo $nodiffusebumplighting 0 } }".to_vec(),
            "materials/inner.vmt" => b"Patch { include materials/base.vmt replace { $albedo inherited/albedo } insert { $untouched raw Proxies { AnimatedTexture { animatedtexturevar $basetexture } } } }".to_vec(),
            "materials/base.vmt" => b"WorldVertexTransition { $basetexture raw/base $bumpmap missing/normal $albedo original/albedo $nodiffusebumplighting 1 $basetexture2 second/base }".to_vec(),
            _ => panic!("Unexpected asset read {path}"),
        }))
    }).unwrap().unwrap();
    let original = material.clone();
    assert_eq!(
        reads,
        [
            "materials/outer.vmt",
            "materials/inner.vmt",
            "materials/base.vmt"
        ]
    );
    assert_eq!(
        material.primary_base_texture().unwrap(),
        Some("selected/albedo")
    );
    assert_eq!(material.get("$basetexture"), Some("raw/base"));
    assert_eq!(material.get("$basetexture2"), Some("second/base"));
    assert_eq!(material.get("$untouched"), Some("raw"));
    assert_eq!(material.blocks.len(), 1);
    assert_eq!(material, original);
}
