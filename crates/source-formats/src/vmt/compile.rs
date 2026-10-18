use crate::bsp::{contents, surf};

use super::Material;

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
        if semantics.lit() && m.get("$bumpmap").is_some() {
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
