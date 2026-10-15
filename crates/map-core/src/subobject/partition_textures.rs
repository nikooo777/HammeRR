use super::*;
use crate::surface::{Projection, SurfaceEdit};
use source_formats::vmf::Side;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartitionTexture {
    pub shift: [f64; 2],
    pub scale: [f64; 2],
    pub rotation: f64,
}

impl PartitionTexture {
    pub fn from_side(side: &Side) -> Self {
        Self {
            shift: [side.u_axis.offset, side.v_axis.offset],
            scale: [side.u_axis.scale, side.v_axis.scale],
            rotation: side.rotation,
        }
    }

    pub fn validate(&self) -> Result<(), EditError> {
        if self
            .shift
            .into_iter()
            .chain(self.scale)
            .chain([self.rotation])
            .any(|value| !value.is_finite() || value.abs() > MAX_COORDINATE)
            || self.scale.iter().any(|scale| scale.abs() < 1e-8)
        {
            return Err(invalid(
                "Planned texture values must be finite, within ±65536 and have nonzero scales",
            ));
        }
        Ok(())
    }

    pub fn project(&self, source: &Side) -> Result<Projection, EditError> {
        self.validate()?;
        let mut side = source.clone();
        crate::surface::update(
            &mut side,
            &SurfaceEdit {
                shift_u: Some(self.shift[0]),
                shift_v: Some(self.shift[1]),
                scale_u: Some(self.scale[0]),
                scale_v: Some(self.scale[1]),
                rotation: Some(self.rotation),
                ..Default::default()
            },
            &[],
        )?;
        Ok(Projection {
            u: side.u_axis,
            v: side.v_axis,
            rotation: side.rotation,
        })
    }

    pub fn place(
        &self,
        source: &Side,
        points: &[DVec3],
        dimensions: [u32; 2],
        fit: bool,
        justify: [Option<crate::surface::Justify>; 2],
    ) -> Result<Self, EditError> {
        let projection = crate::surface::placed_projection(
            &self.project(source)?,
            points,
            fit.then_some(dimensions),
            [
                justify[0].map(|value| (value, dimensions[0])),
                justify[1].map(|value| (value, dimensions[1])),
            ],
        )?;
        let texture = Self {
            shift: [projection.u.offset, projection.v.offset],
            scale: [projection.u.scale, projection.v.scale],
            rotation: projection.rotation,
        };
        texture.validate()?;
        Ok(texture)
    }

    pub(super) fn apply(
        &self,
        source: &Side,
        target: &mut Side,
        before: &[DVec3],
        after: &[DVec3],
        attached: bool,
    ) -> Result<(), EditError> {
        let projection = self.project(source)?;
        let mut side = source.clone();
        side.u_axis = projection.u;
        side.v_axis = projection.v;
        side.rotation = projection.rotation;
        let projection = if attached {
            crate::texture_deform::attach(&side, before, after)?
        } else {
            projection
        };
        target.u_axis = projection.u;
        target.v_axis = projection.v;
        target.rotation = projection.rotation;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PieceTexture {
    pub solid: u32,
    pub partition: u32,
    pub texture: PartitionTexture,
}

pub(super) fn checked_textures(
    document: &Document,
    partitions: &[Partition],
    textures: &[PieceTexture],
) -> Result<BTreeMap<(u32, u32), PartitionTexture>, EditError> {
    if partitions.is_empty() || partitions.len() > MAX_SOLIDS || textures.len() > MAX_SOLIDS * 2 {
        return Err(invalid(
            "Plan textures for at most 32 pieces in 1–16 brushes",
        ));
    }
    let mut result = BTreeMap::new();
    for texture in textures {
        let part = partitions
            .iter()
            .find(|part| {
                part.source.solid == texture.solid
                    && part.loops.iter().any(|piece| piece.id == texture.partition)
            })
            .ok_or_else(|| invalid("Choose texture targets from the current partitions"))?;
        texture.texture.validate()?;
        let source = crate::surface::side(
            document,
            crate::surface::FaceId {
                solid: part.source.solid,
                side: part.source.side,
            },
        )
        .ok_or_else(|| invalid("Partition source face does not exist"))?;
        texture.texture.project(source)?;
        if result
            .insert((texture.solid, texture.partition), texture.texture)
            .is_some()
        {
            return Err(invalid("Choose distinct existing partition piece IDs"));
        }
    }
    Ok(result)
}

pub fn crease_partitions_with_surfaces(
    tx: &mut Transaction,
    partitions: &[Partition],
    rise: f64,
    texture_lock: bool,
    materials: &[PartitionMaterial],
    textures: &[PieceTexture],
) -> Result<CreasePlan, EditError> {
    let materials =
        super::partition_materials::checked_materials(tx.document(), partitions, materials)?;
    let textures = checked_textures(tx.document(), partitions, textures)?;
    let originals = partitions
        .iter()
        .map(|part| {
            crate::surface::side(
                tx.document(),
                crate::surface::FaceId {
                    solid: part.source.solid,
                    side: part.source.side,
                },
            )
            .cloned()
            .ok_or_else(|| invalid("Partition source face does not exist"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut plan = super::crease::crease_partition_roofs(tx, partitions, rise, texture_lock)?;
    for (part, original) in partitions.iter().zip(originals) {
        let change = plan
            .changes
            .iter()
            .find(|change| change.source == part.source)
            .unwrap();
        let replacement = plan
            .replacements
            .iter_mut()
            .find(|replacement| replacement.solid.id == part.source.solid)
            .unwrap();
        let positions = part.positions(rise);
        for piece in &part.loops {
            let key = (part.source.solid, piece.id);
            let texture = textures.get(&key);
            let material = materials.get(&key);
            if texture.is_none() && material.is_none() {
                continue;
            }
            let before = piece
                .vertices
                .iter()
                .map(|vertex| part.vertices[*vertex as usize].position)
                .collect::<Vec<_>>();
            let after = piece
                .vertices
                .iter()
                .map(|vertex| positions[*vertex as usize])
                .collect::<Vec<_>>();
            let target = replacement
                .solid
                .sides
                .iter_mut()
                .filter(|side| side.id == part.source.side || side.id == change.new_side)
                .filter(|side| {
                    let plane = Plane::from_points(side.plane.0).unwrap();
                    after
                        .iter()
                        .all(|point| plane.distance_to(*point).abs() < EPSILON)
                })
                .collect::<Vec<_>>();
            let [target]: [&mut Side; 1] = target.try_into().map_err(|_: Vec<_>| {
                invalid("Planned texture piece must match exactly one roof face")
            })?;
            if let Some(material) = material {
                target.material.clone_from(material);
            }
            if let Some(texture) = texture {
                texture.apply(&original, target, &before, &after, texture_lock)?;
            }
        }
    }
    apply(tx, &plan.replacements)?;
    Ok(plan)
}
