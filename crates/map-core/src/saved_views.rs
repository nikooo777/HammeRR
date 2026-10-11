use std::collections::BTreeSet;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use glam::DVec3;
use source_formats::keyvalues::Node;
use source_formats::vmf::{Vmf, format_vec3, parse_vec3};

use crate::{EditError, Transaction};

pub const KEY: &str = "hammerr_saved_views";
pub const MAX_VIEWS: usize = 64;
pub const MAX_NAME_CHARS: usize = 80;
const FIELDS: &[&str] = &[
    "id",
    "name_base64",
    "projection",
    "position",
    "yaw",
    "pitch",
    "fov",
    "ortho_height",
];

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub id: u32,
    pub name: String,
    pub projection: String,
    pub position: DVec3,
    pub yaw: f64,
    pub pitch: f64,
    pub fov: f64,
    pub ortho_height: f64,
    pub extra: Vec<Node>,
}

impl View {
    pub fn validate(&self) -> Result<(), String> {
        if self.id == 0 {
            return Err("Saved view IDs must be positive".into());
        }
        if self.name.trim().is_empty()
            || self.name.chars().count() > MAX_NAME_CHARS
            || self.name.chars().any(char::is_control)
        {
            return Err(format!(
                "View names need 1–{MAX_NAME_CHARS} characters without control characters"
            ));
        }
        if !matches!(
            self.projection.as_str(),
            "perspective" | "top" | "front" | "side"
        ) {
            return Err("Unknown saved view projection".into());
        }
        if !self.position.is_finite()
            || self.position.abs().max_element() > 1_000_000.0
            || !self.yaw.is_finite()
            || self.yaw.abs() > 1_000_000.0
            || !self.pitch.is_finite()
            || !(-90.0..=90.0).contains(&self.pitch)
            || !self.fov.is_finite()
            || !(1.0..179.0).contains(&self.fov)
            || !self.ortho_height.is_finite()
            || !(1.0..=131_072.0).contains(&self.ortho_height)
        {
            return Err("Saved view camera values are outside the supported range".into());
        }
        Ok(())
    }

    fn to_node(&self) -> Node {
        let mut fields = vec![
            Node::string("id", self.id.to_string()),
            Node::string("name_base64", STANDARD.encode(&self.name)),
            Node::string("projection", &self.projection),
            Node::string("position", format_vec3(self.position)),
            Node::string("yaw", self.yaw.to_string()),
            Node::string("pitch", self.pitch.to_string()),
            Node::string("fov", self.fov.to_string()),
            Node::string("ortho_height", self.ortho_height.to_string()),
        ];
        fields.extend(self.extra.clone());
        Node::block("view", fields)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Views {
    pub views: Vec<View>,
    pub extra: Vec<Node>,
}

fn field<'a>(fields: &'a [Node], name: &str) -> Result<&'a str, String> {
    let mut matching = fields.iter().filter(|node| node.key == name);
    let value = matching
        .next()
        .and_then(Node::as_str)
        .ok_or_else(|| format!("Missing saved view field: {name}"))?;
    if matching.next().is_some() {
        return Err(format!("Duplicate saved view field: {name}"));
    }
    Ok(value)
}

