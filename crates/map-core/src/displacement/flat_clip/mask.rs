use std::collections::BTreeSet;

use glam::DVec3;

use super::{FIELD_WORK, Work, invalid, labels, native, same, triangles, volume};
use crate::surface::FaceId;
use crate::{BrushGeometry, DisplacementMesh, Document, EditError};

pub(crate) struct TriangleMask {
    source: FaceId,
    power: u32,
    ids: Vec<usize>,
    removed: Vec<bool>,
    triangles: Vec<[DVec3; 3]>,
    positions: Vec<DVec3>,
    axes: [usize; 2],
}

impl TriangleMask {
    pub(crate) fn new(
        document: &Document,
        source: FaceId,
        ids: &[usize],
        work: &mut Work,
    ) -> Result<Self, EditError> {
        let (_, solid) = crate::subobject::solid(document, source.solid)?;
        let geometry =
            BrushGeometry::from_solid(solid).map_err(|error| invalid(error.to_string()))?;
        let face = geometry
            .faces
            .iter()
            .find(|face| solid.sides[face.side].id == source.side)
            .ok_or_else(|| invalid("Triangle removal requires an original terrain face"))?;
        let info = solid.sides[face.side]
            .displacement
            .as_ref()
            .ok_or_else(|| invalid("Triangle removal requires an original displacement"))?;
        if !(2..=4).contains(&info.power) {
            return Err(invalid("Triangle removal requires native power 2, 3 or 4"));
        }
        let cells = 1usize << info.power;
        let count = 2 * cells * cells;
        work.consume(FIELD_WORK * ((cells + 1).pow(2) + count + ids.len()))
            .map_err(invalid)?;
        let ids = ids.iter().copied().collect::<BTreeSet<_>>();
        if ids.iter().any(|&id| id >= count) {
            return Err(invalid("Select valid original native terrain triangle IDs"));
        }
        let (_, mut mesh) = native::checked_mesh(face, info).map_err(invalid)?;
        triangles::native_tags(info, &mesh)?;
        let (data, base) = super::data(face, &mesh, info.power)?;
        super::native_base(&mut mesh, &data, base, work)?;
        let (_, axes) = labels::projection(&mesh, work)?;
        let native = triangles::indices(info.power)?;
        let source_order = triangles::source_indices(info.power)?;
        let correspondence = triangles::correspondence(&native, &source_order)?;
        work.consume(FIELD_WORK * count).map_err(invalid)?;
        let mut removed = vec![false; count];
        let mut selected = Vec::with_capacity(ids.len());
        for &id in &ids {
            removed[correspondence[id]] = true;
            selected.push(native[id].map(|index| mesh.base_positions[index]));
        }
        Ok(Self {
            source,
            power: info.power,
            ids: ids.into_iter().collect(),
            removed,
            triangles: selected,
            positions: mesh.base_positions,
            axes,
        })
    }

    pub(crate) fn source(&self) -> FaceId {
        self.source
    }

    pub(crate) fn power(&self) -> u32 {
        self.power
    }

    pub(crate) fn ids(&self) -> &[usize] {
        &self.ids
    }

    pub(crate) fn base_triangles(&self) -> &[[DVec3; 3]] {
        &self.triangles
    }

    pub(crate) fn is_removed_source(&self, index: usize) -> Result<bool, EditError> {
        self.removed
            .get(index)
            .copied()
            .ok_or_else(|| invalid("Triangle removal lost its original source triangle order"))
    }

    pub(crate) fn verify_source(
        &self,
        source: FaceId,
        mesh: &DisplacementMesh,
        work: &mut Work,
    ) -> Result<(), EditError> {
        work.consume(FIELD_WORK * self.positions.len())
            .map_err(invalid)?;
        if source != self.source
            || mesh.size != (1usize << self.power) + 1
            || self.positions.len() != mesh.base_positions.len()
            || self
                .positions
                .iter()
                .zip(&mesh.base_positions)
                .any(|(&a, &b)| !same(a, b))
        {
            return Err(invalid(
                "Triangle removal must retain its original face, power and exact BASE domain",
            ));
        }
        Ok(())
    }

    pub(super) fn covers(
        &self,
        base: [DVec3; 3],
        children: &[[DVec3; 4]],
        work: &mut Work,
    ) -> Result<bool, EditError> {
        volume::base_covered(base, children, self.base_triangles(), self.axes, work)
    }

    pub(super) fn intersects(&self, base: [DVec3; 3], work: &mut Work) -> Result<bool, EditError> {
        let projected = labels::Projected::new(base, self.axes)?;
        for triangle in self.base_triangles() {
            work.consume(FIELD_WORK).map_err(invalid)?;
            if projected.overlaps(
                &labels::Projected::new(*triangle, self.axes)?,
                self.axes,
                work,
            )? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
