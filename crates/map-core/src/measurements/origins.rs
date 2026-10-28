use std::collections::HashSet;

use glam::{DMat3, DQuat, DVec3, EulerRot};
use source_formats::keyvalues::Node;
use source_formats::vmf::{Vmf, parse_vec3};

use super::{KEY, Measurement, validate_point};
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

    fn local_key(self) -> &'static str {
        match self {
            Self::Start => "start_entity_local_offset",
            Self::End => "end_entity_local_offset",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::End => "end",
        }
    }
}

pub(crate) fn reserved_origin_ids(vmf: &Vmf) -> HashSet<u32> {
    vmf.extra
        .iter()
        .filter(|node| node.key == KEY)
        .filter_map(Node::children)
        .flatten()
        .filter(|node| node.key == "measurement")
        .filter_map(Node::children)
        .flatten()
        .filter(|node| [Endpoint::Start.key(), Endpoint::End.key()].contains(&node.key.as_str()))
        .filter_map(Node::as_str)
        .filter_map(|value| value.parse::<u32>().ok())
        .filter(|id| *id != 0)
        .collect()
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

pub fn entity_local_pose(vmf: &Vmf, object: ObjectId) -> Result<(DVec3, DMat3), String> {
    let origin = entity_origin(vmf, object)?;
    let ObjectId::Entity(id) = object else {
        unreachable!()
    };
    let entity = vmf.entities.iter().find(|entity| entity.id == id).unwrap();
    let orientation_key = if entity.properties.get("angles").is_some() {
        "angles"
    } else {
        "angle"
    };
    let mut keys = vec![orientation_key];
    if matches!(entity.classname(), Some("light_spot" | "light_environment")) {
        keys.push("pitch");
    }
    for key in keys {
        if entity.properties.get_all(key).count() > 1 {
            return Err(format!(
                "Attached entity:{id} has ambiguous duplicate {key} properties"
            ));
        }
    }
    if let Some(value) = entity.properties.get(orientation_key) {
        let valid = if orientation_key == "angles" {
            parse_vec3(value).is_some_and(|angles| angles.is_finite())
        } else {
            value.trim().parse::<f64>().is_ok_and(f64::is_finite)
        };
        if !valid {
            return Err(format!(
                "Attached entity:{id} needs valid finite authored {orientation_key}"
            ));
        }
    }
    let angles = crate::query::point_entity_angles(entity)
        .ok_or_else(|| format!("Attached entity:{id} needs a valid finite authored orientation"))?;
    let radians = angles.map(|angle| angle.rem_euclid(360.0).to_radians());
    let basis = DMat3::from_quat(DQuat::from_euler(
        EulerRot::ZYX,
        radians.y,
        radians.x,
        radians.z,
    ));
    if !basis.is_finite() {
        return Err(format!("Attached entity:{id} has an invalid orientation"));
    }
    Ok((origin, basis))
}

impl Measurement {
    pub fn local_offset(&self, endpoint: Endpoint) -> Result<Option<DVec3>, String> {
        let mut nodes = self
            .extra
            .iter()
            .filter(|node| node.key == endpoint.local_key());
        let Some(node) = nodes.next() else {
            return Ok(None);
        };
        if nodes.next().is_some() {
            return Err(format!(
                "Duplicate measurement {} local offset",
                endpoint.label()
            ));
        }
        let offset = node
            .as_str()
            .and_then(parse_vec3)
            .filter(|offset| offset.is_finite())
            .ok_or_else(|| format!("Invalid measurement {} local offset", endpoint.label()))?;
        if self.origin_reference(endpoint)?.is_none() {
            return Err(format!(
                "Measurement {} local offset needs an entity reference",
                endpoint.label()
            ));
        }
        Ok(Some(offset))
    }

    pub fn set_local_reference(
        &mut self,
        endpoint: Endpoint,
        entity: u32,
        offset: DVec3,
    ) -> Result<(), String> {
        if entity == 0 || !offset.is_finite() {
            return Err("Local attachments need a positive entity ID and finite offset".into());
        }
        self.set_origin_reference(endpoint, Some(entity));
        self.extra.push(Node::string(
            endpoint.local_key(),
            super::exact_point(offset),
        ));
        Ok(())
    }

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
        self.extra.retain(|node| node.key != endpoint.local_key());
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
        if let Some(offset) = self.local_offset(endpoint)? {
            let id = self.origin_reference(endpoint)?.unwrap();
            let (origin, basis) = entity_local_pose(vmf, ObjectId::Entity(id))?;
            let point = origin + basis * offset;
            validate_point(point)?;
            return Ok(point);
        }
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
            return Err("Measurement endpoints must be distinct; attached points coincide".into());
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
            return Err("Measurement endpoints must be distinct; attached points coincide".into());
        }
        Ok(())
    }
}
