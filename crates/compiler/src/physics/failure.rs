use glam::DVec3;
use serde::{Deserialize, Serialize};
use source_formats::vmf::{Entity, Vmf};

use super::{Request, emit};
use crate::Diagnostic;
use crate::map::Map;

pub(super) const MAX_FAILURE_BYTES: u64 = 64 * 1024;
const PROTOCOL_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Stage {
    Request,
    Library,
    ConvexConstruction,
    CompoundConstruction,
    GeometryVerification,
    Serialization,
    RoundTripVerification,
    Keys,
    Output,
}

impl Stage {
    fn label(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Library => "library loading",
            Self::ConvexConstruction => "convex construction",
            Self::CompoundConstruction => "compound construction",
            Self::GeometryVerification => "geometry verification",
            Self::Serialization => "serialization",
            Self::RoundTripVerification => "round-trip verification",
            Self::Keys => "physics keys",
            Self::Output => "output",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Failure {
    version: u8,
    pub stage: Stage,
    pub message: String,
    pub model: Option<usize>,
    pub body: Option<usize>,
    pub tag: Option<u32>,
}

impl Failure {
    pub fn new(stage: Stage, message: impl Into<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            stage,
            message: message.into(),
            model: None,
            body: None,
            tag: None,
        }
    }

    pub fn shape(stage: Stage, tag: u32, message: impl Into<String>) -> Self {
        Self {
            tag: Some(tag),
            ..Self::new(stage, message)
        }
    }

    pub fn context(mut self, model: usize, body: usize) -> Self {
        self.model = Some(model);
        self.body = Some(body);
        self
    }

    pub fn stage(mut self, stage: Stage) -> Self {
        self.stage = stage;
        self
    }

    pub fn diagnostic(
        &self,
        vmf: &Vmf,
        map: &Map,
        models: &[emit::Model],
        request: &Request,
    ) -> Result<Diagnostic, String> {
        if self.version != PROTOCOL_VERSION
            || self.message.is_empty()
            || (self.tag.is_some()
                && !matches!(
                    self.stage,
                    Stage::ConvexConstruction
                        | Stage::GeometryVerification
                        | Stage::RoundTripVerification
                ))
            || (self.model.is_some() && matches!(self.stage, Stage::Request | Stage::Library))
        {
            return Err("Physics worker returned an invalid failure protocol".into());
        }
        let context = match (self.model, self.body) {
            (Some(model), Some(body)) => {
                let model = request
                    .models
                    .iter()
                    .find(|candidate| candidate.index == model)
                    .ok_or("Physics worker failure references an unrequested model")?;
                let body = model
                    .bodies
                    .get(body)
                    .ok_or("Physics worker failure references an unrequested body")?;
                Some((model, body))
            }
            (None, None) if self.tag.is_none() => None,
            _ => return Err("Physics worker failure has incomplete source context".into()),
        };
        let mut message = format!("Engine physics {}: {}", self.stage.label(), self.message);
        let mut object = None;
        let mut position = None;
        if let Some(tag) = self.tag {
            let (_, body) = context.ok_or("Physics worker failure tag needs body context")?;
            let shape = body
                .shapes
                .iter()
                .find(|shape| shape.brush == tag)
                .ok_or("Physics worker failure references an unrequested hull tag")?;
            if body.surface.is_some() {
                let mut mins = DVec3::splat(f64::INFINITY);
                let mut maxs = DVec3::splat(f64::NEG_INFINITY);
                for point in &shape.corners {
                    let point = DVec3::from_array(point.map(f64::from));
                    mins = mins.min(point);
                    maxs = maxs.max(point);
                }
                message.push_str(&format!(
                    "; generated fluid hull tag {tag}, world wet leaf {}, checked bounds {:?} to {:?}",
                    (tag as usize)
                        .checked_sub(map.brushes.len())
                        .ok_or("Generated fluid failure has an invalid wet-leaf tag")?,
                    mins.to_array(),
                    maxs.to_array(),
                ));
                position = Some((mins + maxs) * 0.5);
            } else {
                let brush = map
                    .brushes
                    .get(tag as usize)
                    .ok_or("Physics worker failure references an unknown authored brush")?;
                let owner = if vmf.world.solids.iter().any(|solid| solid.id == brush.id) {
                    format!("VMF entity {} (\"worldspawn\")", vmf.world.id)
                } else {
                    let entity = vmf
                        .entities
                        .iter()
                        .find(|entity| entity.solids.iter().any(|solid| solid.id == brush.id))
                        .ok_or("Physics worker failure brush has no authored entity")?;
                    entity_description(entity)
                };
                let origin = map.entities[brush.entity].origin.unwrap_or(DVec3::ZERO);
                let mins = brush.mins + origin;
                let maxs = brush.maxs + origin;
                message.push_str(&format!(
                    "; VMF solid {}, {}, checked world bounds {:?} to {:?}",
                    brush.id,
                    owner,
                    mins.to_array(),
                    maxs.to_array(),
                ));
                object = Some(format!("solid:{}", brush.id));
                position = Some((mins + maxs) * 0.5);
            }
        } else if let Some((model, _)) = context {
            let entity_index = models
                .get(model.index)
                .ok_or("Physics worker failure model has no source model")?
                .entity;
            let entity = &map.entities[entity_index];
            message.push_str(&format!(
                "; model {}, body {}, VMF entity {} ({:?})",
                model.index,
                self.body.unwrap(),
                entity.id,
                entity.classname,
            ));
            object = Some(format!("entity:{}", entity.id));
            position = entity.origin;
        }
        Ok(Diagnostic::error(message, object, position))
    }
}

fn entity_description(entity: &Entity) -> String {
    let classname = entity.classname().unwrap_or("missing classname");
    let targetname = entity
        .properties
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("targetname"))
        .map(|(_, value)| value);
    let mut result = format!("VMF entity {} ({classname:?})", entity.id);
    if let Some(targetname) = targetname {
        result.push_str(&format!(", targetname {targetname:?}"));
    }
    result
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::new(Stage::Request, message)
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        Self::from(message.to_owned())
    }
}

#[cfg(test)]
mod tests;
