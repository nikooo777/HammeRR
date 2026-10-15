use glam::DVec3;
use source_formats::{
    keyvalues::{Node, Value},
    vmf::{Vmf, format_vec3, parse_vec3},
};

use crate::{EditError, Transaction};

pub const SHELL_THICKNESS: f64 = 16.0;
pub const MIN_EXTENT: f64 = 16.0;
pub const MAX_COORDINATE: f64 = crate::checks::SOURCE_COORDINATE_LIMIT - SHELL_THICKNESS;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub mins: DVec3,
    pub maxs: DVec3,
}

impl Bounds {
    pub fn validate(self) -> Result<Self, EditError> {
        if !self.mins.is_finite()
            || !self.maxs.is_finite()
            || self.mins.abs().max_element() > MAX_COORDINATE
            || self.maxs.abs().max_element() > MAX_COORDINATE
            || (self.maxs - self.mins).min_element() < MIN_EXTENT
        {
            return Err(invalid(format!(
                "bounds must be finite, within ±{MAX_COORDINATE}, and at least {MIN_EXTENT} units wide on every axis"
            )));
        }
        Ok(self)
    }

    pub fn contains(self, point: DVec3) -> bool {
        point.is_finite() && point.cmpge(self.mins).all() && point.cmple(self.maxs).all()
    }

    pub fn intersects(self, mins: DVec3, maxs: DVec3) -> bool {
        mins.cmple(self.maxs).all() && maxs.cmpge(self.mins).all()
    }

