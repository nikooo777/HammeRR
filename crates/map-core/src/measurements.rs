use std::collections::BTreeSet;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use glam::DVec3;
use source_formats::keyvalues::Node;
use source_formats::vmf::{Vmf, parse_vec3};

use crate::{EditError, Transaction};

mod origins;
pub(crate) use origins::reserved_origin_ids;
pub use origins::{Endpoint, entity_local_pose, entity_origin};

pub const KEY: &str = "hammerr_measurements";
pub const MAX_MEASUREMENTS: usize = 128;
pub const MAX_NAME_CHARS: usize = 80;
pub const MAX_COORDINATE: f64 = 65_536.0;
const MAX_ENCODED_NAME_BYTES: usize = MAX_NAME_CHARS * 4 * 2;
const FIELDS: &[&str] = &["id", "name_base64", "start", "end"];

#[derive(Clone, Debug, PartialEq)]
pub struct Measurement {
    pub id: u32,
    pub name: String,
    pub start: DVec3,
    pub end: DVec3,
    pub extra: Vec<Node>,
}

impl Measurement {
    pub fn validate(&self) -> Result<(), String> {
        if self.id == 0 {
            return Err("Measurement IDs must be positive".into());
        }
        if self.name.trim().is_empty()
            || self.name.chars().count() > MAX_NAME_CHARS
            || self.name.chars().any(char::is_control)
        {
            return Err(format!(
                "Measurement names need 1–{MAX_NAME_CHARS} characters without control characters"
            ));
        }
        for point in [self.start, self.end] {
            validate_point(point)?;
        }
        let start_origin = self.origin_reference(Endpoint::Start)?;
        let end_origin = self.origin_reference(Endpoint::End)?;
        self.local_offset(Endpoint::Start)?;
        self.local_offset(Endpoint::End)?;
        if self.start == self.end && start_origin.is_none() && end_origin.is_none() {
            return Err("Measurement endpoints must be distinct".into());
        }
        if self
            .extra
            .iter()
            .any(|node| FIELDS.contains(&node.key.as_str()))
        {
            return Err("Measurement extension fields conflict with a required field".into());
        }
        Ok(())
    }

    pub fn components(&self) -> DVec3 {
        self.end - self.start
    }

    pub fn distance(&self) -> f64 {
        self.components().length()
    }

    fn to_node(&self) -> Node {
        let mut fields = vec![
            Node::string("id", self.id.to_string()),
            Node::string("name_base64", STANDARD.encode(&self.name)),
            Node::string("start", exact_point(self.start)),
            Node::string("end", exact_point(self.end)),
        ];
        fields.extend(self.extra.clone());
        Node::block("measurement", fields)
    }

    fn preserving_node(&self, original: &Self, node: &Node) -> Node {
        let mut remaining = self.to_node().children().unwrap().to_vec();
        let mut fields = Vec::new();
        for previous in node.children().unwrap() {
            let Some(index) = remaining.iter().position(|field| field.key == previous.key) else {
                continue;
            };
            let current = remaining.remove(index);
            let unchanged = match previous.key.as_str() {
                "id" => self.id == original.id,
                "name_base64" => self.name == original.name,
                "start" => self.start == original.start,
                "end" => self.end == original.end,
                _ => current == *previous,
            };
            fields.push(if unchanged { previous.clone() } else { current });
        }
        fields.extend(remaining);
        Node::block("measurement", fields)
    }
}

fn exact_point(point: DVec3) -> String {
    format!("{} {} {}", point.x, point.y, point.z)
}

fn validate_point(point: DVec3) -> Result<(), String> {
    if !point.is_finite() || point.abs().max_element() > MAX_COORDINATE {
        return Err(format!(
            "Measurement endpoints need finite coordinates within ±{MAX_COORDINATE}"
        ));
    }
    Ok(())
}

fn field<'a>(nodes: &'a [Node], key: &str) -> Result<&'a str, String> {
    let mut fields = nodes.iter().filter(|node| node.key == key);
    let value = fields
        .next()
        .and_then(Node::as_str)
        .ok_or_else(|| format!("Missing or invalid measurement field: {key}"))?;
    if fields.next().is_some() {
        return Err(format!("Duplicate measurement field: {key}"));
    }
    Ok(value)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measurements {
    pub measurements: Vec<Measurement>,
    pub extra: Vec<Node>,
}

