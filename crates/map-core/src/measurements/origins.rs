use glam::DVec3;
use source_formats::keyvalues::Node;
use source_formats::vmf::{Vmf, parse_vec3};

use super::{Measurement, validate_point};
use crate::ObjectId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Start,
    End,
}

impl Endpoint {
    fn key(self) -> &'static str {
        match self {
            Self::Start => "start_entity_origin",
            Self::End => "end_entity_origin",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::End => "end",
        }
    }
}

pub fn entity_origin(vmf: &Vmf, object: ObjectId) -> Result<DVec3, String> {
    let ObjectId::Entity(id) = object else {
        return Err(
            "Origin attachment supports point entities only; brushes are unsupported".into(),
        );
    };
    let mut entities = vmf.entities.iter().filter(|entity| entity.id == id);
    let entity = entities
        .next()
        .ok_or_else(|| format!("Attached entity:{id} is missing"))?;
    if entities.next().is_some() {
        return Err(format!(
            "Attached entity:{id} is ambiguous because its ID is duplicated"
        ));
    }
    if !entity.solids.is_empty() {
        return Err(format!(
            "Attached entity:{id} is a brush entity; origin attachment supports point entities only"
        ));
    }
    let mut origins = entity.properties.get_all("origin");
    let origin = origins.next();
    if origins.next().is_some() {
        return Err(format!(
            "Attached entity:{id} has ambiguous duplicate origin properties"
        ));
    }
    let origin = origin
        .and_then(parse_vec3)
        .filter(|point| point.is_finite())
        .ok_or_else(|| format!("Attached entity:{id} needs a valid authored finite origin"))?;
    validate_point(origin)?;
    Ok(origin)
}

impl Measurement {
    pub fn origin_reference(&self, endpoint: Endpoint) -> Result<Option<u32>, String> {
        let mut nodes = self.extra.iter().filter(|node| node.key == endpoint.key());
        let Some(node) = nodes.next() else {
            return Ok(None);
        };
        if nodes.next().is_some() {
            return Err(format!(
                "Duplicate measurement {} origin reference",
                endpoint.label()
            ));
        }
        let id = node
            .as_str()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|id| *id != 0)
            .ok_or_else(|| format!("Invalid measurement {} origin reference", endpoint.label()))?;
        Ok(Some(id))
    }

    pub fn set_origin_reference(&mut self, endpoint: Endpoint, entity: Option<u32>) {
        if let Some(id) = entity {
            let reference = Node::string(endpoint.key(), id.to_string());
            if let Some(node) = self
                .extra
                .iter_mut()
                .find(|node| node.key == endpoint.key())
            {
                *node = reference;
            } else {
                self.extra.push(reference);
            }
        } else {
            self.extra.retain(|node| node.key != endpoint.key());
        }
    }

    pub fn stored_endpoint(&self, endpoint: Endpoint) -> DVec3 {
        match endpoint {
            Endpoint::Start => self.start,
            Endpoint::End => self.end,
        }
    }

    pub fn set_stored_endpoint(&mut self, endpoint: Endpoint, point: DVec3) {
        match endpoint {
            Endpoint::Start => self.start = point,
            Endpoint::End => self.end = point,
        }
    }

    pub fn resolve_endpoint(&self, vmf: &Vmf, endpoint: Endpoint) -> Result<DVec3, String> {
        match self.origin_reference(endpoint)? {
            Some(id) => entity_origin(vmf, ObjectId::Entity(id)),
            None => Ok(self.stored_endpoint(endpoint)),
        }
    }

    pub fn resolved(&self, vmf: &Vmf) -> Result<Self, String> {
        let mut result = self.clone();
        result.start = self.resolve_endpoint(vmf, Endpoint::Start)?;
        result.end = self.resolve_endpoint(vmf, Endpoint::End)?;
        if result.start == result.end {
            return Err("Measurement endpoints must be distinct; attached origins coincide".into());
        }
        Ok(result)
    }

    pub fn validate_available_endpoints(&self, vmf: &Vmf) -> Result<(), String> {
        self.validate()?;
        if let (Ok(start), Ok(end)) = (
            self.resolve_endpoint(vmf, Endpoint::Start),
            self.resolve_endpoint(vmf, Endpoint::End),
        ) && start == end
        {
            return Err("Measurement endpoints must be distinct; attached origins coincide".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
