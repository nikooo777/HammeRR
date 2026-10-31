use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use source_formats::keyvalues::Node;
use source_formats::vmf::Vmf;

use crate::{EditError, Transaction};

pub const KEY: &str = "hammerr_color_palette";
const FIELDS: &[&str] = &["name_base64", "rgb1"];

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub rgb1: [f64; 3],
    pub extra: Vec<Node>,
}

impl Entry {
    fn to_node(&self) -> Node {
        let mut fields = vec![
            Node::string("name_base64", STANDARD.encode(&self.name)),
            Node::string("rgb1", self.rgb1.map(|value| value.to_string()).join(" ")),
        ];
        fields.extend(self.extra.clone());
        Node::block("color", fields)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub entries: Vec<Entry>,
    pub extra: Vec<Node>,
}

fn field<'a>(fields: &'a [Node], key: &str) -> Result<&'a str, String> {
    let mut matching = fields.iter().filter(|node| node.key == key);
    let value = matching
        .next()
        .and_then(Node::as_str)
        .ok_or_else(|| format!("Missing or invalid map palette field: {key}"))?;
    if matching.next().is_some() {
        return Err(format!("Duplicate map palette field: {key}"));
    }
    Ok(value)
}

fn validate_metadata(node: &Node) -> Result<(), String> {
    crate::metadata::InputBudget::default()
        .nodes(std::slice::from_ref(node))
        .map_err(|error| error.to_string())
}

impl Metadata {
    pub fn parse(vmf: &Vmf) -> Result<Self, String> {
        let mut matching = vmf.extra.iter().filter(|node| node.key == KEY);
        let Some(node) = matching.next() else {
            return Ok(Self::default());
        };
        if matching.next().is_some() {
            return Err("Multiple map palette metadata blocks; metadata is preserved".into());
        }
        validate_metadata(node)?;
        let fields = node
            .children()
            .ok_or("Map palette must be a metadata block")?;
        if field(fields, "version")? != "1" {
            return Err("Unsupported map palette metadata version; metadata is preserved".into());
        }
        let mut result = Self::default();
        for node in fields {
            if node.key == "version" {
                continue;
            }
            if node.key != "color" {
                result.extra.push(node.clone());
                continue;
            }
            let fields = node.children().ok_or("Map palette color must be a block")?;
            let name = String::from_utf8(
                STANDARD
                    .decode(field(fields, "name_base64")?)
                    .map_err(|_| "Invalid map palette name encoding")?,
            )
            .map_err(|_| "Map palette name is not UTF-8")?;
            let channels = field(fields, "rgb1")?
                .split_whitespace()
                .map(str::parse::<f64>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "Invalid map palette RGB numbers")?;
            let rgb1 = channels
                .try_into()
                .map_err(|_| "Map palette colors require three RGB numbers")?;
            result.entries.push(Entry {
                name,
                rgb1,
                extra: fields
                    .iter()
                    .filter(|node| !FIELDS.contains(&node.key.as_str()))
                    .cloned()
                    .collect(),
            });
        }
        Ok(result)
    }

    pub fn write(self, tx: &mut Transaction) -> Result<(), EditError> {
        Self::parse(tx.vmf()).map_err(EditError::Invalid)?;
        let node = if self.entries.is_empty() && self.extra.is_empty() {
            None
        } else {
            let mut fields = vec![Node::string("version", "1")];
            fields.extend(self.entries.iter().map(Entry::to_node));
            fields.extend(self.extra);
            let node = Node::block(KEY, fields);
            validate_metadata(&node).map_err(EditError::Invalid)?;
            Some(node)
        };
        tx.set_editor_metadata(KEY, node);
        Ok(())
    }
}