impl Measurements {
    pub fn parse(vmf: &Vmf) -> Result<Self, String> {
        let mut blocks = vmf.extra.iter().filter(|node| node.key == KEY);
        let Some(node) = blocks.next() else {
            return Ok(Self::default());
        };
        if blocks.next().is_some() {
            return Err("Multiple measurement metadata blocks; metadata is preserved".into());
        }
        let nodes = node
            .children()
            .ok_or("Measurements must be a metadata block")?;
        let version = field(nodes, "version")?;
        if !matches!(version, "1" | "2") {
            return Err("Unsupported measurement metadata version; metadata is preserved".into());
        }
        let mut result = Self::default();
        for node in nodes {
            if node.key == "version" {
                continue;
            }
            if node.key != "measurement" {
                result.extra.push(node.clone());
                continue;
            }
            if result.measurements.len() == MAX_MEASUREMENTS {
                return Err(format!(
                    "A map supports at most {MAX_MEASUREMENTS} measurements"
                ));
            }
            let fields = node.children().ok_or("Measurement must be a block")?;
            let encoded = field(fields, "name_base64")?;
            if encoded.len() > MAX_ENCODED_NAME_BYTES {
                return Err("Encoded measurement name exceeds the supported limit".into());
            }
            let name = String::from_utf8(
                STANDARD
                    .decode(encoded)
                    .map_err(|_| "Invalid measurement name encoding")?,
            )
            .map_err(|_| "Measurement name is not UTF-8")?;
            result.measurements.push(Measurement {
                id: field(fields, "id")?
                    .parse()
                    .map_err(|_| "Invalid measurement ID")?,
                name,
                start: parse_vec3(field(fields, "start")?).ok_or("Invalid measurement start")?,
                end: parse_vec3(field(fields, "end")?).ok_or("Invalid measurement end")?,
                extra: fields
                    .iter()
                    .filter(|node| !FIELDS.contains(&node.key.as_str()))
                    .cloned()
                    .collect(),
            });
        }
        result.validate()?;
        if version == "1" && result.has_local_attachments() {
            return Err(
                "Local measurement attachments require metadata version 2; metadata is preserved"
                    .into(),
            );
        }
        Ok(result)
    }

    fn has_local_attachments(&self) -> bool {
        self.measurements.iter().any(|measurement| {
            [Endpoint::Start, Endpoint::End]
                .into_iter()
                .any(|endpoint| measurement.local_offset(endpoint).ok().flatten().is_some())
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.measurements.len() > MAX_MEASUREMENTS {
            return Err(format!(
                "A map supports at most {MAX_MEASUREMENTS} measurements"
            ));
        }
        if self
            .extra
            .iter()
            .any(|node| matches!(node.key.as_str(), "version" | "measurement"))
        {
            return Err(
                "Measurement metadata extension fields conflict with a required field".into(),
            );
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for measurement in &self.measurements {
            measurement.validate()?;
            if !ids.insert(measurement.id) {
                return Err("Duplicate measurement ID".into());
            }
            if !names.insert(measurement.name.trim().to_lowercase()) {
                return Err("A measurement already uses that name".into());
            }
        }
        Ok(())
    }

    pub fn next_id(&self) -> Result<u32, String> {
        self.measurements
            .iter()
            .map(|measurement| measurement.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| "Measurement IDs exhausted".into())
    }

    pub fn write(self, tx: &mut Transaction) -> Result<(), EditError> {
        let previous = Self::parse(tx.vmf()).map_err(EditError::Invalid)?;
        self.validate().map_err(EditError::Invalid)?;
        let needs_local_version = self.has_local_attachments();
        let node = if self.measurements.is_empty() && self.extra.is_empty() {
            None
        } else {
            let original = tx.vmf().extra.iter().find(|node| node.key == KEY);
            let original_fields = original.and_then(Node::children).unwrap_or_default();
            let mut pending = self.measurements.clone();
            let mut pending_extra = self.extra;
            let mut nodes = Vec::new();
            for node in original_fields {
                if node.key == "version" {
                    nodes.push(if needs_local_version && node.as_str() == Some("1") {
                        Node::string("version", "2")
                    } else {
                        node.clone()
                    });
                } else if node.key == "measurement" {
                    let id = field(node.children().unwrap(), "id")
                        .unwrap()
                        .parse::<u32>()
                        .unwrap();
                    if let Some(index) = pending.iter().position(|entry| entry.id == id) {
                        let measurement = pending.remove(index);
                        let old = previous
                            .measurements
                            .iter()
                            .find(|entry| entry.id == id)
                            .unwrap();
                        nodes.push(measurement.preserving_node(old, node));
                    }
                } else if let Some(index) = pending_extra.iter().position(|entry| entry == node) {
                    nodes.push(pending_extra.remove(index));
                }
            }
            if original.is_none() {
                nodes.push(Node::string(
                    "version",
                    if needs_local_version { "2" } else { "1" },
                ));
            }
            nodes.extend(pending.iter().map(Measurement::to_node));
            nodes.extend(pending_extra);
            Some(Node::block(KEY, nodes))
        };
        tx.set_editor_metadata(KEY, node);
        Ok(())
    }
}
