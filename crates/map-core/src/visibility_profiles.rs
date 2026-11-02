use std::collections::BTreeSet;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use source_formats::keyvalues::{self, Node, WriteOptions};
use source_formats::vmf::Vmf;

use crate::{EditError, Transaction};

pub const KEY: &str = "hammerr_visibility_profiles";
pub const MAX_PROFILES: usize = 64;
pub const MAX_METADATA_BYTES: usize = 256 * 1024;
pub const MAX_NAME_CHARACTERS: usize = 64;
pub const MAX_NAME_BYTES: usize = 256;
const FIELDS: &[&str] = &["name_base64", "payload_base64"];

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub payload: String,
    pub extra: Vec<Node>,
}

impl Entry {
    fn to_node(&self) -> Node {
        let mut fields = vec![
            Node::string("name_base64", STANDARD.encode(&self.name)),
            Node::string("payload_base64", STANDARD.encode(&self.payload)),
        ];
        fields.extend(self.extra.clone());
        Node::block("profile", fields)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub entries: Vec<Entry>,
    pub extra: Vec<Node>,
}

pub fn name_key(name: &str) -> String {
    name.to_lowercase()
}

pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.trim() != name
        || name.len() > MAX_NAME_BYTES
        || name.chars().count() > MAX_NAME_CHARACTERS
        || name.chars().any(char::is_control)
    {
        return Err(format!(
            "Visibility profile names require 1–{MAX_NAME_CHARACTERS} characters, at most {MAX_NAME_BYTES} UTF-8 bytes, without control characters or surrounding whitespace"
        ));
    }
    Ok(())
}

fn field<'a>(fields: &'a [Node], key: &str) -> Result<&'a str, String> {
    let mut matching = fields.iter().filter(|node| node.key == key);
    let value = matching
        .next()
        .and_then(Node::as_str)
        .ok_or_else(|| format!("Missing or invalid visibility profile field: {key}"))?;
    if matching.next().is_some() {
        return Err(format!("Duplicate visibility profile field: {key}"));
    }
    Ok(value)
}

fn decoded_field(fields: &[Node], key: &str) -> Result<String, String> {
    let decoded = STANDARD
        .decode(field(fields, key)?)
        .map_err(|_| format!("Invalid visibility profile field encoding: {key}"))?;
    String::from_utf8(decoded).map_err(|_| format!("Visibility profile field is not UTF-8: {key}"))
}

fn validate_metadata(node: &Node) -> Result<(), String> {
    let nodes = std::slice::from_ref(node);
    crate::metadata::InputBudget::default()
        .nodes(nodes)
        .map_err(|error| error.to_string())?;
    let encoded =
        keyvalues::try_write(nodes, &WriteOptions::default()).map_err(|error| error.to_string())?;
    if encoded.len() > MAX_METADATA_BYTES {
        return Err(format!(
            "Visibility profile metadata exceeds {MAX_METADATA_BYTES} encoded bytes; metadata is preserved"
        ));
    }
    Ok(())
}

impl Metadata {
    fn validate_entries(&self) -> Result<(), String> {
        if self.entries.len() > MAX_PROFILES {
            return Err(format!(
                "Visibility profiles contain more than {MAX_PROFILES} entries; metadata is preserved"
            ));
        }
        if self
            .extra
            .iter()
            .any(|node| matches!(node.key.as_str(), "version" | "profile"))
        {
            return Err("Visibility profile metadata contains reserved extra fields".into());
        }
        let mut names = BTreeSet::new();
        for entry in &self.entries {
            validate_name(&entry.name)?;
            if !names.insert(name_key(&entry.name)) {
                return Err("Visibility profiles have duplicate case-insensitive names; metadata is preserved".into());
            }
            if entry.payload.len() > MAX_METADATA_BYTES {
                return Err(format!(
                    "Visibility profile payload exceeds {MAX_METADATA_BYTES} bytes"
                ));
            }
            if entry
                .extra
                .iter()
                .any(|node| FIELDS.contains(&node.key.as_str()))
            {
                return Err("Visibility profile contains reserved extra fields".into());
            }
        }
        Ok(())
    }

    fn checked_node(&self) -> Result<Node, String> {
        self.validate_entries()?;
        let mut fields = vec![Node::string("version", "1")];
        fields.extend(self.entries.iter().map(Entry::to_node));
        fields.extend(self.extra.clone());
        let node = Node::block(KEY, fields);
        validate_metadata(&node)?;
        Ok(node)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.checked_node().map(|_| ())
    }

    pub fn parse(vmf: &Vmf) -> Result<Self, String> {
        let mut matching = vmf.extra.iter().filter(|node| node.key == KEY);
        let Some(node) = matching.next() else {
            return Ok(Self::default());
        };
        if matching.next().is_some() {
            return Err(
                "Multiple visibility profile metadata blocks; metadata is preserved".into(),
            );
        }
        validate_metadata(node)?;
        let fields = node
            .children()
            .ok_or("Visibility profiles must be a metadata block")?;
        if field(fields, "version")? != "1" {
            return Err(
                "Unsupported visibility profile metadata version; metadata is preserved".into(),
            );
        }
        let mut result = Self::default();
        for node in fields {
            if node.key == "version" {
                continue;
            }
            if node.key != "profile" {
                result.extra.push(node.clone());
                continue;
            }
            if result.entries.len() == MAX_PROFILES {
                return Err(format!(
                    "Visibility profiles contain more than {MAX_PROFILES} entries; metadata is preserved"
                ));
            }
            let fields = node
                .children()
                .ok_or("Visibility profile must be a metadata block")?;
            result.entries.push(Entry {
                name: decoded_field(fields, "name_base64")?,
                payload: decoded_field(fields, "payload_base64")?,
                extra: fields
                    .iter()
                    .filter(|node| !FIELDS.contains(&node.key.as_str()))
                    .cloned()
                    .collect(),
            });
        }
        result.validate_entries()?;
        Ok(result)
    }

    pub fn write(self, tx: &mut Transaction) -> Result<(), EditError> {
        Self::parse(tx.vmf()).map_err(EditError::Invalid)?;
        let node = self.checked_node().map_err(EditError::Invalid)?;
        let node = if self.entries.is_empty() && self.extra.is_empty() {
            None
        } else {
            Some(node)
        };
        tx.set_editor_metadata(KEY, node);
        Ok(())
    }
}