impl Views {
    pub fn parse(vmf: &Vmf) -> Result<Self, String> {
        let mut matching = vmf.extra.iter().filter(|node| node.key == KEY);
        let Some(node) = matching.next() else {
            return Ok(Self::default());
        };
        if matching.next().is_some() {
            return Err("Multiple saved view metadata blocks".into());
        }
        let fields = node
            .children()
            .ok_or("Saved views must be a metadata block")?;
        if field(fields, "version")? != "1" {
            return Err("Unsupported saved view metadata version".into());
        }
        let mut result = Self::default();
        for node in fields {
            if node.key == "version" {
                continue;
            }
            if node.key != "view" {
                result.extra.push(node.clone());
                continue;
            }
            let fields = node.children().ok_or("Saved view must be a block")?;
            let number = |name| {
                field(fields, name)?
                    .parse::<f64>()
                    .map_err(|_| format!("Invalid saved view {name}"))
            };
            let name = String::from_utf8(
                STANDARD
                    .decode(field(fields, "name_base64")?)
                    .map_err(|_| "Invalid saved view name encoding")?,
            )
            .map_err(|_| "Saved view name is not UTF-8")?;
            result.views.push(View {
                id: field(fields, "id")?
                    .parse()
                    .map_err(|_| "Invalid saved view ID")?,
                name,
                projection: field(fields, "projection")?.into(),
                position: parse_vec3(field(fields, "position")?)
                    .ok_or("Invalid saved view position")?,
                yaw: number("yaw")?,
                pitch: number("pitch")?,
                fov: number("fov")?,
                ortho_height: number("ortho_height")?,
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
        if self.views.len() > MAX_VIEWS {
            return Err(format!("A map supports at most {MAX_VIEWS} saved views"));
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for view in &self.views {
            view.validate()?;
            if !ids.insert(view.id) {
                return Err("Duplicate saved view ID".into());
            }
            if !names.insert(view.name.trim().to_lowercase()) {
                return Err("A saved view already uses that name".into());
            }
        }
        Ok(())
    }

    pub fn next_id(&self) -> Result<u32, String> {
        self.views
            .iter()
            .map(|view| view.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| "Saved view IDs exhausted".into())
    }

    pub fn write(self, tx: &mut Transaction) -> Result<(), EditError> {
        self.validate().map_err(EditError::Invalid)?;
        let node = if self.views.is_empty() && self.extra.is_empty() {
            None
        } else {
            let mut nodes = vec![Node::string("version", "1")];
            nodes.extend(self.views.iter().map(View::to_node));
            nodes.extend(self.extra);
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

    fn view(id: u32) -> View {
        View {
            id,
            name: format!("View {id}"),
            projection: "perspective".into(),
            position: DVec3::new(12.123456789, -40.0, 80.0),
            yaw: 37.125,
            pitch: -20.0,
            fov: 60.0,
            ortho_height: 1024.0,
            extra: vec![Node::string("future", "keep")],
        }
    }

    #[test]
    fn views_roundtrip_unicode_quotes_and_unknown_metadata_with_undo() {
        let mut vmf = Document::empty().vmf().clone();
        vmf.extra
            .push(Node::block("unknown", vec![Node::string("keep", "yes")]));
        vmf.cameras = Some(Node::block(
            "cameras",
            vec![Node::string("activecamera", "-1")],
        ));
        let before = vmf.clone();
        let mut document = Document::new(vmf);
        let mut saved = view(1);
        saved.name = "Hall \"été\" \\ entry".into();
        let views = Views {
            views: vec![saved],
            extra: vec![Node::string("future_root", "keep")],
        };
        document
            .edit("save view", |tx| views.clone().write(tx))
            .unwrap();
        let written = document.vmf().write(&Default::default());
        let reopened = Vmf::parse(&written).unwrap();
        assert_eq!(Views::parse(&reopened).unwrap(), views);
        assert_eq!(reopened.cameras, before.cameras);
        assert_eq!(reopened.extra[0], before.extra[0]);
        document.undo().unwrap();
        assert_eq!(document.vmf(), &before);
        document.redo().unwrap();
        assert_eq!(Views::parse(document.vmf()).unwrap(), views);
        let current = document.vmf().clone();
        let result: Result<(), EditError> = document.edit("failed edit", |tx| {
            Views::default().write(tx)?;
            Err(EditError::Invalid("abort".into()))
        });
        assert!(result.is_err());
        assert_eq!(document.vmf(), &current);
    }

    #[test]
    fn invalid_names_cameras_counts_and_duplicate_ids_are_rejected() {
        let original = view(1);
        for name in [
            "".into(),
            " \t".into(),
            "x".repeat(MAX_NAME_CHARS + 1),
            "a\nb".into(),
        ] {
            assert!(
                View {
                    name,
                    ..original.clone()
                }
                .validate()
                .is_err()
            );
        }
        for position in [DVec3::splat(f64::NAN), DVec3::splat(1_000_001.0)] {
            assert!(
                View {
                    position,
                    ..original.clone()
                }
                .validate()
                .is_err()
            );
        }
        for invalid in [
            View {
                fov: 0.0,
                ..original.clone()
            },
            View {
                pitch: 91.0,
                ..original.clone()
            },
            View {
                yaw: f64::INFINITY,
                ..original.clone()
            },
            View {
                ortho_height: 0.0,
                ..original.clone()
            },
            View {
                projection: "unknown".into(),
                ..original.clone()
            },
        ] {
            assert!(invalid.validate().is_err());
        }
        assert!(
            Views {
                views: vec![original.clone(), original.clone()],
                ..Views::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            Views {
                views: vec![
                    original.clone(),
                    View {
                        id: 2,
                        name: "VIEW 1".into(),
                        ..original
                    }
                ],
                ..Views::default()
            }
            .validate()
            .is_err()
        );
        let mut views = Views {
            views: (1..=MAX_VIEWS as u32).map(view).collect(),
            ..Views::default()
        };
        views.validate().unwrap();
        views.views.push(view(65));
        assert!(views.validate().is_err());
    }

    #[test]
    fn malformed_or_future_metadata_is_preserved_and_cannot_be_overwritten() {
        let mut document = Document::empty();
        for node in [
            Node::string(KEY, "bad"),
            Node::block(KEY, vec![Node::string("version", "2")]),
            Node::block(
                KEY,
                vec![Node::string("version", "1"), Node::string("version", "1")],
            ),
        ] {
            let mut vmf = document.vmf().clone();
            vmf.extra.push(node);
            document = Document::new(vmf);
            assert!(Views::parse(document.vmf()).is_err());
            let before = document.vmf().clone();
            assert!(
                document
                    .edit("attempt", |tx| {
                        let views = Views::parse(tx.vmf()).map_err(EditError::Invalid)?;
                        views.write(tx)
                    })
                    .is_err()
            );
            assert_eq!(document.vmf(), &before);
            document = Document::empty();
        }
    }
}
