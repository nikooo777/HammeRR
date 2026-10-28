use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use glam::DVec3;
use source_formats::keyvalues::Node;
use source_formats::vmf::{Vmf, format_vec3, parse_vec3};

use crate::{Document, EditError, ObjectId, Transaction};

pub const METADATA_KEY: &str = "hammerr_context";
pub const MAX_SUMMARY_CHARS: usize = 200;
pub const MAX_NOTE_BYTES: usize = 4096;
pub const MAX_MAP_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_REGIONS: usize = 256;
pub const DEFAULT_CONTEXT_BYTES: usize = 8192;
pub const MAX_REGION_COORDINATE: f64 = 1_000_000.0;
const VERSION: &str = "1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    Map,
    Object(ObjectId),
    Region(u32),
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Map => f.write_str("map"),
            Self::Object(ObjectId::Solid(id)) => write!(f, "solid:{id}"),
            Self::Object(ObjectId::Entity(id)) => write!(f, "entity:{id}"),
            Self::Region(id) => write!(f, "region:{id}"),
        }
    }
}

impl Scope {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value == "map" {
            return Ok(Self::Map);
        }
        let (kind, id) = value
            .split_once(':')
            .ok_or("Use map, solid:<id>, entity:<id> or region:<id>")?;
        let id = id.parse::<u32>().map_err(|_| "Invalid context scope ID")?;
        if id == 0 {
            return Err("Context attachment IDs must be positive".into());
        }
        match kind {
            "solid" => Ok(Self::Object(ObjectId::Solid(id))),
            "entity" => Ok(Self::Object(ObjectId::Entity(id))),
            "region" => Ok(Self::Region(id)),
            _ => Err("Unknown context scope".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub mins: DVec3,
    pub maxs: DVec3,
    pub color: [u8; 3],
}

impl Region {
    pub fn validate(&self) -> Result<(), String> {
        if !self.mins.is_finite()
            || !self.maxs.is_finite()
            || !self.mins.cmplt(self.maxs).all()
            || self.mins.abs().max(self.maxs.abs()).max_element() > MAX_REGION_COORDINATE
        {
            return Err(
                "Region bounds must be finite, within ±1000000 units, with minimum below maximum on every axis".into(),
            );
        }
        Ok(())
    }

    pub fn intersects(&self, mins: DVec3, maxs: DVec3) -> bool {
        self.mins.cmple(maxs).all() && self.maxs.cmpge(mins).all()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub summary: String,
    pub body: String,
    pub author: String,
    pub observed: String,
    pub region: Option<Region>,
    pub extra: Vec<Node>,
}

impl Note {
    pub fn text_bytes(&self) -> usize {
        self.summary.len() + self.body.len()
    }

    pub fn validate(&self, scope: Scope) -> Result<(), String> {
        if matches!(
            scope,
            Scope::Object(ObjectId::Solid(0) | ObjectId::Entity(0)) | Scope::Region(0)
        ) {
            return Err("Context attachment IDs must be positive".into());
        }
        if self.summary.chars().any(char::is_control) || self.body.contains('\0') {
            return Err(
                "Summary must be a single line and note text cannot contain NUL characters".into(),
            );
        }
        if self.summary.trim().is_empty() || self.summary.chars().count() > MAX_SUMMARY_CHARS {
            return Err(format!(
                "Note summary must contain 1–{MAX_SUMMARY_CHARS} characters"
            ));
        }
        if self.text_bytes() > MAX_NOTE_BYTES {
            return Err(format!(
                "Note summary and body exceed the {MAX_NOTE_BYTES}-byte limit; link to a project document for longer context"
            ));
        }
        if !matches!(self.author.as_str(), "human" | "agent" | "system") {
            return Err("Context author must be human, agent or system".into());
        }
        if self.observed.len() > 32 || !self.observed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("Invalid context observation fingerprint".into());
        }
        match (scope, self.region) {
            (Scope::Region(_), Some(region)) => region.validate(),
            (Scope::Region(_), None) => Err("Context region has no bounds".into()),
            (_, Some(_)) => Err("Only region notes can have bounds".into()),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    pub notes: BTreeMap<Scope, Note>,
    pub extra: Vec<Node>,
}

fn field<'a>(nodes: &'a [Node], name: &str) -> Result<&'a str, String> {
    let mut values = nodes.iter().filter(|node| node.key == name);
    let value = values
        .next()
        .and_then(Node::as_str)
        .ok_or_else(|| format!("Context metadata is missing {name}"))?;
    if values.next().is_some() {
        return Err(format!("Context metadata repeats {name}"));
    }
    Ok(value)
}

fn decode(value: &str) -> Result<String, String> {
    if value.len() > MAX_NOTE_BYTES.div_ceil(3) * 4 {
        return Err("Encoded context text exceeds the per-note limit".into());
    }
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| "Invalid base64 context text")?;
    String::from_utf8(bytes).map_err(|_| "Context text is not UTF-8".into())
}

impl Metadata {
    pub fn parse(vmf: &Vmf) -> Result<Self, String> {
        let mut blocks = vmf.extra.iter().filter(|node| node.key == METADATA_KEY);
        let Some(block) = blocks.next() else {
            return Ok(Self::default());
        };
        if blocks.next().is_some() {
            return Err("Map contains more than one HammeRR context block".into());
        }
        let children = block
            .children()
            .ok_or("HammeRR context must be a metadata block")?;
        if field(children, "version")? != VERSION {
            return Err("Unsupported HammeRR context version; metadata has been preserved".into());
        }
        let mut metadata = Self::default();
        let mut total = 0;
        for node in children {
            if node.key == "version" {
                continue;
            }
            if node.key != "note" {
                metadata.extra.push(node.clone());
                continue;
            }
            let fields = node.children().ok_or("Context note must be a block")?;
            let scope = Scope::parse(field(fields, "scope")?)?;
            let region = if matches!(scope, Scope::Region(_)) {
                let mins =
                    parse_vec3(field(fields, "mins")?).ok_or("Invalid context region minimum")?;
                let maxs =
                    parse_vec3(field(fields, "maxs")?).ok_or("Invalid context region maximum")?;
                let color = field(fields, "color")?
                    .split_whitespace()
                    .map(str::parse::<u8>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| "Invalid context region color")?;
                let color: [u8; 3] = color
                    .try_into()
                    .map_err(|_| "Region color must contain three RGB channels")?;
                Some(Region { mins, maxs, color })
            } else {
                if fields
                    .iter()
                    .any(|node| matches!(node.key.as_str(), "mins" | "maxs" | "color"))
                {
                    return Err("Only region context can have bounds and guide colors".into());
                }
                None
            };
            let note = Note {
                summary: decode(field(fields, "summary_base64")?)?,
                body: decode(field(fields, "body_base64")?)?,
                author: field(fields, "author")?.into(),
                observed: field(fields, "observed")?.into(),
                region,
                extra: fields
                    .iter()
                    .filter(|node| {
                        !matches!(
                            node.key.as_str(),
                            "scope"
                                | "summary_base64"
                                | "body_base64"
                                | "author"
                                | "observed"
                                | "mins"
                                | "maxs"
                                | "color"
                        )
                    })
                    .cloned()
                    .collect(),
            };
            note.validate(scope)?;
            total += note.text_bytes();
            if total > MAX_MAP_TEXT_BYTES {
                return Err("Map context exceeds the shared 1 MiB text budget".into());
            }
            if metadata.notes.insert(scope, note).is_some() {
                return Err(format!("More than one note is attached to {scope}"));
            }
        }
        metadata.validate()?;
        Ok(metadata)
    }

