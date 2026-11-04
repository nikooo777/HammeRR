use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use source_formats::keyvalues::{self, LineEnding, Node, WriteOptions};
use source_formats::vmf::Vmf;

use super::{METADATA_KEY, Metadata, VERSION};
use crate::metadata::{InputBudget, MAX_DEPTH, MAX_NODES};
use crate::retained_metadata;

const PREFIX: &str = "_hammerr_context";
const MAX_SERIALIZED_BYTES: usize = 16 * 1024 * 1024;

pub fn is_storage_key(key: &str) -> bool {
    retained_metadata::is_key(key, PREFIX)
}

fn misplaced(vmf: &Vmf) -> bool {
    retained_metadata::misplaced(vmf, PREFIX)
        || vmf
            .world
            .extra
            .iter()
            .any(|node| node.key.eq_ignore_ascii_case(METADATA_KEY))
}

pub fn has_storage(vmf: &Vmf) -> bool {
    misplaced(vmf)
        || vmf
            .extra
            .iter()
            .any(|node| node.key.eq_ignore_ascii_case(METADATA_KEY))
        || vmf
            .world
            .properties
            .iter()
            .any(|(key, _)| is_storage_key(key))
}

pub(super) fn validate_node(node: &Node) -> Result<(), String> {
    InputBudget::default()
        .nodes(std::slice::from_ref(node))
        .map_err(|error| error.to_string())
}

pub(super) fn validate_metadata(metadata: &Metadata) -> Result<(), String> {
    let mut budget = InputBudget::default();
    let mut text =
        |key: &str, value: &str| budget.text(key, value).map_err(|error| error.to_string());
    text(METADATA_KEY, "")?;
    text("version", VERSION)?;
    for (scope, note) in &metadata.notes {
        text("note", "")?;
        text("scope", &scope.to_string())?;
        text("summary_base64", &STANDARD.encode(&note.summary))?;
        text("body_base64", &STANDARD.encode(&note.body))?;
        text("author", &note.author)?;
        text("observed", &note.observed)?;
        if let Some(region) = note.region {
            text("mins", &source_formats::vmf::format_vec3(region.mins))?;
            text("maxs", &source_formats::vmf::format_vec3(region.maxs))?;
            text(
                "color",
                &format!(
                    "{} {} {}",
                    region.color[0], region.color[1], region.color[2]
                ),
            )?;
        }
    }
    for note in metadata.notes.values() {
        budget
            .nodes(&note.extra)
            .map_err(|error| error.to_string())?;
    }
    budget
        .nodes(&metadata.extra)
        .map_err(|error| error.to_string())
}

pub(super) fn read(vmf: &Vmf) -> Result<Option<Node>, String> {
    if misplaced(vmf) {
        return Err("HammeRR context transport must use scalar world properties, with legacy context only in its root block".into());
    }
    let Some(decoded) = retained_metadata::read(vmf, PREFIX, "Context", MAX_SERIALIZED_BYTES)?
    else {
        return Ok(None);
    };
    let nodes = keyvalues::parse_bounded(&decoded, MAX_DEPTH, MAX_NODES)
        .map_err(|error| format!("Invalid context transport metadata: {error}"))?;
    let [node] = nodes.as_slice() else {
        return Err("Context transport must contain exactly one context block".into());
    };
    if node.key != METADATA_KEY || node.children().is_none() {
        return Err("Context transport must contain a versioned context block".into());
    }
    validate_node(node)?;
    Ok(nodes.into_iter().next())
}

fn encoded(metadata: &Metadata) -> Result<Vec<(String, String)>, String> {
    if metadata.notes.is_empty() && metadata.extra.is_empty() {
        return Ok(Vec::new());
    }
    let node = metadata.to_node();
    validate_node(&node)?;
    let value = keyvalues::try_write(
        &[node],
        &WriteOptions {
            line_ending: LineEnding::Lf,
        },
    )
    .map_err(|error| error.to_string())?;
    retained_metadata::encode(PREFIX, "Context", &value, MAX_SERIALIZED_BYTES)
}

pub(crate) fn store(vmf: &mut Vmf, metadata: &Metadata) -> Result<(), String> {
    metadata.validate()?;
    let carrier = encoded(metadata)?;
    retained_metadata::store(vmf, PREFIX, "Context", carrier)?;
    vmf.extra
        .retain(|node| !node.key.eq_ignore_ascii_case(METADATA_KEY));
    Ok(())
}
