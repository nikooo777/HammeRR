use std::collections::BTreeSet;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use glam::DVec3;
use source_formats::keyvalues::Node;
use source_formats::vmf::{Vmf, parse_vec3};

use crate::{EditError, Transaction};

mod origins;
pub use origins::{Endpoint, entity_origin};

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
        if field(nodes, "version")? != "1" {
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
        Ok(result)
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
                    nodes.push(node.clone());
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
                nodes.push(Node::string("version", "1"));
            }
            nodes.extend(pending.iter().map(Measurement::to_node));
            nodes.extend(pending_extra);
            Some(Node::block(KEY, nodes))
        };
        tx.set_editor_metadata(KEY, node);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    fn measurement(id: u32) -> Measurement {
        Measurement {
            id,
            name: format!("Ruler {id}"),
            start: DVec3::new(0.12345678901234567, -7.5, 32.0),
            end: DVec3::new(3.1234567890123457, -3.5, 44.0),
            extra: vec![Node::string("future", "keep")],
        }
    }

    #[test]
    fn exact_endpoints_and_unicode_names_roundtrip_preserving_unknown_data_and_history() {
        let mut vmf = Document::empty().into_vmf();
        vmf.extra.push(Node::block(
            "other_editor",
            vec![Node::string("keep", "yes")],
        ));
        vmf.cameras = Some(Node::block(
            "cameras",
            vec![Node::string("activecamera", "-1")],
        ));
        let original = vmf.clone();
        let mut document = Document::new(vmf);
        let mut ruler = measurement(1);
        ruler.name = "Hall \"été\" \\ entrance".into();
        let metadata = Measurements {
            measurements: vec![ruler],
            extra: vec![Node::string("future_root", "keep")],
        };
        document
            .edit("ruler", |tx| metadata.clone().write(tx))
            .unwrap();
        let reopened = Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        assert_eq!(Measurements::parse(&reopened).unwrap(), metadata);
        assert_eq!(reopened.cameras, original.cameras);
        assert_eq!(reopened.extra[0], original.extra[0]);
        assert_eq!(reopened.world, original.world);
        assert_eq!(reopened.entities, original.entities);
        document.undo().unwrap();
        assert_eq!(document.vmf(), &original);
        document.redo().unwrap();
        assert_eq!(Measurements::parse(document.vmf()).unwrap(), metadata);
        let before = document.vmf().clone();
        let failed: Result<(), EditError> = document.edit("abort", |tx| {
            Measurements::default().write(tx)?;
            Err(EditError::Invalid("abort".into()))
        });
        assert!(failed.is_err());
        assert_eq!(document.vmf(), &before);
    }

    #[test]
    fn components_are_signed_and_distance_uses_all_three_axes() {
        let ruler = Measurement {
            start: DVec3::new(4.0, 7.0, 20.0),
            end: DVec3::new(1.0, 11.0, 8.0),
            ..measurement(1)
        };
        assert_eq!(ruler.components(), DVec3::new(-3.0, 4.0, -12.0));
        assert_eq!(ruler.distance(), 13.0);
    }

    #[test]
    fn names_counts_ids_and_endpoint_ranges_are_bounded() {
        let mut metadata = Measurements {
            measurements: (1..=MAX_MEASUREMENTS as u32).map(measurement).collect(),
            extra: Vec::new(),
        };
        metadata.validate().unwrap();
        metadata.measurements.push(measurement(129));
        assert!(metadata.validate().is_err());
        for name in [
            "".into(),
            " \t".into(),
            "x".repeat(MAX_NAME_CHARS + 1),
            "a\nb".into(),
        ] {
            assert!(
                Measurement {
                    name,
                    ..measurement(1)
                }
                .validate()
                .is_err()
            );
        }
        for point in [
            DVec3::splat(f64::NAN),
            DVec3::splat(f64::INFINITY),
            DVec3::splat(MAX_COORDINATE + 1.0),
        ] {
            for ruler in [
                Measurement {
                    start: point,
                    ..measurement(1)
                },
                Measurement {
                    end: point,
                    ..measurement(1)
                },
            ] {
                assert!(ruler.validate().is_err());
            }
        }
        Measurement {
            start: DVec3::splat(-MAX_COORDINATE),
            end: DVec3::splat(MAX_COORDINATE),
            ..measurement(1)
        }
        .validate()
        .unwrap();
        assert!(
            Measurement {
                start: DVec3::ZERO,
                end: DVec3::ZERO,
                ..measurement(1)
            }
            .validate()
            .is_err()
        );
        for second in [
            measurement(1),
            Measurement {
                name: " RULER 1 ".into(),
                ..measurement(2)
            },
        ] {
            assert!(
                Measurements {
                    measurements: vec![measurement(1), second],
                    extra: Vec::new()
                }
                .validate()
                .is_err()
            );
        }
    }

    #[test]
    fn malformed_imports_remain_untouched_and_cannot_be_overwritten() {
        let valid = measurement(1).to_node();
        let mut duplicate = valid.clone();
        if let source_formats::keyvalues::Value::Block(fields) = &mut duplicate.value {
            fields.push(Node::string("start", "0 0 0"));
        }
        let cases = vec![
            vec![Node::string(KEY, "bad")],
            vec![Node::block(KEY, vec![Node::string("version", "2")])],
            vec![Node::block(
                KEY,
                vec![Node::string("version", "1"), Node::string("version", "1")],
            )],
            vec![Node::block(
                KEY,
                vec![Node::string("version", "1"), duplicate],
            )],
            vec![Node::block(
                KEY,
                vec![Node::string("version", "1"), valid.clone(), valid],
            )],
            vec![
                Node::block(KEY, vec![Node::string("version", "1")]),
                Node::block(KEY, vec![Node::string("version", "1")]),
            ],
        ];
        for nodes in cases {
            let mut vmf = Document::empty().into_vmf();
            vmf.extra = nodes;
            let mut document = Document::new(vmf);
            let before = document.vmf().clone();
            assert!(Measurements::parse(document.vmf()).is_err());
            assert!(
                document
                    .edit("overwrite", |tx| Measurements::default().write(tx))
                    .is_err()
            );
            assert_eq!(document.vmf(), &before);
            assert_eq!(document.undo_labels().count(), 0);
            let reopened = Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
            assert_eq!(reopened.extra, before.extra);
        }
    }

    #[test]
    fn imported_records_obey_count_name_and_coordinate_limits_without_truncation() {
        let mut vmf = Document::empty().into_vmf();
        let mut records = vec![Node::string("version", "1")];
        records.extend((1..=MAX_MEASUREMENTS as u32 + 1).map(|id| measurement(id).to_node()));
        vmf.extra.push(Node::block(KEY, records));
        assert!(Measurements::parse(&vmf).unwrap_err().contains("at most"));
        let fields = measurement(1).to_node().children().unwrap().to_vec();
        for (key, value) in [
            ("id", "0".to_string()),
            ("name_base64", "!not base64!".to_string()),
            (
                "name_base64",
                STANDARD.encode("x".repeat(MAX_NAME_CHARS + 1)),
            ),
            ("start", "NaN 0 0".to_string()),
            ("end", "0 0 65537".to_string()),
            ("end", "1 2 3 4".to_string()),
        ] {
            let mut invalid = fields.clone();
            *invalid.iter_mut().find(|node| node.key == key).unwrap() = Node::string(key, value);
            vmf.extra = vec![Node::block(
                KEY,
                vec![
                    Node::string("version", "1"),
                    Node::block("measurement", invalid),
                ],
            )];
            let original = vmf.clone();
            assert!(Measurements::parse(&vmf).is_err());
            assert_eq!(vmf, original);
        }
    }
}