    pub fn text_bytes(&self) -> usize {
        self.notes.values().map(Note::text_bytes).sum()
    }

    pub fn validate(&self) -> Result<(), String> {
        for (scope, note) in &self.notes {
            note.validate(*scope)?;
        }
        if self.text_bytes() > MAX_MAP_TEXT_BYTES {
            return Err("Map context exceeds the shared 1 MiB text budget; consolidate existing notes or link to project documents".into());
        }
        if self
            .notes
            .keys()
            .filter(|scope| matches!(scope, Scope::Region(_)))
            .count()
            > MAX_REGIONS
        {
            return Err(format!(
                "A map supports at most {MAX_REGIONS} context regions; consolidate existing regions"
            ));
        }
        Ok(())
    }

    pub fn to_node(&self) -> Node {
        let mut nodes = vec![Node::string("version", VERSION)];
        for (scope, note) in &self.notes {
            let mut fields = vec![
                Node::string("scope", scope.to_string()),
                Node::string("summary_base64", STANDARD.encode(&note.summary)),
                Node::string("body_base64", STANDARD.encode(&note.body)),
                Node::string("author", &note.author),
                Node::string("observed", &note.observed),
            ];
            if let Some(region) = note.region {
                fields.extend([
                    Node::string("mins", format_vec3(region.mins)),
                    Node::string("maxs", format_vec3(region.maxs)),
                    Node::string(
                        "color",
                        format!(
                            "{} {} {}",
                            region.color[0], region.color[1], region.color[2]
                        ),
                    ),
                ]);
            }
            fields.extend(note.extra.clone());
            nodes.push(Node::block("note", fields));
        }
        nodes.extend(self.extra.clone());
        Node::block(METADATA_KEY, nodes)
    }
}

struct Fingerprint(u64);

impl Default for Fingerprint {
    fn default() -> Self {
        Self(0xcbf29ce484222325)
    }
}

impl fmt::Write for Fingerprint {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
        Ok(())
    }
}

