use super::*;
use crate::surface::{Projection, SurfaceEdit};
use source_formats::vmf::{Side, UvAxis};

const PROJECTION_EPSILON: f64 = 1e-10;
const TEXEL_EPSILON: f64 = 0.0001;

fn same_projection(actual: &Projection, expected: &Projection, points: &[DVec3]) -> bool {
    for (actual, expected) in [(actual.u, expected.u), (actual.v, expected.v)] {
        let actual_gradient = actual.axis / actual.scale;
        let expected_gradient = expected.axis / expected.scale;
        if !actual_gradient.is_finite()
            || !expected_gradient.is_finite()
            || !actual.offset.is_finite()
            || !expected.offset.is_finite()
        {
            return false;
        }
        let magnitude = actual_gradient
            .abs()
            .max(expected_gradient.abs())
            .max_element()
            .max(1.0);
        if (actual_gradient - expected_gradient).abs().max_element()
            > PROJECTION_EPSILON * magnitude
            || (actual.offset - expected.offset).abs() > PROJECTION_EPSILON
            || points.iter().any(|point| {
                let actual_uv = point.dot(actual_gradient) + actual.offset;
                let expected_uv = point.dot(expected_gradient) + expected.offset;
                !actual_uv.is_finite()
                    || !expected_uv.is_finite()
                    || (actual_uv - expected_uv).abs() > TEXEL_EPSILON
            })
        {
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartitionTexture {
    pub shift: [f64; 2],
    pub scale: [f64; 2],
    pub rotation: f64,
    pub axes: Option<[[f64; 3]; 2]>,
}

impl PartitionTexture {
    pub fn from_side(side: &Side) -> Self {
        Self {
            shift: [side.u_axis.offset, side.v_axis.offset],
            scale: [side.u_axis.scale, side.v_axis.scale],
            rotation: side.rotation,
            axes: None,
        }
    }

    pub fn from_projection(projection: &Projection) -> Self {
        Self {
            shift: [projection.u.offset, projection.v.offset],
            scale: [projection.u.scale, projection.v.scale],
            rotation: projection.rotation,
            axes: Some([projection.u.axis.to_array(), projection.v.axis.to_array()]),
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
        if self
            .axes
            .iter()
            .flatten()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(invalid("Planned texture axes must be finite"));
        }
        Ok(())
    }

    pub fn project(&self, source: &Side) -> Result<Projection, EditError> {
        self.validate()?;
        let mut side = source.clone();
        let edit = if let Some([u, v]) = self.axes {
            SurfaceEdit {
                projection: Some(Projection {
                    u: UvAxis {
                        axis: DVec3::from_array(u),
                        offset: self.shift[0],
                        scale: self.scale[0],
                    },
                    v: UvAxis {
                        axis: DVec3::from_array(v),
                        offset: self.shift[1],
                        scale: self.scale[1],
                    },
                    rotation: self.rotation,
                }),
                ..Default::default()
            }
        } else {
            SurfaceEdit {
                shift_u: Some(self.shift[0]),
                shift_v: Some(self.shift[1]),
                scale_u: Some(self.scale[0]),
                scale_v: Some(self.scale[1]),
                rotation: Some(self.rotation),
                ..Default::default()
            }
        };
        crate::surface::update(&mut side, &edit, &[])?;
        Ok(Projection {
            u: side.u_axis,
            v: side.v_axis,
            rotation: side.rotation,
        })
    }

    pub(crate) fn reproject(
        &self,
        source: &Side,
        projection: &Projection,
        points: &[DVec3],
    ) -> Result<Self, EditError> {
        if self.axes.is_none()
            && let Ok(current) = self.project(source)
            && same_projection(&current, projection, points)
        {
            return Ok(*self);
        }
        let explicit = Self::from_projection(projection);
        explicit.validate()?;
        let mut projected = source.clone();
        crate::surface::update(
            &mut projected,
            &SurfaceEdit {
                projection: Some(projection.clone()),
                ..Default::default()
            },
            points,
        )?;
        if self.axes.is_some() {
            return Ok(explicit);
        }
        let normal = Plane::from_points(source.plane.0)
            .ok_or_else(|| invalid("Saved partition target face has an invalid plane"))?
            .normal;
        let desired_u = projection.u.axis / projection.u.scale;
        let source_tangent = source.u_axis.axis - normal * source.u_axis.axis.dot(normal);
        let desired_tangent = desired_u - normal * desired_u.dot(normal);
        let Some(source_tangent) = source_tangent.try_normalize() else {
            return Ok(explicit);
        };
        let Some(desired_tangent) = desired_tangent.try_normalize() else {
            return Ok(explicit);
        };
        let angle = normal
            .dot(source_tangent.cross(desired_tangent))
            .atan2(source_tangent.dot(desired_tangent));
        for angle in [angle, angle + std::f64::consts::PI] {
            let rotation = (source.rotation + angle.to_degrees()).rem_euclid(360.0);
            let turn =
                glam::DQuat::from_axis_angle(normal, (rotation - source.rotation).to_radians());
            let mut texture = Self {
                shift: [projection.u.offset, projection.v.offset],
                scale: [0.0; 2],
                rotation,
                axes: None,
            };
            for (index, (base, desired)) in [
                (source.u_axis.axis, projection.u),
                (source.v_axis.axis, projection.v),
            ]
            .into_iter()
            .enumerate()
            {
                let gradient = desired.axis / desired.scale;
                let Some(direction) = gradient.try_normalize() else {
                    return Ok(explicit);
                };
                texture.scale[index] = (turn * base).dot(direction) / gradient.length();
            }
            if let Ok(actual) = texture.project(source)
                && same_projection(&actual, projection, points)
            {
                return Ok(texture);
            }
        }
        Ok(explicit)
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
            axes: self
                .axes
                .map(|_| [projection.u.axis.to_array(), projection.v.axis.to_array()]),
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

pub fn crease_partitions_with_surface_options(
    tx: &mut Transaction,
    partitions: &[Partition],
    rise: f64,
    roof_texture_lock: bool,
    neighbor_texture_lock: bool,
    materials: &[PartitionMaterial],
    textures: &[PieceTexture],
) -> Result<CreasePlan, EditError> {
    let originals = partitions
        .iter()
        .map(|part| {
            solid(tx.document(), part.source.solid).map(|(_, source)| (source.id, source.clone()))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let mut plan = crease_partitions_with_surfaces(
        tx,
        partitions,
        rise,
        roof_texture_lock,
        materials,
        textures,
    )?;
    if neighbor_texture_lock {
        super::ridge_fragments::attach(tx, &mut plan, &originals)?;
    }
    Ok(plan)
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
