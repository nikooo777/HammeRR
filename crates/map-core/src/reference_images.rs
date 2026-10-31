use std::collections::BTreeSet;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use glam::{DQuat, DVec3, EulerRot};
use source_formats::keyvalues::Node;
use source_formats::vmf::Vmf;

use crate::{EditError, Transaction};

pub const KEY: &str = "hammerr_reference_images";
pub const MAX_IMAGES: usize = 8;
pub const MAX_AXIS: u32 = 4096;
pub const MAX_PIXELS: u64 = 4 * 1024 * 1024;
pub const MAX_TOTAL_PIXELS: u64 = 16 * 1024 * 1024;
pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_NAME_CHARACTERS: usize = 80;
const MAX_COORDINATE: f64 = 1_000_000.0;
const FIELDS: &[&str] = &[
    "id",
    "name_base64",
    "path_base64",
    "sha256",
    "width",
    "height",
    "position",
    "rotation",
    "size",
    "opacity",
    "flip_x",
    "flip_y",
    "top",
    "front",
    "side",
    "perspective",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub id: u32,
    pub name: String,
    pub path: String,
    pub sha256: String,
    pub dimensions: [u32; 2],
    pub position: [f64; 3],
    pub rotation: [f64; 3],
    pub size: [f64; 2],
    pub opacity: f64,
    pub flip: [bool; 2],
    pub views: [bool; 4],
    pub extra: Vec<Node>,
}

impl Image {
    pub fn validate(&self) -> Result<(), String> {
        if self.id == 0 {
            return Err("Reference image IDs must be positive".into());
        }
        validate_name(&self.name)?;
        validate_path(&self.path)?;
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("Invalid reference image SHA256 identity".into());
        }
        validate_dimensions(self.dimensions)?;
        if self
            .position
            .iter()
            .chain(&self.rotation)
            .any(|value| !value.is_finite() || value.abs() > MAX_COORDINATE)
            || self
                .size
                .iter()
                .any(|value| !value.is_finite() || !(0.001..=MAX_COORDINATE).contains(value))
            || !self.opacity.is_finite()
            || !(0.0..=1.0).contains(&self.opacity)
        {
            return Err("Reference image position/rotation must be finite within ±1000000, size 0.001–1000000 units, opacity 0–1".into());
        }
        let corners = self.corners().map(|corner| corner.as_vec3());
        if corners.iter().any(|corner| !corner.is_finite())
            || (corners[1] - corners[0])
                .cross(corners[3] - corners[0])
                .length_squared()
                == 0.0
        {
            return Err("Reference image size cannot be represented at this position; increase its size or move it nearer the origin".into());
        }
        Ok(())
    }

    pub fn corners(&self) -> [DVec3; 4] {
        let angles = self.rotation.map(f64::to_radians);
        let rotation = DQuat::from_euler(EulerRot::XYZ, angles[0], angles[1], angles[2]);
        let right = rotation * DVec3::X * self.size[0] * 0.5;
        let up = rotation * DVec3::Y * self.size[1] * 0.5;
        let center = DVec3::from_array(self.position);
        [
            center - right + up,
            center - right - up,
            center + right - up,
            center + right + up,
        ]
    }

    fn to_node(&self) -> Node {
        let mut fields = vec![
            Node::string("id", self.id.to_string()),
            Node::string("name_base64", STANDARD.encode(&self.name)),
            Node::string("path_base64", STANDARD.encode(&self.path)),
            Node::string("sha256", &self.sha256),
            Node::string("width", self.dimensions[0].to_string()),
            Node::string("height", self.dimensions[1].to_string()),
            Node::string("position", numbers(self.position)),
            Node::string("rotation", numbers(self.rotation)),
            Node::string("size", numbers(self.size)),
            Node::string("opacity", self.opacity.to_string()),
        ];
        for (key, value) in ["flip_x", "flip_y"].into_iter().zip(self.flip).chain(
            ["top", "front", "side", "perspective"]
                .into_iter()
                .zip(self.views),
        ) {
            fields.push(Node::string(key, if value { "1" } else { "0" }));
        }
        fields.extend(self.extra.clone());
        Node::block("image", fields)
    }
}

pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.trim() != name
        || name.chars().count() > MAX_NAME_CHARACTERS
        || name.len() > 256
        || name.chars().any(char::is_control)
    {
        return Err("Reference image names need 1–80 characters, at most 256 UTF-8 bytes, without controls or surrounding whitespace".into());
    }
    Ok(())
}

pub fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || !std::path::Path::new(path).is_absolute()
        || path.chars().any(char::is_control)
    {
        return Err("Reference images require an absolute UTF-8 file path without control characters, at most 4096 bytes".into());
    }
    Ok(())
}

pub fn validate_dimensions([width, height]: [u32; 2]) -> Result<(), String> {
    if width == 0
        || height == 0
        || width > MAX_AXIS
        || height > MAX_AXIS
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(
            "Reference images require 1–4096 pixels per axis and at most 4 megapixels".into(),
        );
    }
    Ok(())
}

