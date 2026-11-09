use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use source_formats::bsp::{FaceGeometry, LightingKind, checked_model_face_range};
use source_formats::keyvalues::{FlatBlock, FlatBlockLimits, parse_flat_blocks_controlled};

use crate::util::{self, invalid};

const MAX_ENTITY_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENTITY_BLOCKS: usize = 16384;
const MAX_ENTITY_PAIRS: usize = 131072;
const SUPPORT_CLASS: &str = "func_movelinear";
const DECLARATION_PROFILE: &str = "css_linux64_build25338318_compiled_func_movelinear_v1";
const ASSOCIATION: &str = "matching_exact_compiled_entity_and_model_name_declaration";
pub(super) const MAX_PLUGIN_REQUEST_BYTES: usize = MAX_ENTITY_BYTES + 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CompiledSupportDeclaration {
    pub hammer_id: i32,
    pub model_name: String,
    pub bsp_model_index: u32,
    pub parented: bool,
}

fn model_index(name: &str) -> io::Result<u32> {
    let value = name
        .strip_prefix('*')
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|index| *index > 0)
        .ok_or_else(|| invalid("Compiled moving support requires canonical model *N with N > 0"))?;
    if name != format!("*{value}") {
        return Err(invalid(
            "Compiled moving support model name is not canonical",
        ));
    }
    Ok(value)
}

fn field<'a>(entity: &'a FlatBlock, name: &str) -> io::Result<Option<&'a str>> {
    let mut values = entity
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str());
    let value = values.next();
    if values.next().is_some() {
        return Err(invalid(format!(
            "Compiled moving support has duplicate {name} fields"
        )));
    }
    Ok(value)
}

pub(super) fn validate(declarations: &[CompiledSupportDeclaration]) -> io::Result<()> {
    if declarations.len() > MAX_ENTITY_BLOCKS {
        return Err(invalid(
            "Compiled moving supports exceed the entity-block bound",
        ));
    }
    let mut ids = BTreeSet::new();
    for declaration in declarations {
        if declaration.hammer_id <= 0
            || !ids.insert(declaration.hammer_id)
            || model_index(&declaration.model_name)? != declaration.bsp_model_index
        {
            return Err(invalid(
                "Compiled moving supports have invalid models or duplicate identities",
            ));
        }
    }
    Ok(())
}

pub(super) fn matching<'a>(
    declarations: &'a [CompiledSupportDeclaration],
    classname: &str,
    hammer_id: i32,
    model_name: &str,
) -> io::Result<&'a CompiledSupportDeclaration> {
    let declaration = declarations
        .iter()
        .find(|declaration| declaration.hammer_id == hammer_id)
        .filter(|declaration| classname == SUPPORT_CLASS && declaration.model_name == model_name)
        .ok_or_else(|| invalid("Native moving support has no exact compiled declaration"))?;
    if declaration.parented {
        return Err(invalid(
            "Parented compiled moving support is outside this profile",
        ));
    }
    Ok(declaration)
}

pub(super) fn metadata(declarations: &[CompiledSupportDeclaration]) -> io::Result<Value> {
    validate(declarations)?;
    let bytes = serde_json::to_vec(declarations).map_err(io::Error::other)?;
    let excluded = declarations
        .iter()
        .filter(|declaration| declaration.parented)
        .count();
    Ok(
        json!({"profile":DECLARATION_PROFILE,"count":declarations.len(),
        "supported_count":declarations.len()-excluded,"excluded_parented_count":excluded,
        "sha256":util::hash(&bytes),"association":ASSOCIATION}),
    )
}

pub(super) fn derive(
    bytes: &[u8],
    team: u8,
    check: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<Vec<CompiledSupportDeclaration>> {
    let bsp = crate::owned_client::parse_bsp(bytes, check)?;
    let entities = parse_flat_blocks_controlled(
        bsp.entities()?,
        FlatBlockLimits {
            max_bytes: MAX_ENTITY_BYTES,
            max_blocks: MAX_ENTITY_BLOCKS,
            max_pairs: MAX_ENTITY_PAIRS,
        },
        check,
    )?;
    let spawn_class = if team == 2 {
        "info_player_terrorist"
    } else {
        "info_player_counterterrorist"
    };
    if !entities.iter().any(|entity| {
        entity.iter().any(|(key, value)| {
            key.eq_ignore_ascii_case("classname") && value.eq_ignore_ascii_case(spawn_class)
        })
    }) {
        return Err(invalid(
            "Player support observation requires an eligible spawn for the selected team",
        ));
    }
    let mut declarations = Vec::new();
    let mut hammer_ids = BTreeMap::<i32, usize>::new();
    for entity in &entities {
        check()?;
        for (key, value) in entity {
            if key.eq_ignore_ascii_case("hammerid")
                && let Ok(id) = value.parse::<i32>()
            {
                *hammer_ids.entry(id).or_default() += 1;
            }
        }
        if !entity.iter().any(|(key, value)| {
            key.eq_ignore_ascii_case("classname") && value.eq_ignore_ascii_case(SUPPORT_CLASS)
        }) {
            continue;
        }
        if field(entity, "classname")? != Some(SUPPORT_CLASS) {
            return Err(invalid(
                "Compiled moving support classname is not canonical",
            ));
        }
        let hammer_id = field(entity, "hammerid")?
            .and_then(|value| value.parse::<i32>().ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid("Compiled moving support requires one positive Hammer ID"))?;
        let model_name = field(entity, "model")?
            .ok_or_else(|| invalid("Compiled moving support requires one brush model"))?;
        declarations.push(CompiledSupportDeclaration {
            hammer_id,
            model_name: model_name.to_owned(),
            bsp_model_index: model_index(model_name)?,
            parented: field(entity, "parentname")?.is_some_and(|value| !value.is_empty()),
        });
    }
    validate(&declarations)?;
    if declarations.is_empty() {
        return Ok(declarations);
    }
    let geometry = FaceGeometry::new_controlled(&bsp, LightingKind::Ldr, check)?;
    for declaration in &declarations {
        check()?;
        let model = geometry
            .models()
            .get(declaration.bsp_model_index as usize)
            .ok_or_else(|| invalid("Compiled moving support model is outside the BSP catalog"))?;
        let faces = checked_model_face_range(model, geometry.faces().len())?;
        if geometry.faces()[faces]
            .iter()
            .any(|face| face.dispinfo >= 0)
        {
            return Err(invalid(
                "Compiled moving support model owns displacement faces",
            ));
        }
        if hammer_ids.get(&declaration.hammer_id) != Some(&1) {
            return Err(invalid(
                "Compiled moving support Hammer ID is not unique in the BSP",
            ));
        }
    }
    check()?;
    Ok(declarations)
}
