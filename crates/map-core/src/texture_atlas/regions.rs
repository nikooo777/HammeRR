use std::collections::BTreeSet;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use source_formats::gamefs::asset_relative;
use source_formats::keyvalues::Node;
use source_formats::vmf::Vmf;

use super::{FitRegion, Rectangle};
use crate::{EditError, Transaction};

pub const KEY: &str = "hammerr_atlas_regions";
pub const MAX_REGIONS: usize = 128;
pub const MAX_NAME_CHARS: usize = 80;
pub const MAX_TEXTURE_AXIS: u32 = 8192;
const MAX_PATH_BYTES: usize = 512;
const MAX_ENCODED_NAME_BYTES: usize = MAX_NAME_CHARS * 4 * 2;
const FIELDS: &[&str] = &[
    "id",
    "name_base64",
    "material",
    "image_identity",
    "image_width",
    "image_height",
    "x",
    "y",
    "width",
    "height",
    "padding",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub id: u32,
    pub name: String,
    pub material: String,
    pub image_identity: String,
    pub dimensions: [u32; 2],
    pub region: FitRegion,
    pub extra: Vec<Node>,
}

impl Region {
    pub fn validate(&self) -> Result<(), String> {
        if self.id == 0 {
            return Err("Atlas region IDs must be positive".into());
        }
        if self.name.trim().is_empty()
            || self.name.chars().count() > MAX_NAME_CHARS
            || self.name.chars().any(char::is_control)
        {
            return Err(format!(
                "Atlas region names need 1–{MAX_NAME_CHARS} characters without control characters"
            ));
        }
        if self.material.is_empty()
            || self.material.len() > MAX_PATH_BYTES
            || self.material.chars().any(char::is_control)
            || self.material.contains(['*', '?'])
            || self.material.starts_with("materials/")
            || self.material.ends_with(".vmt")
            || asset_relative(&self.material).ok().as_deref() != Some(self.material.as_str())
            || self.material.split('/').any(str::is_empty)
        {
            return Err("Atlas regions require a canonical material path".into());
        }
        if self.image_identity.len() != 64
            || !self
                .image_identity
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("Invalid atlas image identity".into());
        }
        if self.dimensions.iter().any(|size| *size > MAX_TEXTURE_AXIS) {
            return Err("Saved atlas dimensions exceed 8192 pixels per axis".into());
        }
        self.region
            .usable_rectangle(self.dimensions)
            .map_err(|error| error.to_string())?;
        if self
            .extra
            .iter()
            .any(|node| FIELDS.contains(&node.key.as_str()))
        {
            return Err("Atlas region extension fields conflict with required fields".into());
        }
        crate::metadata::InputBudget::default()
            .nodes(&self.extra)
            .map_err(|error| error.to_string())
    }

    fn to_node(&self) -> Node {
        let mut fields = vec![
            Node::string("id", self.id.to_string()),
            Node::string("name_base64", STANDARD.encode(&self.name)),
            Node::string("material", &self.material),
            Node::string("image_identity", &self.image_identity),
            Node::string("image_width", self.dimensions[0].to_string()),
            Node::string("image_height", self.dimensions[1].to_string()),
            Node::string("x", self.region.rectangle.x.to_string()),
            Node::string("y", self.region.rectangle.y.to_string()),
            Node::string("width", self.region.rectangle.width.to_string()),
            Node::string("height", self.region.rectangle.height.to_string()),
            Node::string("padding", self.region.padding.to_string()),
        ];
        fields.extend(self.extra.clone());
        Node::block("region", fields)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Regions {
    pub regions: Vec<Region>,
    pub extra: Vec<Node>,
}

fn field<'a>(fields: &'a [Node], key: &str) -> Result<&'a str, String> {
    let mut matching = fields.iter().filter(|node| node.key == key);
    let value = matching
        .next()
        .and_then(Node::as_str)
        .ok_or_else(|| format!("Missing or invalid atlas region field: {key}"))?;
    if matching.next().is_some() {
        return Err(format!("Duplicate atlas region field: {key}"));
    }
    Ok(value)
}

fn validate_metadata(node: &Node) -> Result<(), String> {
    crate::metadata::InputBudget::default()
        .nodes(std::slice::from_ref(node))
        .map_err(|error| error.to_string())
}

