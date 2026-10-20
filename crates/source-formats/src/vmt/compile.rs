use crate::bsp::{contents, surf};

use super::Material;

pub(super) fn is_lightmapped_shader(shader: &str) -> bool {
    matches!(
        shader,
        "lightmappedgeneric" | "worldvertextransition" | "worldtwotextureblend"
    )
}

fn diffuse_bump_parameter(material: &Material) -> Option<&str> {
    if is_lightmapped_shader(&material.shader) {
        material.get("$nodiffusebumplighting").map(str::trim)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompileSemantics {
    pub contents: i32,
    pub surface: i32,
    pub draws: bool,
    pub hint: bool,
    pub skip: bool,
    pub unsupported_flags: Vec<String>,
}

impl Default for CompileSemantics {
    fn default() -> Self {
        Self {
            contents: contents::SOLID,
            surface: 0,
            draws: true,
            hint: false,
            skip: false,
            unsupported_flags: Vec::new(),
        }
    }
}

impl CompileSemantics {
    pub fn lit(&self) -> bool {
        self.draws && self.surface & (surf::NOLIGHT | surf::SKY | surf::SKY2D) == 0
    }

    pub fn checked_from_material(material: &Material) -> Result<Self, String> {
        let semantics = Self::from_material(material);
        if semantics.lit()
            && material.get("$bumpmap").is_some()
            && diffuse_bump_parameter(material).is_some_and(|value| !matches!(value, "0" | "1"))
        {
            return Err("Unsupported $nodiffusebumplighting on a lit primary-normal material; use literal 0 or 1".into());
        }
        Ok(semantics)
    }

    pub fn from_material(m: &Material) -> Self {
        let mut semantics = Self::default();
        let flag = |key: &str| m.compile_flag(key);
        let hide = |semantics: &mut Self| {
            semantics.draws = false;
            semantics.surface |= surf::NODRAW | surf::NOLIGHT;
        };

        if flag("%compilenodraw") {
            hide(&mut semantics);
        }
        let mut clip = 0;
        if flag("%compileclip") {
            clip |= contents::PLAYERCLIP | contents::MONSTERCLIP;
        }
        if flag("%playerclip") || flag("%compileplayerclip") {
            clip |= contents::PLAYERCLIP;
        }
        if flag("%compilenpcclip") {
            clip |= contents::MONSTERCLIP;
        }
        if clip != 0 {
            semantics.contents = clip;
            hide(&mut semantics);
        }
        if flag("%compiletrigger") {
            semantics.surface |= surf::TRIGGER;
            hide(&mut semantics);
        }
        if flag("%compilesky") {
            semantics.surface |= surf::SKY | surf::NOLIGHT;
        }
        if flag("%compile2dsky") {
            semantics.surface |= surf::SKY | surf::SKY2D | surf::NOLIGHT;
        }
        if flag("%compilehint") {
            semantics.hint = true;
            semantics.contents = 0;
            semantics.surface |= surf::HINT;
            hide(&mut semantics);
        }
        if flag("%compileskip") {
            semantics.skip = true;
            semantics.contents = 0;
            semantics.surface |= surf::SKIP;
            hide(&mut semantics);
        }
        if flag("%compileorigin") {
            semantics.contents = contents::ORIGIN;
            hide(&mut semantics);
        }
        if flag("%compileareaportal") {
            semantics.contents = contents::AREAPORTAL;
            hide(&mut semantics);
        }
        if flag("%compileladder") {
            semantics.contents = contents::LADDER;
            hide(&mut semantics);
        }
        if flag("%compilefog") {
            semantics.contents = 0;
            hide(&mut semantics);
        }
        if flag("%compilewater") || m.shader == "water" {
            semantics.contents = contents::WATER;
            semantics.surface |= surf::WARP | surf::NOLIGHT;
        }
        if flag("%compileslime") {
            semantics.contents = contents::SLIME;
        }
        if flag("%compileblocklos") {
            semantics.contents = contents::BLOCKLOS;
        }
        if flag("%compilenonsolid") {
            semantics.contents = contents::OPAQUE;
        }
        if flag("%compiledetail") && semantics.contents != 0 {
            semantics.contents |= contents::DETAIL;
        }
        if flag("%compilenolight") || m.shader == "unlitgeneric" {
            semantics.surface |= surf::NOLIGHT;
        }
        let alpha = m.get("$alpha").and_then(|v| v.trim().parse::<f64>().ok());
        let translucent = m.translucent() || alpha.is_some_and(|a| a < 1.0);
        if semantics.draws
            && !flag("%compilenonsolid")
            && !flag("%compileblocklos")
            && (translucent || m.alpha_test())
        {
            if translucent {
                semantics.surface |= surf::TRANS;
            }
            if semantics.contents & contents::SOLID != 0 {
                semantics.contents = (semantics.contents & !contents::SOLID) | contents::WINDOW;
            }
            semantics.contents |= contents::TRANSLUCENT;
        }
        let diffuse_bump_disabled = diffuse_bump_parameter(m) == Some("1");
        if semantics.lit() && m.get("$bumpmap").is_some() && !diffuse_bump_disabled {
            semantics.surface |= surf::BUMPLIGHT;
        }
        const HANDLED: &[&str] = &[
            "%compilenodraw",
            "%compileclip",
            "%compileplayerclip",
            "%compilenpcclip",
            "%compiletrigger",
            "%compilesky",
            "%compile2dsky",
            "%compilehint",
            "%compileskip",
            "%compileorigin",
            "%compileareaportal",
            "%compileladder",
            "%compilewater",
            "%compileslime",
            "%compileblocklos",
            "%compilenonsolid",
            "%compiledetail",
            "%compilenolight",
        ];
        semantics.unsupported_flags = m
            .params
            .iter()
            .filter(|(key, _)| {
                key.starts_with("%compile") && flag(key) && !HANDLED.contains(&key.as_str())
            })
            .map(|(key, _)| key.clone())
            .collect();
        semantics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_primary_normal_admission_refuses_unproven_values_without_changing_preview() {
        for shader in [
            "LightmappedGeneric",
            "WorldVertexTransition",
            "WorldTwoTextureBlend",
        ] {
            for value in [
                "2", "true", "1.0", "", " ", "01", "+1", "-1", "NaN", "inf", "0.0",
            ] {
                let material = Material::parse(&format!(
                    "{shader} {{ $bumpmap keep/primary $nodiffusebumplighting \"{value}\" }}"
                ))
                .unwrap();
                let error = CompileSemantics::checked_from_material(&material).unwrap_err();
                assert!(
                    error.contains("$nodiffusebumplighting") && error.contains("literal 0 or 1")
                );
                assert_ne!(material.compile_semantics().surface & surf::BUMPLIGHT, 0);
                assert_eq!(material.get("$bumpmap"), Some("keep/primary"));
            }
            for parameter in [
                "",
                "$nodiffusebumplighting 0",
                "$nodiffusebumplighting 1",
                "$nodiffusebumplighting \" 1 \"",
            ] {
                let material =
                    Material::parse(&format!("{shader} {{ $bumpmap keep/primary {parameter} }}"))
                        .unwrap();
                assert_eq!(
                    CompileSemantics::checked_from_material(&material).unwrap(),
                    material.compile_semantics()
                );
            }
        }
        for text in [
            "LightmappedGeneric { %compilenolight 1 $bumpmap primary $nodiffusebumplighting 2 }",
            "LightmappedGeneric { %compilenodraw 1 $bumpmap primary $nodiffusebumplighting true }",
            "LightmappedGeneric { %compilesky 1 $bumpmap primary $nodiffusebumplighting 1.0 }",
            "WorldVertexTransition { $bumpmap2 secondary $nodiffusebumplighting 2 }",
            "WorldTwoTextureBlend { $nodiffusebumplighting true }",
            "UnknownShader { $bumpmap primary $nodiffusebumplighting 2 }",
        ] {
            let material = Material::parse(text).unwrap();
            assert_eq!(
                CompileSemantics::checked_from_material(&material).unwrap(),
                material.compile_semantics()
            );
        }
        let large = Material::parse(&format!(
            "LightmappedGeneric {{ $bumpmap primary $nodiffusebumplighting \"{}\" }}",
            "x".repeat(8192)
        ))
        .unwrap();
        assert!(
            CompileSemantics::checked_from_material(&large)
                .unwrap_err()
                .len()
                < 160
        );
        let empty_primary =
            Material::parse("LightmappedGeneric { $bumpmap \"\" $nodiffusebumplighting 2 }")
                .unwrap();
        assert!(CompileSemantics::checked_from_material(&empty_primary).is_err());
    }

    #[test]
    fn checked_admission_uses_final_patch_value_and_allows_a_repaired_override() {
        for (base_value, patch_value, accepted) in
            [("0", "2", false), ("2", "0", true), ("2", "1", true)]
        {
            let base = format!(
                "WorldVertexTransition {{ $bumpmap keep/primary $nodiffusebumplighting {base_value} }}"
            );
            let patch = format!(
                "Patch {{ include materials/base.vmt replace {{ $nodiffusebumplighting {patch_value} }} }}"
            );
            let material = Material::load_with_reader("materials/patch.vmt", &mut |path| {
                Ok(Some(if path == "materials/base.vmt" {
                    base.as_bytes().to_vec()
                } else {
                    patch.as_bytes().to_vec()
                }))
            })
            .unwrap()
            .unwrap();
            assert_eq!(
                CompileSemantics::checked_from_material(&material).is_ok(),
                accepted
            );
            assert_eq!(material.get("$nodiffusebumplighting"), Some(patch_value));
        }
    }

    #[test]
    fn canonical_diffuse_bump_suppression_changes_only_the_checked_surface_bit() {
        for shader in [
            "LightmappedGeneric",
            "WorldVertexTransition",
            "WorldTwoTextureBlend",
            "UnknownShader",
        ] {
            let ordinary = Material::parse(&format!(
                "{shader} {{ $translucent 1 %compiledetail 1 $surfaceprop concrete }}"
            ))
            .unwrap()
            .compile_semantics();
            for value in [
                None,
                Some("0"),
                Some("1"),
                Some(" 1 "),
                Some("true"),
                Some("01"),
                Some("+1"),
                Some("1.0"),
                Some("2"),
                Some("-1"),
            ] {
                let parameter = value.map_or(String::new(), |value| {
                    format!("$NoDiffuseBumpLighting \"{value}\"")
                });
                let material = Material::parse(&format!(
                    "{shader} {{ $translucent 1 %compiledetail 1 $surfaceprop concrete $bumpmap keep/primary $bumpmap2 keep/secondary {parameter} }}"
                ))
                .unwrap();
                let mut expected = ordinary.clone();
                if shader == "UnknownShader" || !value.is_some_and(|value| value.trim() == "1") {
                    expected.surface |= surf::BUMPLIGHT;
                }
                assert_eq!(
                    material.compile_semantics(),
                    expected,
                    "{shader}: {value:?}"
                );
                assert_eq!(material.get("$bumpmap"), Some("keep/primary"));
                assert_eq!(material.get("$bumpmap2"), Some("keep/secondary"));
            }
        }
        for text in [
            "UnlitGeneric { $bumpmap primary $nodiffusebumplighting 0 }",
            "LightmappedGeneric { %compilenodraw 1 $bumpmap primary }",
            "LightmappedGeneric { %compilesky 1 $bumpmap primary }",
            "WorldVertexTransition { $bumpmap2 secondary $nodiffusebumplighting 0 }",
            "WorldVertexTransition { $bumpmap2 secondary $nodiffusebumplighting 1 }",
        ] {
            let material = Material::parse(text).unwrap();
            assert_eq!(
                material.compile_semantics().surface & surf::BUMPLIGHT,
                0,
                "{text}"
            );
        }
    }

    #[test]
    fn resolved_patch_insert_replace_and_inheritance_drive_diffuse_bump_admission() {
        for shader in [
            "LightmappedGeneric",
            "WorldVertexTransition",
            "WorldTwoTextureBlend",
        ] {
            for (section, base_value, patch_value, expected) in [
                ("insert", None, "1", Some("1")),
                ("insert", Some("0"), "1", Some("1")),
                ("replace", None, "1", None),
                ("replace", Some("0"), "1", Some("1")),
                ("replace", Some("1"), "0", Some("0")),
            ] {
                let base_parameter = base_value.map_or(String::new(), |value| {
                    format!("$nodiffusebumplighting {value}")
                });
                let base = format!(
                    "{shader} {{ $bumpmap keep/primary $surfaceprop concrete {base_parameter} }}"
                );
                let patch = format!(
                    "Patch {{ include materials/base.vmt {section} {{ $nodiffusebumplighting {patch_value} }} }}"
                );
                let inherited = "Patch { include materials/patch.vmt insert { %compiledetail 1 } }";
                let material = Material::load_with_reader("materials/inherited.vmt", &mut |path| {
                    Ok(Some(match path {
                        "materials/base.vmt" => base.as_bytes().to_vec(),
                        "materials/patch.vmt" => patch.as_bytes().to_vec(),
                        "materials/inherited.vmt" => inherited.as_bytes().to_vec(),
                        _ => panic!("unexpected material dependency {path}"),
                    }))
                })
                .unwrap()
                .unwrap();
                assert_eq!(material.get("$nodiffusebumplighting"), expected);
                assert_eq!(material.get("$bumpmap"), Some("keep/primary"));
                assert_eq!(material.get("$surfaceprop"), Some("concrete"));
                let mut expected_semantics = CompileSemantics::default();
                expected_semantics.contents |= contents::DETAIL;
                if expected != Some("1") {
                    expected_semantics.surface |= surf::BUMPLIGHT;
                }
                assert_eq!(material.compile_semantics(), expected_semantics);
            }
        }
    }
}
