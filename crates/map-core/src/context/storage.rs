use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use source_formats::keyvalues::{self, LineEnding, Node, WriteOptions};
use source_formats::vmf::{Properties, Vmf};

use super::{METADATA_KEY, Metadata, VERSION, text_fingerprint};
use crate::metadata::{InputBudget, MAX_DEPTH, MAX_NODES};

const PREFIX: &str = "_hammerr_context";
const ENVELOPE_VERSION: &str = "1";
const CHUNK_BYTES: usize = 480;
const MAX_SERIALIZED_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENCODED_BYTES: usize = MAX_SERIALIZED_BYTES.div_ceil(3) * 4;
const MAX_CHUNKS: usize = MAX_ENCODED_BYTES.div_ceil(CHUNK_BYTES);
const MAX_WORLD_PROPERTIES: usize = 65_534;

pub fn is_storage_key(key: &str) -> bool {
    key.trim()
        .get(..PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(PREFIX))
}

fn misplaced(vmf: &Vmf) -> bool {
    vmf.extra.iter().any(|node| is_storage_key(&node.key))
        || vmf
            .world
            .extra
            .iter()
            .any(|node| is_storage_key(&node.key) || node.key.eq_ignore_ascii_case(METADATA_KEY))
        || vmf.entities.iter().any(|entity| {
            entity.properties.iter().any(|(key, _)| is_storage_key(key))
                || entity.extra.iter().any(|node| is_storage_key(&node.key))
                || entity
                    .editor
                    .as_ref()
                    .is_some_and(|editor| editor.iter().any(|(key, _)| is_storage_key(key)))
        })
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

fn header(value: &str) -> Result<(usize, usize, &str), String> {
    let fields = value.split(' ').collect::<Vec<_>>();
    let [version, count, bytes, checksum] = fields.as_slice() else {
        return Err("Invalid HammeRR context transport header".into());
    };
    if *version != ENVELOPE_VERSION {
        return Err("Unsupported HammeRR context transport version".into());
    }
    let count = count
        .parse::<usize>()
        .map_err(|_| "Invalid context transport chunk count")?;
    let bytes = bytes
        .parse::<usize>()
        .map_err(|_| "Invalid context transport byte count")?;
    if count == 0
        || count > MAX_CHUNKS
        || bytes == 0
        || bytes > MAX_SERIALIZED_BYTES
        || checksum.len() != 16
        || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit())
        || value != format!("{ENVELOPE_VERSION} {count} {bytes} {checksum}")
    {
        return Err("Context transport header exceeds its bounds or is not canonical".into());
    }
    let encoded = bytes.div_ceil(3) * 4;
    if count != encoded.div_ceil(CHUNK_BYTES) {
        return Err("Context transport length does not match its chunk count".into());
    }
    Ok((count, bytes, checksum))
}

pub(super) fn read(vmf: &Vmf) -> Result<Option<Node>, String> {
    if misplaced(vmf) {
        return Err("HammeRR context transport must use scalar world properties, with legacy context only in its root block".into());
    }
    let mut fields = vmf
        .world
        .properties
        .iter()
        .filter(|(key, _)| is_storage_key(key));
    let Some(first) = fields.next() else {
        return Ok(None);
    };
    if vmf.world.properties.0.len() > MAX_WORLD_PROPERTIES {
        return Err("Context transport exceeds the 65534 world-property limit".into());
    }
    let fields = std::iter::once(first).chain(fields).collect::<Vec<_>>();
    let mut headers = fields
        .iter()
        .copied()
        .filter(|(key, _)| key.trim().eq_ignore_ascii_case(PREFIX));
    let Some((key, value)) = headers.next() else {
        return Err("Context transport is missing its header".into());
    };
    if headers.next().is_some() {
        return Err("Context transport has duplicate or case-aliased headers".into());
    }
    if key != PREFIX || value.len() > CHUNK_BYTES {
        return Err("Context transport header spelling or size is not canonical".into());
    }
    let (count, bytes, checksum) = header(value)?;
    if fields.len() != count + 1 {
        return Err("Context transport has missing, extra or duplicate reserved properties".into());
    }
    let encoded_bytes = bytes.div_ceil(3) * 4;
    let mut chunks = vec![None; count];
    for (key, value) in fields {
        if key == PREFIX {
            continue;
        }
        let index = key
            .strip_prefix(PREFIX)
            .and_then(|suffix| suffix.strip_prefix('_'))
            .and_then(|index| index.parse::<usize>().ok())
            .filter(|&index| index < count)
            .ok_or("Context transport has a malformed or case-aliased chunk key")?;
        let expected = format!("{PREFIX}_{index:05}");
        let expected_bytes = CHUNK_BYTES.min(encoded_bytes - index * CHUNK_BYTES);
        if key != expected || value.len() != expected_bytes || !value.is_ascii() {
            return Err("Context transport has noncanonical or truncated chunks".into());
        }
        if chunks[index].replace(value).is_some() {
            return Err("Context transport repeats a chunk number".into());
        }
    }
    let mut encoded = String::with_capacity(encoded_bytes);
    for chunk in chunks {
        encoded.push_str(chunk.ok_or("Context transport is missing a chunk number")?);
    }
    let decoded = STANDARD
        .decode(encoded)
        .map_err(|_| "Invalid base64 context transport")?;
    if decoded.len() != bytes {
        return Err("Context transport decoded length does not match its header".into());
    }
    let decoded = String::from_utf8(decoded).map_err(|_| "Context transport is not UTF-8")?;
    if text_fingerprint(&decoded) != checksum {
        return Err("Context transport checksum does not match its contents".into());
    }
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
    if value.len() > MAX_SERIALIZED_BYTES {
        return Err("Context transport exceeds its 16 MiB serialized metadata limit".into());
    }
    let checksum = text_fingerprint(&value);
    let bytes = value.len();
    let encoded = STANDARD.encode(value);
    let count = encoded.len().div_ceil(CHUNK_BYTES);
    let mut properties = Vec::with_capacity(count + 1);
    properties.push((
        PREFIX.into(),
        format!("{ENVELOPE_VERSION} {count} {bytes} {checksum}"),
    ));
    for (index, chunk) in encoded.as_bytes().chunks(CHUNK_BYTES).enumerate() {
        let value = std::str::from_utf8(chunk).map_err(|_| "Context transport must be ASCII")?;
        properties.push((format!("{PREFIX}_{index:05}"), value.to_owned()));
    }
    Ok(properties)
}

pub(crate) fn store(vmf: &mut Vmf, metadata: &Metadata) -> Result<(), String> {
    metadata.validate()?;
    let carrier = encoded(metadata)?;
    let ordinary = vmf
        .world
        .properties
        .iter()
        .filter(|(key, _)| !is_storage_key(key))
        .count();
    if ordinary.saturating_add(carrier.len()) > MAX_WORLD_PROPERTIES {
        return Err("Context transport and ordinary world properties exceed the shared 65534-property limit".into());
    }
    let mut properties = Vec::with_capacity(ordinary + carrier.len());
    let mut carrier = Some(carrier);
    for (key, value) in &vmf.world.properties.0 {
        if is_storage_key(key) {
            if let Some(carrier) = carrier.take() {
                properties.extend(carrier);
            }
        } else {
            properties.push((key.clone(), value.clone()));
        }
    }
    if let Some(carrier) = carrier {
        properties.extend(carrier);
    }
    vmf.world.properties = Properties(properties);
    vmf.extra
        .retain(|node| !node.key.eq_ignore_ascii_case(METADATA_KEY));
    Ok(())
}
