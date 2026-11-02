use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use source_formats::keyvalues::{self, Node, WriteOptions};

pub const MAX_CATEGORY_BYTES: usize = 48;
pub const MAX_TAGS: usize = 8;
pub const MAX_TAG_BYTES: usize = 32;
const KEY: &str = "hammerr_prefab";
const VERSION: &str = "1";

#[derive(
    Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct Organization {
    pub category: String,
    pub tags: Vec<String>,
}

impl Organization {
    pub fn normalized(&self) -> Result<Self, String> {
        if self.tags.len() > MAX_TAGS {
            return Err(format!("A prefab supports at most {MAX_TAGS} tags"));
        }
        if self.category.chars().any(char::is_control)
            || self
                .tags
                .iter()
                .any(|tag| tag.chars().any(char::is_control))
        {
            return Err("Prefab categories and tags cannot contain control characters".into());
        }
        let organization = Self {
            category: self.category.trim().into(),
            tags: self.tags.iter().map(|tag| tag.trim().into()).collect(),
        };
        organization.validate()?;
        Ok(organization)
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_text(&self.category, MAX_CATEGORY_BYTES, true, "Prefab category")?;
        if self.tags.len() > MAX_TAGS {
            return Err(format!("A prefab supports at most {MAX_TAGS} tags"));
        }
        let mut names = HashSet::new();
        for tag in &self.tags {
            validate_text(tag, MAX_TAG_BYTES, false, "Prefab tag")?;
            if !names.insert(tag.to_lowercase()) {
                return Err("Prefab tags must be unique without regard to case".into());
            }
        }
        Ok(())
    }
}

fn validate_text(value: &str, limit: usize, empty: bool, label: &str) -> Result<(), String> {
    if (!empty && value.is_empty())
        || value.len() > limit
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "{label} must {}fit in {limit} UTF-8 bytes, without surrounding whitespace or control characters",
            if empty { "" } else { "be nonempty and " }
        ));
    }
    Ok(())
}

fn field<'a>(nodes: &'a [Node], key: &str) -> Result<&'a Node, String> {
    let mut matches = nodes.iter().filter(|node| node.key == key);
    let node = matches
        .next()
        .ok_or_else(|| format!("Missing prefab organization field: {key}"))?;
    if matches.next().is_some() {
        return Err(format!("Duplicate prefab organization field: {key}"));
    }
    Ok(node)
}

fn block(nodes: &[Node]) -> Result<Option<&Node>, String> {
    let mut matches = nodes.iter().filter(|node| node.key == KEY);
    let node = matches.next();
    if matches.next().is_some() {
        return Err("Duplicate prefab organization metadata blocks".into());
    }
    Ok(node)
}

fn parse_nodes(nodes: &[Node]) -> Result<Organization, String> {
    let Some(node) = block(nodes)? else {
        return Ok(Organization::default());
    };
    let fields = node
        .children()
        .ok_or("Prefab organization metadata must be a block")?;
    if field(fields, "version")?.as_str() != Some(VERSION) {
        return Err("Unsupported prefab organization metadata version".into());
    }
    let category = field(fields, "category")?
        .as_str()
        .ok_or("Prefab category must be text")?
        .to_owned();
    let tags = field(fields, "tags")?
        .children()
        .ok_or("Prefab tags must be a block")?
        .iter()
        .filter(|node| node.key == "tag")
        .take(MAX_TAGS + 1)
        .map(|node| {
            node.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "Prefab tag must be text".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let organization = Organization { category, tags };
    organization.validate()?;
    Ok(organization)
}

fn nodes(text: &str) -> Result<Vec<Node>, String> {
    if text.len() > map_core::transfer::MAX_BYTES {
        return Err("Prefab VMF exceeds 8 MiB".into());
    }
    keyvalues::parse_bounded(
        text,
        map_core::transfer::MAX_DEPTH,
        map_core::transfer::MAX_NODES,
    )
    .map_err(|error| format!("Prefab VMF: {error}"))
}

pub(super) fn parse(text: &str) -> Result<Organization, String> {
    parse_nodes(&nodes(text)?)
}

fn rewrite_tags(previous: &Node, tags: &[String]) -> Node {
    let mut remaining = tags.iter();
    let mut fields = Vec::new();
    for node in previous.children().unwrap() {
        if node.key == "tag" {
            if let Some(tag) = remaining.next() {
                fields.push(if node.as_str() == Some(tag.as_str()) {
                    node.clone()
                } else {
                    Node::string("tag", tag)
                });
            }
        } else {
            fields.push(node.clone());
        }
    }
    fields.extend(remaining.map(|tag| Node::string("tag", tag)));
    Node::block("tags", fields)
}

fn rewritten(node: Option<&Node>, organization: &Organization) -> Node {
    let Some(node) = node else {
        return Node::block(
            KEY,
            vec![
                Node::string("version", VERSION),
                Node::string("category", &organization.category),
                Node::block(
                    "tags",
                    organization
                        .tags
                        .iter()
                        .map(|tag| Node::string("tag", tag))
                        .collect(),
                ),
            ],
        );
    };
    let fields = node
        .children()
        .unwrap()
        .iter()
        .map(|node| match node.key.as_str() {
            "category" if node.as_str() != Some(organization.category.as_str()) => {
                Node::string("category", &organization.category)
            }
            "tags" => rewrite_tags(node, &organization.tags),
            _ => node.clone(),
        })
        .collect();
    Node::block(KEY, fields)
}

fn serialize(nodes: &[Node]) -> Result<String, String> {
    let text =
        keyvalues::try_write(nodes, &WriteOptions::default()).map_err(|error| error.to_string())?;
    if text.len() > map_core::transfer::MAX_BYTES {
        return Err("Prefab organization exceeds the 8 MiB VMF limit".into());
    }
    Ok(text)
}

pub(super) fn organize(text: &str, organization: &Organization) -> Result<String, String> {
    let organization = organization.normalized()?;
    let mut nodes = nodes(text)?;
    let previous = parse_nodes(&nodes)?;
    if previous == organization && block(&nodes)?.is_some() {
        return Ok(text.to_owned());
    }
    let rewritten = rewritten(block(&nodes)?, &organization);
    if let Some(index) = nodes.iter().position(|node| node.key == KEY) {
        nodes[index] = rewritten;
    } else {
        nodes.push(rewritten);
    }
    serialize(&nodes)
}

pub(super) fn replace(previous: &str, replacement: &str) -> Result<String, String> {
    let previous = nodes(previous)?;
    parse_nodes(&previous)?;
    let mut replacement = nodes(replacement)?;
    parse_nodes(&replacement)?;
    let organization = block(&previous)?
        .cloned()
        .unwrap_or_else(|| rewritten(None, &Organization::default()));
    if let Some(index) = replacement.iter().position(|node| node.key == KEY) {
        replacement[index] = organization;
    } else {
        replacement.push(organization);
    }
    serialize(&replacement)
}
