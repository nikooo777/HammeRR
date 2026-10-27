use std::collections::{BTreeMap, BTreeSet};

use source_formats::keyvalues::{self, Node, ParseErrorKind};

use super::{Failure, MAX_KEY_BYTES, MAX_PATH_BYTES, WaterDependencyState, is_water_shader};

#[derive(Debug, Clone)]
pub(super) struct Parameter {
    pub key: String,
    pub value: String,
}

pub(super) struct RawMaterial {
    pub shader: String,
    pub parameters: BTreeMap<String, Parameter>,
    pub include: Option<Parameter>,
    pub edits: Vec<(bool, BTreeMap<String, Parameter>)>,
}

fn unsupported(code: &'static str, message: impl Into<String>) -> Failure {
    Failure::new(WaterDependencyState::Unsupported, code, message)
}

fn parameters(nodes: &[Node], packing: bool) -> Result<BTreeMap<String, Parameter>, Failure> {
    let mut values = BTreeMap::new();
    for node in nodes {
        let key = node.key.to_ascii_lowercase();
        if key.len() > MAX_KEY_BYTES || !key.is_ascii() || key.contains('#') {
            return Err(unsupported(
                "unsupported_key",
                "Material keys must be bounded unsuffixed ASCII names",
            )
            .key(&node.key));
        }
        let value = node.as_str().ok_or_else(|| {
            unsupported(
                "dynamic_block",
                "Nested, proxy and conditional material blocks are unsupported",
            )
            .key(&node.key)
        })?;
        if value.len() > MAX_PATH_BYTES {
            return Err(Failure::new(
                WaterDependencyState::Limited,
                "value_limit",
                "Material declaration exceeds 512 bytes",
            )
            .key(&node.key));
        }
        if !(supported_water_key(&key) || packing && supported_slime_key(&key)) {
            return Err(unsupported(
                "unknown_property",
                format!("Unsupported Water declaration {}", node.key),
            )
            .key(&node.key));
        }
        if values
            .insert(
                key,
                Parameter {
                    key: node.key.clone(),
                    value: value.into(),
                },
            )
            .is_some()
        {
            return Err(unsupported(
                "duplicate_key",
                "Duplicate case-insensitive material declaration",
            )
            .key(&node.key));
        }
    }
    Ok(values)
}

pub(super) fn parse(bytes: &[u8]) -> Result<RawMaterial, Failure> {
    parse_scoped(bytes, false)
}

pub(super) fn parse_for_packing(bytes: &[u8]) -> Result<RawMaterial, Failure> {
    parse_scoped(bytes, true)
}

