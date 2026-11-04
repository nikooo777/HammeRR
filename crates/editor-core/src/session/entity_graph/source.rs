use map_core::Document;
use sha2::{Digest, Sha256};

use super::{Scope, Session, expanded, id};

pub(super) enum Source<'a> {
    Authored(&'a Document),
    Expanded(Box<expanded::Source>),
}

impl<'a> Source<'a> {
    pub fn prepare(session: &'a Session, scope: Scope) -> Result<Self, String> {
        let source = match scope {
            Scope::Authored => Self::Authored(&session.document),
            Scope::Expanded => Self::Expanded(Box::new(expanded::Source::prepare(session)?)),
        };
        if let Self::Expanded(expanded) = &source {
            for entity in expanded
                .document()
                .vmf()
                .entities
                .iter()
                .filter(|entity| !entity.hidden)
            {
                expanded.node(entity.id)?;
            }
        }
        Ok(source)
    }

    pub fn document(&self) -> &Document {
        match self {
            Self::Authored(document) => document,
            Self::Expanded(source) => source.document(),
        }
    }

    pub fn seeds(&self, roots: &[u32]) -> Result<Vec<u32>, String> {
        match self {
            Self::Authored(_) => Ok(roots.to_vec()),
            Self::Expanded(source) => source.seeds(roots),
        }
    }

    pub fn id(&self, internal: u32) -> String {
        match self {
            Self::Authored(_) => id(internal),
            Self::Expanded(source) => source
                .node(internal)
                .expect("admitted graph provenance")
                .id
                .clone(),
        }
    }

    pub fn owner(&self, internal: u32) -> u32 {
        match self {
            Self::Authored(_) => internal,
            Self::Expanded(source) => {
                source
                    .node(internal)
                    .expect("admitted graph provenance")
                    .owner
            }
        }
    }

    pub fn provenance_bound(&self, internal: u32) -> usize {
        match self {
            Self::Authored(_) => 0,
            Self::Expanded(source) => {
                let node = source.node(internal).expect("admitted graph provenance");
                node.file
                    .as_ref()
                    .map_or(0, |file| file.len().saturating_mul(8))
                    .saturating_add(
                        node.occurrence
                            .as_ref()
                            .map_or(0, |path| path.response_copy_bytes()),
                    )
                    .saturating_add(512)
            }
        }
    }

    pub fn provenance(&self, internal: u32) -> Option<serde_json::Value> {
        match self {
            Self::Authored(_) => None,
            Self::Expanded(source) => source
                .node(internal)
                .expect("admitted graph provenance")
                .provenance(),
        }
    }

    pub fn resolve(&self, identity: &str) -> Option<u32> {
        match self {
            Self::Authored(document) => {
                let map_core::ObjectId::Entity(id) = crate::identity::parse_id(identity).ok()?
                else {
                    return None;
                };
                document
                    .entity(id)
                    .filter(|entity| !entity.hidden)
                    .map(|entity| entity.id)
            }
            Self::Expanded(source) => source.resolve(identity),
        }
    }

    pub fn hash(&self, hash: &mut Sha256) {
        if let Self::Expanded(source) = self {
            hash.update(source.snapshot_digest().as_bytes());
        }
    }

    pub fn retained_bytes(&self) -> usize {
        match self {
            Self::Authored(_) => 0,
            Self::Expanded(source) => source.retained_bytes(),
        }
    }

    pub fn verify(&self) -> Result<(), String> {
        match self {
            Self::Authored(_) => Ok(()),
            Self::Expanded(source) => source.verify(),
        }
    }
}