fn numbers<const N: usize>(values: [f64; N]) -> String {
    values.map(|value| value.to_string()).join(" ")
}
fn field<'a>(fields: &'a [Node], key: &str) -> Result<&'a str, String> {
    let mut found = fields.iter().filter(|node| node.key == key);
    let value = found
        .next()
        .and_then(Node::as_str)
        .ok_or_else(|| format!("Missing or invalid reference image field: {key}"))?;
    if found.next().is_some() {
        return Err(format!("Duplicate reference image field: {key}"));
    }
    Ok(value)
}
fn vector<const N: usize>(fields: &[Node], key: &str) -> Result<[f64; N], String> {
    let values = field(fields, key)?
        .split_whitespace()
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| format!("Invalid reference image {key}"))?;
    values
        .try_into()
        .map_err(|_| format!("Reference image {key} needs {N} numbers"))
}
fn text(fields: &[Node], key: &str, limit: usize) -> Result<String, String> {
    let encoded = field(fields, key)?;
    if encoded.len() > limit * 2 {
        return Err(format!("Reference image {key} exceeds its byte limit"));
    }
    String::from_utf8(
        STANDARD
            .decode(encoded)
            .map_err(|_| format!("Invalid reference image {key} encoding"))?,
    )
    .map_err(|_| format!("Reference image {key} is not UTF-8"))
}
fn flag(fields: &[Node], key: &str) -> Result<bool, String> {
    match field(fields, key)? {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("Reference image {key} requires 0 or 1")),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub images: Vec<Image>,
    pub extra: Vec<Node>,
}

impl Metadata {
    pub fn parse(vmf: &Vmf) -> Result<Self, String> {
        let mut found = vmf.extra.iter().filter(|node| node.key == KEY);
        let Some(node) = found.next() else {
            return Ok(Self::default());
        };
        if found.next().is_some() {
            return Err("Duplicate reference image metadata blocks; metadata is preserved".into());
        }
        crate::metadata::InputBudget::default()
            .nodes(std::slice::from_ref(node))
            .map_err(|error| error.to_string())?;
        let fields = node
            .children()
            .ok_or("Reference images require a metadata block")?;
        if field(fields, "version")? != "1" {
            return Err(
                "Unsupported reference image metadata version; metadata is preserved".into(),
            );
        }
        let mut result = Self::default();
        for node in fields {
            if node.key == "version" {
                continue;
            }
            if node.key != "image" {
                result.extra.push(node.clone());
                continue;
            }
            if result.images.len() == MAX_IMAGES {
                return Err("At most 8 reference images are supported".into());
            }
            let fields = node.children().ok_or("Reference image must be a block")?;
            let number = |key| {
                field(fields, key)?
                    .parse::<u32>()
                    .map_err(|_| format!("Invalid reference image {key}"))
            };
            result.images.push(Image {
                id: number("id")?,
                name: text(fields, "name_base64", 256)?,
                path: text(fields, "path_base64", MAX_PATH_BYTES)?,
                sha256: field(fields, "sha256")?.into(),
                dimensions: [number("width")?, number("height")?],
                position: vector(fields, "position")?,
                rotation: vector(fields, "rotation")?,
                size: vector(fields, "size")?,
                opacity: field(fields, "opacity")?
                    .parse()
                    .map_err(|_| "Invalid reference image opacity")?,
                flip: [flag(fields, "flip_x")?, flag(fields, "flip_y")?],
                views: [
                    flag(fields, "top")?,
                    flag(fields, "front")?,
                    flag(fields, "side")?,
                    flag(fields, "perspective")?,
                ],
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
        if self.images.len() > MAX_IMAGES {
            return Err("At most 8 reference images are supported".into());
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut pixels = 0;
        for image in &self.images {
            image.validate()?;
            if !ids.insert(image.id) || !names.insert(&image.name) {
                return Err("Reference image IDs and exact names must be unique".into());
            }
            pixels += u64::from(image.dimensions[0]) * u64::from(image.dimensions[1]);
        }
        if pixels > MAX_TOTAL_PIXELS {
            return Err(
                "Reference images exceed the map's 16-megapixel decoded image budget".into(),
            );
        }
        Ok(())
    }

    pub fn next_id(&self) -> Result<u32, String> {
        self.images
            .iter()
            .map(|image| image.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| "Reference image IDs exhausted".into())
    }

    pub fn write(self, tx: &mut Transaction) -> Result<(), EditError> {
        Self::parse(tx.vmf()).map_err(EditError::Invalid)?;
        self.validate().map_err(EditError::Invalid)?;
        let node = if self.images.is_empty() && self.extra.is_empty() {
            None
        } else {
            let mut nodes = vec![Node::string("version", "1")];
            nodes.extend(self.images.iter().map(Image::to_node));
            nodes.extend(self.extra);
            let node = Node::block(KEY, nodes);
            crate::metadata::InputBudget::default().nodes(std::slice::from_ref(&node))?;
            Some(node)
        };
        tx.set_editor_metadata(KEY, node);
        Ok(())
    }
}
