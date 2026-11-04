use glam::DVec3;

use crate::keyvalues::Node;

use super::{Error, Side, Solid, format_vec3, invalid};

pub const MAX_PRECISE_SIDES: usize = 64;
pub const MAX_PRECISE_VERTICES: usize = 64;
pub const MAX_PRECISE_ENTRIES: usize = MAX_PRECISE_SIDES * MAX_PRECISE_VERTICES;
pub const MAX_PRECISE_COORDINATE: f64 = 65_536.0;
pub const MAX_PRECISE_FIELDS: usize = 128;
const MAX_FIELD_TEXT_BYTES: usize = 1018;
const KEY: &str = "vertices_plus";

#[derive(Debug, Clone, PartialEq)]
pub struct PreciseVertices {
    pub vertices: Vec<DVec3>,
    pub has_opaque_fields: bool,
}

pub fn is_precise_vertices_key(key: &str) -> bool {
    key.eq_ignore_ascii_case(KEY)
}

fn validate_fields(fields: &[Node], path: &str) -> Result<(), Error> {
    if fields.len() > MAX_PRECISE_FIELDS {
        return Err(invalid(
            path,
            "precise-vertex fields exceed their input budget",
        ));
    }
    for field in fields {
        let value = field
            .as_str()
            .ok_or_else(|| invalid(path, "nested precise-vertex blocks are unsupported"))?;
        let field_bytes = field
            .key
            .len()
            .checked_add(value.len())
            .ok_or_else(|| invalid(path, "precise-vertex fields exceed their text budget"))?;
        if field_bytes > MAX_FIELD_TEXT_BYTES {
            return Err(invalid(
                path,
                "precise-vertex fields exceed the native line limit",
            ));
        }
    }
    Ok(())
}

fn validate(vertices: &[DVec3], path: &str) -> Result<(), Error> {
    if !(3..=MAX_PRECISE_VERTICES).contains(&vertices.len()) {
        return Err(invalid(path, "precise rings require 3–64 vertices"));
    }
    for (index, point) in vertices.iter().enumerate() {
        if !point.is_finite() || point.abs().max_element() > MAX_PRECISE_COORDINATE {
            return Err(invalid(
                path,
                "precise vertices must be finite and within ±65536 units",
            ));
        }
        if vertices[..index].contains(point) {
            return Err(invalid(path, "precise ring contains a duplicate vertex"));
        }
    }
    Ok(())
}

impl Side {
    pub fn precise_vertices(&self) -> Result<Option<PreciseVertices>, Error> {
        let path = format!("side {}/vertices_plus", self.id);
        if self
            .extra_properties
            .iter()
            .any(|(key, _)| is_precise_vertices_key(key))
        {
            return Err(invalid(&path, "precise vertices must be a block"));
        }
        let mut blocks = self
            .extra
            .iter()
            .filter(|node| is_precise_vertices_key(&node.key));
        let Some(block) = blocks.next() else {
            return Ok(None);
        };
        if blocks.next().is_some() {
            return Err(invalid(&path, "duplicate precise-vertex blocks"));
        }
        let fields = block
            .children()
            .ok_or_else(|| invalid(&path, "precise vertices must be a block"))?;
        validate_fields(fields, &path)?;
        let mut vertices = Vec::new();
        let mut has_opaque_fields = false;
        for field in fields {
            let value = field
                .as_str()
                .ok_or_else(|| invalid(&path, "nested precise-vertex blocks are unsupported"))?;
            if field.key != "v" {
                if field.key.starts_with('v') {
                    return Err(invalid(
                        &path,
                        "precise vertex entries require the exact key v",
                    ));
                }
                has_opaque_fields = true;
                continue;
            }
            if vertices.len() >= MAX_PRECISE_VERTICES {
                return Err(invalid(
                    &path,
                    "precise vertex entries exceed their input budget",
                ));
            }
            let mut parts = value.split_ascii_whitespace();
            let mut point = DVec3::ZERO;
            for axis in 0..3 {
                point[axis] = parts
                    .next()
                    .and_then(|part| part.parse::<f64>().ok())
                    .ok_or_else(|| invalid(&path, "precise vertex requires three numbers"))?;
            }
            if parts.next().is_some() {
                return Err(invalid(
                    &path,
                    "precise vertex requires exactly three numbers",
                ));
            }
            vertices.push(point);
        }
        validate(&vertices, &path)?;
        Ok(Some(PreciseVertices {
            vertices,
            has_opaque_fields,
        }))
    }

    pub fn set_precise_vertices(&mut self, vertices: &[DVec3]) -> Result<(), Error> {
        let path = format!("side {}/vertices_plus", self.id);
        validate(vertices, &path)?;
        self.precise_vertices()?;
        let mut fields = vertices
            .iter()
            .map(|&point| Node::string("v", format_vec3(point)))
            .collect::<Vec<_>>();
        if let Some(block) = self
            .extra
            .iter()
            .find(|node| is_precise_vertices_key(&node.key))
        {
            let retained = block
                .children()
                .ok_or_else(|| invalid(&path, "precise vertices must be a block"))?;
            fields.extend(retained.iter().filter(|field| field.key != "v").cloned());
        }
        validate_fields(&fields, &path)?;
        let block = Node::block(KEY, fields);
        if let Some(existing) = self
            .extra
            .iter_mut()
            .find(|node| is_precise_vertices_key(&node.key))
        {
            *existing = block;
        } else {
            self.extra.push(block);
        }
        Ok(())
    }

    pub fn clear_precise_vertices(&mut self) -> Result<(), Error> {
        if self
            .precise_vertices()?
            .is_some_and(|data| data.has_opaque_fields)
        {
            return Err(invalid(
                &format!("side {}/vertices_plus", self.id),
                "cannot remove precise vertices with opaque fields",
            ));
        }
        self.extra
            .retain(|node| !is_precise_vertices_key(&node.key));
        Ok(())
    }
}

impl Solid {
    pub fn precise_vertices(&self) -> Result<Option<Vec<PreciseVertices>>, Error> {
        if !self.sides.iter().any(|side| {
            side.extra
                .iter()
                .any(|node| is_precise_vertices_key(&node.key))
                || side
                    .extra_properties
                    .iter()
                    .any(|(key, _)| is_precise_vertices_key(key))
        }) {
            return Ok(None);
        }
        let path = format!("solid {}/vertices_plus", self.id);
        if self.sides.len() > MAX_PRECISE_SIDES {
            return Err(invalid(&path, "precise brushes exceed the 64-side profile"));
        }
        let mut entries = 0;
        let mut rings = Vec::with_capacity(self.sides.len());
        for side in &self.sides {
            let ring = side
                .precise_vertices()?
                .ok_or_else(|| invalid(&path, "precise vertices must cover every brush side"))?;
            entries += ring.vertices.len();
            if entries > MAX_PRECISE_ENTRIES {
                return Err(invalid(
                    &path,
                    "precise brushes exceed their 4096-entry profile",
                ));
            }
            rings.push(ring);
        }
        Ok(Some(rings))
    }
}