fn parse_scoped(bytes: &[u8], packing: bool) -> Result<RawMaterial, Failure> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        Failure::new(
            WaterDependencyState::UnavailableSource,
            "utf8",
            error.to_string(),
        )
    })?;
    let nodes =
        keyvalues::parse_static_material_bounded(text.trim_start_matches('\u{feff}'), 64, 4096)
            .map_err(|error| {
                let (state, code) = match error.kind {
                    ParseErrorKind::ConditionalToken => {
                        (WaterDependencyState::Unsupported, "conditional")
                    }
                    ParseErrorKind::EmbeddedCommentMarker => {
                        (WaterDependencyState::Unsupported, "embedded_comment_marker")
                    }
                    ParseErrorKind::NestingLimit | ParseErrorKind::NodeLimit => {
                        (WaterDependencyState::Limited, "raw_syntax")
                    }
                    _ => (WaterDependencyState::UnavailableSource, "raw_syntax"),
                };
                Failure::new(state, code, error.to_string())
            })?;
    if nodes.len() != 1 {
        return Err(unsupported(
            "material_roots",
            "Static inventory requires exactly one material root",
        ));
    }
    let root = &nodes[0];
    let children = root.children().ok_or_else(|| {
        Failure::new(
            WaterDependencyState::UnavailableSource,
            "raw_syntax",
            "Material root must be a block",
        )
    })?;
    let shader = root.key.to_ascii_lowercase();
    if shader.len() > MAX_KEY_BYTES || !shader.is_ascii() {
        return Err(unsupported(
            "unsupported_shader",
            "Shader name exceeds the bounded ASCII subset",
        ));
    }
    let mut result = RawMaterial {
        shader,
        parameters: BTreeMap::new(),
        include: None,
        edits: Vec::new(),
    };
    if result.shader == "patch" {
        let mut seen = BTreeSet::new();
        let mut edited = BTreeSet::new();
        for node in children {
            let key = node.key.to_ascii_lowercase();
            if !seen.insert(key.clone()) {
                return Err(unsupported("duplicate_key", "Duplicate Patch section").key(&node.key));
            }
            match key.as_str() {
                "include" => {
                    let value = node.as_str().ok_or_else(|| {
                        unsupported("patch_include", "Patch include must be scalar")
                    })?;
                    if value.len() > MAX_PATH_BYTES {
                        return Err(Failure::new(
                            WaterDependencyState::Limited,
                            "path_limit",
                            "Patch include exceeds 512 bytes",
                        ));
                    }
                    result.include = Some(Parameter {
                        key: node.key.clone(),
                        value: value.into(),
                    });
                }
                "insert" | "replace" => {
                    let values = parameters(
                        node.children().ok_or_else(|| {
                            unsupported("patch_edit", "Patch edits must be scalar blocks")
                        })?,
                        packing,
                    )?;
                    for name in values.keys() {
                        if !edited.insert(name.clone()) {
                            return Err(unsupported(
                                "patch_conflict",
                                "A declaration cannot occur in both Patch edit sections",
                            )
                            .key(name));
                        }
                    }
                    result.edits.push((key == "replace", values));
                }
                _ => {
                    return Err(
                        unsupported("patch_property", "Unsupported Patch section").key(&node.key)
                    );
                }
            }
        }
        if result.include.is_none() {
            return Err(unsupported(
                "patch_include",
                "Patch requires exactly one include",
            ));
        }
    } else if is_water_shader(&result.shader)
        || packing
            && matches!(
                result.shader.as_str(),
                "lightmappedgeneric" | "unlitgeneric"
            )
    {
        result.parameters = parameters(children, packing)?;
    }
    Ok(result)
}

pub(super) fn supported_water_key(key: &str) -> bool {
    matches!(
        key,
        "$normalmap"
            | "$basetexture"
            | "$reflecttexture"
            | "$refracttexture"
            | "$envmap"
            | "$bottommaterial"
            | "$underwateroverlay"
            | "%tooltexture"
            | "$refractamount"
            | "$refracttint"
            | "$reflectamount"
            | "$reflecttint"
            | "$bumpframe"
            | "$bumptransform"
            | "$scale"
            | "$time"
            | "$waterdepth"
            | "$cheapwaterstartdistance"
            | "$cheapwaterenddistance"
            | "$envmapframe"
            | "$fogcolor"
            | "$forcecheap"
            | "$forceexpensive"
            | "$reflectentities"
            | "$fogstart"
            | "$fogend"
            | "$abovewater"
            | "$reflectblendfactor"
            | "$nofresnel"
            | "$nolowendlightmap"
            | "$scroll1"
            | "$scroll2"
            | "$blurrefract"
            | "$fogenable"
            | "$surfaceprop"
            | "$basetexturetransform"
            | "%compilewater"
            | "%compilenolight"
            | "%keywords"
    )
}

pub(super) fn supported_slime_key(key: &str) -> bool {
    matches!(
        key,
        "%compileslime"
            | "$bottommaterial"
            | "$basetexture"
            | "$bumpmap"
            | "%tooltexture"
            | "$surfaceprop"
            | "%keywords"
            | "%notooltexture"
            | "$nofog"
            | "$fogenable"
            | "$fogcolor"
            | "$fogstart"
            | "$fogend"
            | "$color"
            | "$color2"
            | "$basetexturetransform"
            | "$bumptransform"
            | "$reflectivity"
            | "$alphatest"
            | "$translucent"
            | "$additive"
            | "$vertexalpha"
            | "$vertexalphatest"
            | "$allowalphatocoverage"
            | "$distancealpha"
            | "$distancealphafromdetail"
            | "$glow"
            | "$wireframe"
            | "$decal"
            | "$nodraw"
            | "$depthblend"
            | "$softedges"
            | "$outline"
            | "$ignorez"
            | "$znearer"
            | "$alpha"
    )
}