pub fn map_fingerprint(vmf: &Vmf) -> String {
    let mut fingerprint = Fingerprint::default();
    let mut properties = vmf.world.properties.clone();
    properties
        .0
        .retain(|(key, _)| !key.eq_ignore_ascii_case("mapversion"));
    let _ = write!(
        fingerprint,
        "{properties:?}{:?}{:?}",
        vmf.world.groups, vmf.world.extra
    );
    for solid in &vmf.world.solids {
        let _ = write!(fingerprint, "{}{:?}", solid.hidden, solid.to_node());
    }
    for entity in &vmf.entities {
        let _ = write!(fingerprint, "{:?}", entity.to_node());
    }
    format!("{:016x}", fingerprint.0)
}

pub fn text_fingerprint(text: &str) -> String {
    let mut fingerprint = Fingerprint::default();
    let _ = fingerprint.write_str(text);
    format!("{:016x}", fingerprint.0)
}

pub fn object_unique(doc: &Document, id: ObjectId) -> bool {
    match id {
        ObjectId::Solid(id) => doc.vmf().solids().filter(|solid| solid.id == id).count() == 1,
        ObjectId::Entity(id) => {
            doc.vmf()
                .entities
                .iter()
                .filter(|entity| entity.id == id)
                .count()
                == 1
        }
    }
}

pub fn object_exists(doc: &Document, id: ObjectId) -> bool {
    match id {
        ObjectId::Solid(id) => doc.vmf().solids().any(|solid| solid.id == id),
        ObjectId::Entity(id) => doc.entity(id).is_some(),
    }
}

pub fn fingerprint(
    doc: &Document,
    scope: Scope,
    region: Option<Region>,
) -> Result<String, EditError> {
    let mut fingerprint = Fingerprint::default();
    match scope {
        Scope::Map => return Ok(doc.context_map_fingerprint().into()),
        Scope::Region(_) => {
            let region =
                region.ok_or_else(|| EditError::Invalid("Context region has no bounds".into()))?;
            let _ = write!(
                fingerprint,
                "{} {:?} {:?}",
                doc.context_map_fingerprint(),
                format_vec3(region.mins),
                format_vec3(region.maxs)
            );
        }
        Scope::Object(ObjectId::Solid(id)) => {
            let solid = doc
                .vmf()
                .solids()
                .find(|solid| solid.id == id)
                .ok_or(EditError::NotFound(ObjectId::Solid(id)))?;
            let _ = write!(fingerprint, "{}{:?}", solid.hidden, solid.to_node());
        }
        Scope::Object(ObjectId::Entity(id)) => {
            let entity = doc
                .entity(id)
                .ok_or(EditError::NotFound(ObjectId::Entity(id)))?;
            let _ = write!(fingerprint, "{:?}", entity.to_node());
        }
    }
    Ok(format!("{:016x}", fingerprint.0))
}

pub fn set_note(
    tx: &mut Transaction,
    scope: Scope,
    summary: String,
    body: String,
    author: &str,
    region: Option<Region>,
) -> Result<(), EditError> {
    if let Scope::Object(id) = scope
        && !object_unique(tx.document(), id)
    {
        return Err(EditError::Invalid(
            "Context attachment must identify one existing object".into(),
        ));
    }
    let mut metadata = tx.document().context()?.clone();
    let extra = metadata
        .notes
        .get(&scope)
        .map(|note| note.extra.clone())
        .unwrap_or_default();
    let note = Note {
        summary,
        body,
        author: author.into(),
        observed: fingerprint(tx.document(), scope, region)?,
        region,
        extra,
    };
    note.validate(scope).map_err(EditError::Invalid)?;
    metadata.notes.insert(scope, note);
    tx.set_context(metadata)
}

pub fn delete_note(tx: &mut Transaction, scope: Scope) -> Result<(), EditError> {
    let mut metadata = tx.document().context()?.clone();
    if metadata.notes.remove(&scope).is_none() {
        return Err(EditError::Invalid("Context note does not exist".into()));
    }
    tx.set_context(metadata)
}

pub fn copy_notes(tx: &mut Transaction, mapping: &[(ObjectId, ObjectId)]) -> Result<(), EditError> {
    let mut metadata = tx.document().context()?.clone();
    let mut changed = false;
    for (old, new) in mapping {
        if let Some(note) = metadata.notes.get(&Scope::Object(*old)).cloned() {
            if metadata.notes.contains_key(&Scope::Object(*new)) {
                return Err(EditError::Invalid("A context note already uses a copied object's ID; inspect orphaned notes first".into()));
            }
            metadata.notes.insert(Scope::Object(*new), note);
            changed = true;
        }
    }
    if changed {
        tx.set_context(metadata)?;
    }
    Ok(())
}

pub fn remove_notes_for(
    tx: &mut Transaction,
    removed: &BTreeSet<ObjectId>,
) -> Result<(), EditError> {
    let Ok(metadata) = tx.document().context() else {
        return Ok(());
    };
    if !metadata
        .notes
        .keys()
        .any(|scope| matches!(scope,Scope::Object(id) if removed.contains(id)))
    {
        return Ok(());
    }
    let mut metadata = metadata.clone();
    metadata
        .notes
        .retain(|scope, _| !matches!(scope,Scope::Object(id) if removed.contains(id)));
    tx.set_context(metadata)
}
