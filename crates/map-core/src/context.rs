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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit;
    use source_formats::keyvalues::WriteOptions;

    fn fixture() -> (Document, ObjectId) {
        let mut doc = Document::empty();
        let id = doc
            .edit("box", |tx| {
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "brick/test")
            })
            .unwrap();
        (doc, id)
    }

    fn note(doc: &mut Document, scope: Scope, summary: &str, body: &str) {
        doc.edit("note", |tx| {
            set_note(tx, scope, summary.into(), body.into(), "agent", None)
        })
        .unwrap();
    }

    #[test]
    fn prose_roundtrips_quotes_unicode_newlines_and_unknown_metadata() {
        let (mut doc, id) = fixture();
        let mut vmf = doc.vmf().clone();
        vmf.extra.push(Node::block(
            "another_editor",
            vec![Node::string("preserve", "value")],
        ));
        doc = Document::new(vmf);
        let body = "Keep the \"warm\" light.\n日本語 / é / 🛠\nLiteral \\n remains literal.";
        note(&mut doc, Scope::Object(id), "Lighting intent", body);
        let text = doc.vmf().write(&WriteOptions::default());
        let restored = Document::new(Vmf::parse(&text).unwrap());
        assert_eq!(restored.context().unwrap(), doc.context().unwrap());
        assert_eq!(
            restored.context().unwrap().notes[&Scope::Object(id)].body,
            body
        );
        assert_eq!(restored.vmf().extra[0].key, "another_editor");
        assert!(!text.contains("Keep the"));
        assert_eq!(
            fingerprint(&restored, Scope::Object(id), None).unwrap(),
            restored.context().unwrap().notes[&Scope::Object(id)].observed
        );
    }

    #[test]
    fn metadata_edits_share_undo_redo_and_failure_rolls_back() {
        let (mut doc, id) = fixture();
        let before = doc.vmf().clone();
        note(
            &mut doc,
            Scope::Object(id),
            "Sightline blocker",
            "Keep this wall tall.",
        );
        let with_note = doc.vmf().clone();
        doc.undo().unwrap();
        assert_eq!(doc.vmf(), &before);
        assert!(doc.context().unwrap().notes.is_empty());
        doc.redo().unwrap();
        assert_eq!(doc.vmf(), &with_note);
        let error = doc.edit("abort", |tx| {
            set_note(
                tx,
                Scope::Map,
                "Overview".into(),
                "body".into(),
                "human",
                None,
            )?;
            edit::translate(tx, &[id], DVec3::X, true)?;
            Err::<(), _>(EditError::Invalid("abort".into()))
        });
        assert!(error.is_err());
        assert_eq!(doc.vmf(), &with_note);
        assert_eq!(doc.context().unwrap().notes.len(), 1);
    }

    #[test]
    fn object_changes_flag_observations_until_review_or_undo() {
        let (mut doc, id) = fixture();
        note(&mut doc, Scope::Object(id), "Blocker", "Tested sightline.");
        note(&mut doc, Scope::Map, "Map brief", "Small map.");
        let observed = doc.context().unwrap().notes[&Scope::Object(id)]
            .observed
            .clone();
        let map_observed = doc.context().unwrap().notes[&Scope::Map].observed.clone();
        doc.bump_map_version();
        assert_eq!(fingerprint(&doc, Scope::Map, None).unwrap(), map_observed);
        doc.edit("move", |tx| {
            edit::translate(tx, &[id], DVec3::X * 32.0, true)
        })
        .unwrap();
        assert_ne!(
            fingerprint(&doc, Scope::Object(id), None).unwrap(),
            observed
        );
        assert_ne!(fingerprint(&doc, Scope::Map, None).unwrap(), map_observed);
        doc.undo().unwrap();
        assert_eq!(
            fingerprint(&doc, Scope::Object(id), None).unwrap(),
            observed
        );
        assert_eq!(fingerprint(&doc, Scope::Map, None).unwrap(), map_observed);
    }

    #[test]
    fn deletion_removes_attached_notes_and_undo_restores_them() {
        let (mut doc, id) = fixture();
        note(
            &mut doc,
            Scope::Object(id),
            "Object note",
            "Must survive undo.",
        );
        let before = doc.vmf().clone();
        doc.edit("delete", |tx| edit::delete(tx, &[id])).unwrap();
        assert!(doc.context().unwrap().notes.is_empty());
        doc.undo().unwrap();
        assert_eq!(doc.vmf(), &before);
        doc.redo().unwrap();
        assert!(doc.context().unwrap().notes.is_empty());
    }

    #[test]
    fn copies_keep_authorship_and_require_review_with_shared_budget() {
        let (mut doc, id) = fixture();
        note(&mut doc, Scope::Object(id), "Object note", "Tested here.");
        let observed = doc.context().unwrap().notes[&Scope::Object(id)]
            .observed
            .clone();
        let copies = doc
            .edit("copy", |tx| edit::duplicate(tx, &[id], DVec3::X * 128.0))
            .unwrap();
        let copied = &doc.context().unwrap().notes[&Scope::Object(copies[0])];
        assert_eq!(copied.author, "agent");
        assert_eq!(copied.observed, observed);
        assert_ne!(
            fingerprint(&doc, Scope::Object(copies[0]), None).unwrap(),
            observed
        );
        doc.undo().unwrap();
        assert_eq!(doc.context().unwrap().notes.len(), 1);
        doc.redo().unwrap();
        assert_eq!(doc.context().unwrap().notes.len(), 2);
    }

    #[test]
    fn note_and_map_limits_apply_to_unicode_imports_and_copy_transactions() {
        let (mut doc, id) = fixture();
        let too_long = "🛠".repeat(MAX_NOTE_BYTES / 4);
        assert!(
            doc.edit("overflow", |tx| set_note(
                tx,
                Scope::Object(id),
                "Title".into(),
                too_long,
                "agent",
                None
            ))
            .is_err()
        );
        assert!(doc.context().unwrap().notes.is_empty());
        let body = "x".repeat(MAX_NOTE_BYTES - 1);
        let full = Note {
            summary: "x".into(),
            body,
            author: "agent".into(),
            observed: String::new(),
            region: None,
            extra: vec![],
        };
        let mut metadata = Metadata::default();
        metadata.notes.insert(Scope::Object(id), full.clone());
        for number in 1..MAX_MAP_TEXT_BYTES / MAX_NOTE_BYTES {
            metadata.notes.insert(
                Scope::Object(ObjectId::Entity(number as u32 + 1000)),
                full.clone(),
            );
        }
        doc.edit("full context", |tx| tx.set_context(metadata))
            .unwrap();
        assert_eq!(doc.context().unwrap().text_bytes(), MAX_MAP_TEXT_BYTES);
        let before = doc.vmf().clone();
        let error = doc
            .edit("copy exceeds map budget", |tx| {
                edit::duplicate(tx, &[id], DVec3::X)
            })
            .unwrap_err();
        assert!(error.to_string().contains("1 MiB"));
        assert_eq!(doc.vmf(), &before);
        let mut imported = doc.vmf().clone();
        let block = imported
            .extra
            .iter_mut()
            .find(|node| node.key == METADATA_KEY)
            .unwrap();
        let source_formats::keyvalues::Value::Block(children) = &mut block.value else {
            panic!()
        };
        let mut extra = Metadata::default();
        extra.notes.insert(Scope::Map, full);
        children.push(extra.to_node().children().unwrap()[1].clone());
        assert!(Metadata::parse(&imported).unwrap_err().contains("1 MiB"));
    }

    #[test]
    fn brush_notes_follow_conversion_and_entity_deletion_cleans_owned_notes() {
        let (mut doc, id) = fixture();
        note(
            &mut doc,
            Scope::Object(id),
            "Brush note",
            "Shared solid identity.",
        );
        let entity = doc
            .edit("tie", |tx| {
                edit::tie_to_entity(
                    tx,
                    &[match id {
                        ObjectId::Solid(id) => id,
                        _ => unreachable!(),
                    }],
                    "func_detail",
                    &[],
                )
            })
            .unwrap();
        assert!(
            doc.context()
                .unwrap()
                .notes
                .contains_key(&Scope::Object(id))
        );
        note(&mut doc, Scope::Object(entity), "Entity note", "Assembly.");
        let copies = doc
            .edit("copy entity", |tx| {
                edit::duplicate(tx, &[entity], DVec3::X * 128.0)
            })
            .unwrap();
        let ObjectId::Entity(copied) = copies[0] else {
            panic!()
        };
        let solid = doc.entity(copied).unwrap().solids[0].id;
        assert!(
            doc.context()
                .unwrap()
                .notes
                .contains_key(&Scope::Object(ObjectId::Solid(solid)))
        );
        assert!(
            doc.context()
                .unwrap()
                .notes
                .contains_key(&Scope::Object(copies[0]))
        );
        doc.edit("delete entity", |tx| edit::delete(tx, &[entity]))
            .unwrap();
        assert!(
            !doc.context()
                .unwrap()
                .notes
                .contains_key(&Scope::Object(id))
        );
        assert!(
            !doc.context()
                .unwrap()
                .notes
                .contains_key(&Scope::Object(entity))
        );
        doc.undo().unwrap();
        assert!(
            doc.context()
                .unwrap()
                .notes
                .contains_key(&Scope::Object(id))
        );
    }
    #[test]
    fn clipping_keeps_only_the_original_attachment_and_undo_restores_it() {
        let (mut doc, id) = fixture();
        note(
            &mut doc,
            Scope::Object(id),
            "Divider",
            "Keep cover on this route.",
        );
        let original = doc.vmf().clone();
        let observed = doc.context().unwrap().notes[&Scope::Object(id)]
            .observed
            .clone();
        let result = doc
            .edit("split", |tx| {
                crate::clip::clip(
                    tx,
                    &[id],
                    crate::Plane {
                        normal: DVec3::X,
                        dist: 32.0,
                    },
                    crate::clip::Keep::Both,
                    None,
                )
            })
            .unwrap();
        assert_eq!(result.cuts[0].solids.len(), 2);
        assert_eq!(doc.context().unwrap().notes.len(), 1);
        assert!(
            doc.context()
                .unwrap()
                .notes
                .contains_key(&Scope::Object(id))
        );
        assert_ne!(
            fingerprint(&doc, Scope::Object(id), None).unwrap(),
            observed
        );
        doc.undo().unwrap();
        assert_eq!(doc.vmf(), &original);
        assert_eq!(
            fingerprint(&doc, Scope::Object(id), None).unwrap(),
            observed
        );
        let entity = doc
            .edit("tie", |tx| {
                edit::tie_to_entity(
                    tx,
                    &[match id {
                        ObjectId::Solid(id) => id,
                        _ => unreachable!(),
                    }],
                    "func_detail",
                    &[],
                )
            })
            .unwrap();
        note(
            &mut doc,
            Scope::Object(entity),
            "Assembly",
            "Combined object.",
        );
        doc.edit("move to world", |tx| {
            edit::move_to_world(
                tx,
                match entity {
                    ObjectId::Entity(id) => id,
                    _ => unreachable!(),
                },
            )
        })
        .unwrap();
        assert!(
            doc.context()
                .unwrap()
                .notes
                .contains_key(&Scope::Object(id))
        );
        assert!(
            !doc.context()
                .unwrap()
                .notes
                .contains_key(&Scope::Object(entity))
        );
    }

    #[test]
    fn region_budget_and_imported_reserved_fields_are_checked() {
        let (mut doc, _) = fixture();
        let region = Region {
            mins: DVec3::ZERO,
            maxs: DVec3::splat(64.0),
            color: [1, 2, 3],
        };
        doc.edit("regions", |tx| {
            let mut metadata = Metadata::default();
            for id in 1..=MAX_REGIONS as u32 {
                metadata.notes.insert(
                    Scope::Region(id),
                    Note {
                        summary: "Region".into(),
                        body: String::new(),
                        author: "agent".into(),
                        observed: String::new(),
                        region: Some(region),
                        extra: vec![],
                    },
                );
            }
            tx.set_context(metadata)
        })
        .unwrap();
        let before = doc.vmf().clone();
        assert!(
            doc.edit("overflow", |tx| set_note(
                tx,
                Scope::Region(MAX_REGIONS as u32 + 1),
                "Overflow".into(),
                String::new(),
                "human",
                Some(region)
            ))
            .is_err()
        );
        assert_eq!(doc.vmf(), &before);
        let (mut doc, _) = fixture();
        note(&mut doc, Scope::Map, "Map", "Valid body");
        let mut vmf = doc.vmf().clone();
        let block = vmf
            .extra
            .iter_mut()
            .find(|node| node.key == METADATA_KEY)
            .unwrap();
        if let source_formats::keyvalues::Value::Block(children) = &mut block.value
            && let source_formats::keyvalues::Value::Block(fields) = &mut children[1].value
        {
            fields.push(Node::string("mins", "0 0 0"));
        }
        assert!(Metadata::parse(&vmf).unwrap_err().contains("Only region"));
    }
    #[test]
    fn orphan_attachments_reserve_only_their_ids_without_exhausting_allocation() {
        let mut doc = Document::empty();
        doc.edit("import notes", |tx| {
            let mut metadata = Metadata::default();
            for id in [2, u32::MAX] {
                metadata.notes.insert(
                    Scope::Object(ObjectId::Solid(id)),
                    Note {
                        summary: "Imported note".into(),
                        body: String::new(),
                        author: "agent".into(),
                        observed: String::new(),
                        region: None,
                        extra: vec![],
                    },
                );
            }
            tx.set_context(metadata)
        })
        .unwrap();
        let mut doc = Document::new(doc.vmf().clone());
        let created = doc
            .edit("box", |tx| {
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "brick/test")
            })
            .unwrap();
        assert_eq!(created, ObjectId::Solid(3));
        assert!(
            !doc.context()
                .unwrap()
                .notes
                .contains_key(&Scope::Object(created))
        );
        assert_eq!(doc.context().unwrap().notes.len(), 2);
    }
    #[test]
    fn entity_and_map_observations_survive_save_layout_normalization() {
        let (mut doc, solid) = fixture();
        doc.edit("signed zero", |tx| {
            let ObjectId::Solid(id) = solid else {
                unreachable!()
            };
            tx.solid_mut(id)?.sides[0].u_axis.axis.z = -0.0;
            Ok(())
        })
        .unwrap();
        let entity = doc
            .edit("light", |tx| {
                edit::create_entity(
                    tx,
                    "light",
                    Some(DVec3::new(0.0, 0.0, 96.0)),
                    &[("_light".into(), "255 220 180 400".into())],
                    vec![],
                )
            })
            .unwrap();
        note(
            &mut doc,
            Scope::Object(entity),
            "Lighting",
            "Warm entrance.",
        );
        note(
            &mut doc,
            Scope::Object(solid),
            "Blocker",
            "Keep this corner.",
        );
        note(&mut doc, Scope::Map, "Overview", "Small courtyard.");
        let saved = doc.snapshot_for_save().write(&WriteOptions::default());
        let restored = Document::new(Vmf::parse(&saved).unwrap());
        for scope in [Scope::Map, Scope::Object(entity), Scope::Object(solid)] {
            assert_eq!(
                fingerprint(&restored, scope, None).unwrap(),
                restored.context().unwrap().notes[&scope].observed
            );
        }
    }
}