    pub fn shell(self) -> [(DVec3, DVec3); 6] {
        std::array::from_fn(|index| {
            let axis = index / 2;
            let upper = index % 2 == 1;
            let mut mins = self.mins - DVec3::splat(SHELL_THICKNESS);
            let mut maxs = self.maxs + DVec3::splat(SHELL_THICKNESS);
            for previous in 0..axis {
                mins[previous] = self.mins[previous];
                maxs[previous] = self.maxs[previous];
            }
            if upper {
                mins[axis] = self.maxs[axis];
            } else {
                maxs[axis] = self.mins[axis];
            }
            (mins, maxs)
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Definition {
    pub enabled: bool,
    pub bounds: Bounds,
}

fn invalid(message: impl std::fmt::Display) -> EditError {
    EditError::Invalid(format!("Imported cordon / compile region: {message}"))
}

fn is_region(node: &Node) -> bool {
    node.key.eq_ignore_ascii_case("cordon") || node.key.eq_ignore_ascii_case("cordons")
}

fn roots(vmf: &Vmf) -> impl Iterator<Item = &Node> {
    vmf.cordon
        .iter()
        .chain(&vmf.extra)
        .filter(|node| is_region(node))
}

fn children(node: &Node) -> Result<&[Node], EditError> {
    node.children()
        .ok_or_else(|| invalid("metadata must be a block"))
}

fn unique<'a>(nodes: &'a [Node], key: &str) -> Result<Option<&'a Node>, EditError> {
    let mut values = nodes
        .iter()
        .filter(|node| node.key.eq_ignore_ascii_case(key));
    let result = values.next();
    if values.next().is_some() {
        return Err(invalid(format!("repeated {key} fields")));
    }
    Ok(result)
}

fn enabled(nodes: &[Node]) -> Result<bool, EditError> {
    match unique(nodes, "active")? {
        None => Ok(false),
        Some(node) => match node.as_str().map(str::trim) {
            Some(value) if value == "0" || value.eq_ignore_ascii_case("false") => Ok(false),
            Some(value) if value == "1" || value.eq_ignore_ascii_case("true") => Ok(true),
            _ => Err(invalid("invalid active flag; use 0/1 or false/true")),
        },
    }
}

fn vector(nodes: &[Node], key: &str) -> Result<DVec3, EditError> {
    let text = unique(nodes, key)?
        .and_then(Node::as_str)
        .ok_or_else(|| invalid(format!("one {key} vector is required")))?;
    let text = text.trim();
    let text = match (text.strip_prefix('('), text.strip_suffix(')')) {
        (Some(_), Some(_)) => &text[1..text.len() - 1],
        (None, None) => text,
        _ => return Err(invalid(format!("invalid {key} vector"))),
    };
    parse_vec3(text).ok_or_else(|| invalid(format!("invalid {key} vector")))
}

fn bounds(nodes: &[Node]) -> Result<Bounds, EditError> {
    Bounds {
        mins: vector(nodes, "mins")?,
        maxs: vector(nodes, "maxs")?,
    }
    .validate()
}

fn definition(node: &Node, active_only: bool) -> Result<Definition, EditError> {
    let nodes = children(node)?;
    let active = enabled(nodes)?;
    if node.key.eq_ignore_ascii_case("cordon") {
        return Ok(Definition {
            enabled: active,
            bounds: bounds(nodes)?,
        });
    }
    let entries = nodes
        .iter()
        .filter(|child| child.key.eq_ignore_ascii_case("cordon"))
        .map(|child| {
            let entries = children(child)?;
            Ok((entries, enabled(entries)?))
        })
        .collect::<Result<Vec<_>, EditError>>()?;
    let entries = entries
        .into_iter()
        .filter(|(_, enabled)| !active_only || *enabled)
        .collect::<Vec<_>>();
    let [(entry, child_enabled)] = entries.as_slice() else {
        return Err(invalid(
            "exactly one region is supported; select one enabled cordon box",
        ));
    };
    let shape = unique(entry, "box")?.ok_or_else(|| invalid("one box block is required"))?;
    Ok(Definition {
        enabled: active && *child_enabled,
        bounds: bounds(children(shape)?)?,
    })
}

pub fn active(vmf: &Vmf) -> Result<Option<Bounds>, EditError> {
    let mut result = None;
    for node in roots(vmf) {
        if !enabled(children(node)?)? {
            continue;
        }
        if result.is_some() {
            return Err(invalid("multiple active root regions are unsupported"));
        }
        let region = definition(node, true)?;
        if !region.enabled {
            return Err(invalid("active metadata has no enabled region"));
        }
        result = Some(region.bounds);
    }
    Ok(result)
}

pub fn inspect(vmf: &Vmf) -> Result<Option<Definition>, EditError> {
    let mut nodes = roots(vmf);
    let Some(node) = nodes.next() else {
        return Ok(None);
    };
    if nodes.next().is_some() {
        return Err(invalid(
            "multiple metadata blocks; clear them explicitly before replacing the region",
        ));
    }
    definition(node, false).map(Some)
}

fn edited_node(vmf: &Vmf, definition: Option<Definition>) -> Result<Option<Node>, EditError> {
    let node = match definition {
        None => None,
        Some(definition) => {
            definition.bounds.validate()?;
            let existing = roots(vmf).collect::<Vec<_>>();
            if existing.len() > 1
                || existing
                    .first()
                    .is_some_and(|node| node.key.eq_ignore_ascii_case("cordons"))
            {
                return Err(invalid(
                    "clear imported plural/multiple records explicitly before authoring a replacement",
                ));
            }
            let mut node = existing
                .first()
                .map_or_else(|| Node::block("cordon", Vec::new()), |node| (*node).clone());
            let Value::Block(nodes) = &mut node.value else {
                return Err(invalid("clear malformed metadata before replacing it"));
            };
            for (key, value) in [
                (
                    "active",
                    if definition.enabled {
                        "1".to_owned()
                    } else {
                        "0".to_owned()
                    },
                ),
                ("mins", format!("({})", format_vec3(definition.bounds.mins))),
                ("maxs", format!("({})", format_vec3(definition.bounds.maxs))),
            ] {
                unique(nodes, key)?;
                if let Some(old) = nodes
                    .iter_mut()
                    .find(|node| node.key.eq_ignore_ascii_case(key))
                {
                    old.value = Value::String(value);
                } else {
                    nodes.push(Node::string(key, value));
                }
            }
            node.key = "cordon".into();
            Some(node)
        }
    };
    Ok(node)
}

pub fn validate_edit(vmf: &Vmf, definition: Option<Definition>) -> Result<(), EditError> {
    edited_node(vmf, definition).map(|_| ())
}

pub fn apply(tx: &mut Transaction, definition: Option<Definition>) -> Result<(), EditError> {
    let node = edited_node(tx.document().vmf(), definition)?;
    let keys = tx
        .document()
        .vmf()
        .extra
        .iter()
        .filter(|node| is_region(node))
        .map(|node| node.key.clone())
        .collect::<Vec<_>>();
    for key in keys {
        tx.set_editor_metadata(&key, None);
    }
    tx.set_cordon(node);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    fn definition(enabled: bool) -> Definition {
        Definition {
            enabled,
            bounds: Bounds {
                mins: DVec3::new(-128.0, -128.0, 0.0),
                maxs: DVec3::new(128.0, 128.0, 256.0),
            },
        }
    }

    #[test]
    fn native_metadata_edits_preserve_opaque_fields_history_and_failed_transactions() {
        let mut document = Document::empty();
        document
            .edit("enable region", |tx| apply(tx, Some(definition(true))))
            .unwrap();
        let active_source = document.vmf().clone();
        assert_eq!(
            active(document.vmf()).unwrap(),
            Some(definition(true).bounds)
        );
        let reopened = Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        assert_eq!(inspect(&reopened).unwrap(), Some(definition(true)));
        document.undo().unwrap();
        assert!(document.vmf().cordon.is_none());
        document.redo().unwrap();
        assert_eq!(document.vmf(), &active_source);
        let failed: Result<(), EditError> = document.edit("abort", |tx| {
            apply(tx, None)?;
            Err(invalid("abort"))
        });
        assert!(failed.is_err());
        assert_eq!(document.vmf(), &active_source);
        let mut source = active_source;
        let Value::Block(children) = &mut source.cordon.as_mut().unwrap().value else {
            unreachable!()
        };
        children.push(Node::string("vendor", "keep"));
        let mut document = Document::new(source);
        document
            .edit("disable", |tx| apply(tx, Some(definition(false))))
            .unwrap();
        assert!(active(document.vmf()).unwrap().is_none());
        assert!(document.vmf().write(&Default::default()).contains("keep"));
        let before = document.vmf().clone();
        document.edit("clear", |tx| apply(tx, None)).unwrap();
        document.undo().unwrap();
        assert_eq!(document.vmf(), &before);
    }

    #[test]
    fn imported_plural_scope_is_strict_and_globally_disabled_input_stays_disabled() {
        let node = Node::block(
            "cordons",
            vec![
                Node::string("active", "1"),
                Node::block(
                    "cordon",
                    vec![
                        Node::string("active", "1"),
                        Node::string("name", "Test"),
                        Node::block(
                            "box",
                            vec![
                                Node::string("mins", "(-128 -128 0)"),
                                Node::string("maxs", "(128 128 256)"),
                            ],
                        ),
                    ],
                ),
            ],
        );
        let mut source = Document::empty().into_vmf();
        source.extra.push(node);
        assert_eq!(active(&source).unwrap(), Some(definition(true).bounds));
        let mut document = Document::new(source.clone());
        assert!(
            document
                .edit("replace", |tx| apply(tx, Some(definition(false))))
                .is_err()
        );
        assert_eq!(document.vmf(), &source);
        let Value::Block(nodes) = &mut source.extra[0].value else {
            unreachable!()
        };
        nodes.push(Node::block("cordon", vec![Node::string("active", "1")]));
        assert!(active(&source).is_err());
        let Value::Block(nodes) = &mut source.extra[0].value else {
            unreachable!()
        };
        nodes[0] = Node::string("active", "0");
        assert_eq!(active(&source).unwrap(), None);
    }
}
