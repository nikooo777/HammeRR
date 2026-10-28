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
        if material.compile_flag("%compilepassbullets") {
            const CONFLICTS: &[&str] = &[
                "%compileclip",
                "%playerclip",
                "%compileplayerclip",
                "%compilenpcclip",
                "%compiletrigger",
                "%compilenolight",
                "%compilesky",
                "%compile2dsky",
                "%compilehint",
                "%compileskip",
                "%compileorigin",
                "%compileareaportal",
                "%compileladder",
                "%compilefog",
                "%compilewater",
                "%compileslime",
                "%compileblocklos",
                "%compilenonsolid",
            ];
            if let Some(conflict) = CONFLICTS.iter().find(|key| material.compile_flag(key)) {
                return Err(format!(
                    "Unsupported %compilepassbullets with {conflict}; remove the conflicting compile flag or use separate materials"
                ));
            }
            if material.shader == "water" {
                return Err(
                    "Unsupported %compilepassbullets on Water; separate GRATE collision from the liquid material".into(),
                );
            }
        }
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
        if flag("%compilepassbullets") {
            semantics.contents = contents::GRATE;
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
            semantics.surface |= surf::NODECALS;
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
            "%compilepassbullets",
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