impl Regions {
    pub fn parse(vmf: &Vmf) -> Result<Self, String> {
        let mut matching = vmf.extra.iter().filter(|node| node.key == KEY);
        let Some(node) = matching.next() else {
            return Ok(Self::default());
        };
        if matching.next().is_some() {
            return Err("Multiple atlas region metadata blocks; metadata is preserved".into());
        }
        validate_metadata(node)?;
        let fields = node
            .children()
            .ok_or("Atlas regions must be a metadata block")?;
        if field(fields, "version")? != "1" {
            return Err("Unsupported atlas region metadata version; metadata is preserved".into());
        }
        let mut result = Self::default();
        for node in fields {
            if node.key == "version" {
                continue;
            }
            if node.key != "region" {
                result.extra.push(node.clone());
                continue;
            }
            if result.regions.len() == MAX_REGIONS {
                return Err(format!(
                    "A map supports at most {MAX_REGIONS} atlas regions"
                ));
            }
            let fields = node.children().ok_or("Atlas region must be a block")?;
            let encoded = field(fields, "name_base64")?;
            if encoded.len() > MAX_ENCODED_NAME_BYTES {
                return Err("Encoded atlas region name exceeds the supported limit".into());
            }
            let name = String::from_utf8(
                STANDARD
                    .decode(encoded)
                    .map_err(|_| "Invalid atlas region name encoding")?,
            )
            .map_err(|_| "Atlas region name is not UTF-8")?;
            let number = |key| {
                field(fields, key)?
                    .parse::<u32>()
                    .map_err(|_| format!("Invalid atlas region {key}"))
            };
            result.regions.push(Region {
                id: number("id")?,
                name,
                material: field(fields, "material")?.into(),
                image_identity: field(fields, "image_identity")?.into(),
                dimensions: [number("image_width")?, number("image_height")?],
                region: FitRegion {
                    rectangle: Rectangle {
                        x: number("x")?,
                        y: number("y")?,
                        width: number("width")?,
                        height: number("height")?,
                    },
                    padding: number("padding")?,
                },
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
        if self.regions.len() > MAX_REGIONS {
            return Err(format!(
                "A map supports at most {MAX_REGIONS} atlas regions"
            ));
        }
        if self
            .extra
            .iter()
            .any(|node| matches!(node.key.as_str(), "version" | "region"))
        {
            return Err("Atlas catalog extension fields conflict with required fields".into());
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for region in &self.regions {
            region.validate()?;
            if !ids.insert(region.id) {
                return Err("Duplicate atlas region ID".into());
            }
            if !names.insert((&region.material, region.name.trim().to_lowercase())) {
                return Err("An atlas region already uses that name for this material".into());
            }
        }
        let mut nodes = vec![Node::string("version", "1")];
        nodes.extend(self.regions.iter().map(Region::to_node));
        nodes.extend(self.extra.clone());
        validate_metadata(&Node::block(KEY, nodes))
    }

    pub fn next_id(&self) -> Result<u32, String> {
        self.regions
            .iter()
            .map(|region| region.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| "Atlas region IDs exhausted".into())
    }

    pub fn write(self, tx: &mut Transaction) -> Result<(), EditError> {
        let previous = Self::parse(tx.vmf()).map_err(EditError::Invalid)?;
        self.validate().map_err(EditError::Invalid)?;
        let node = if self.regions.is_empty() && self.extra.is_empty() {
            None
        } else {
            let original = tx.vmf().extra.iter().find(|node| node.key == KEY);
            let mut pending = self.regions;
            let mut extra = self.extra;
            let mut nodes = Vec::new();
            for node in original.and_then(Node::children).unwrap_or_default() {
                if node.key == "version" {
                    nodes.push(node.clone());
                } else if node.key == "region" {
                    let id = field(node.children().unwrap(), "id")
                        .unwrap()
                        .parse::<u32>()
                        .unwrap();
                    if let Some(index) = pending.iter().position(|region| region.id == id) {
                        let region = pending.remove(index);
                        let old = previous
                            .regions
                            .iter()
                            .find(|region| region.id == id)
                            .unwrap();
                        nodes.push(if &region == old {
                            node.clone()
                        } else {
                            region.to_node()
                        });
                    }
                } else if let Some(index) = extra.iter().position(|entry| entry == node) {
                    nodes.push(extra.remove(index));
                }
            }
            if original.is_none() {
                nodes.push(Node::string("version", "1"));
            }
            nodes.extend(pending.iter().map(Region::to_node));
            nodes.extend(extra);
            Some(Node::block(KEY, nodes))
        };
        if let Some(node) = &node {
            validate_metadata(node).map_err(EditError::Invalid)?;
        }
        tx.set_editor_metadata(KEY, node);
        Ok(())
    }
}
