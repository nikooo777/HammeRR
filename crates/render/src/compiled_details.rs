use std::collections::{BTreeMap, HashSet};

use glam::{DQuat, DVec3, EulerRot, Vec3};
use source_formats::{
    bsp::{DetailProp, DetailSprite},
    gamefs::GameFs,
};

use crate::{
    MaterialLibrary, SceneMesh,
    models::Instance,
    scene::{Batch, LineVertex, Vertex},
};

pub const MAX_VERTICES: usize = 262_144;
pub const MAX_INDICES: usize = 786_432;
pub const MAX_BYTES: usize = 32 * 1024 * 1024;
const FULL_BILLBOARD_TAG: f32 = -2.5;
const VERTICAL_BILLBOARD_TAG: f32 = -3.5;

#[derive(Debug, Clone)]
pub struct Part {
    pub ordinal: usize,
    pub vertices: std::ops::Range<usize>,
    pub wires: std::ops::Range<usize>,
    pub billboards: std::ops::Range<usize>,
    pub bounds: [DVec3; 2],
}

impl Part {
    pub fn intersects(&self, region: [DVec3; 2]) -> bool {
        self.bounds[0].cmple(region[1]).all() && self.bounds[1].cmpge(region[0]).all()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Billboard {
    pub origin: [f32; 3],
    pub offsets: [[f32; 2]; 4],
    pub orientation: u8,
}

impl Billboard {
    pub fn quad(self, eye: Vec3) -> [Vec3; 4] {
        let origin = Vec3::from_array(self.origin);
        let mut direction = eye - origin;
        if self.orientation == 2 {
            direction.z = 0.0;
        }
        let maximum = direction.abs().max_element();
        if maximum > 0.0 {
            direction /= maximum;
        }
        let horizontal = direction.x.hypot(direction.y);
        let (right, up) = if horizontal == 0.0 {
            (
                Vec3::NEG_Y,
                if direction.z > 0.0 {
                    Vec3::NEG_X
                } else {
                    Vec3::X
                },
            )
        } else {
            let right = Vec3::new(direction.y / horizontal, -direction.x / horizontal, 0.0);
            let length = direction.length();
            (
                right,
                Vec3::new(
                    -direction.x * direction.z / (horizontal * length),
                    -direction.y * direction.z / (horizontal * length),
                    horizontal / length,
                ),
            )
        };
        self.offsets.map(|[x, y]| origin + right * x + up * y)
    }

    pub fn bounds(self) -> [DVec3; 2] {
        let radius = self
            .offsets
            .into_iter()
            .map(|[x, y]| f64::from(x).hypot(f64::from(y)))
            .fold(0.0f64, f64::max)
            + 1.0 / 64.0;
        let origin = DVec3::from_array(self.origin.map(f64::from));
        [origin - DVec3::splat(radius), origin + DVec3::splat(radius)]
    }

    fn new(prop: &DetailProp, sprite: DetailSprite) -> Result<Self, String> {
        if prop.kind != 1
            || !matches!(prop.orientation, 1 | 2)
            || !prop.scale.is_finite()
            || prop.scale <= 0.0
            || !Vec3::from_array(prop.origin).is_finite()
            || [
                sprite.upper_left,
                sprite.lower_right,
                sprite.texture_upper_left,
                sprite.texture_lower_right,
            ]
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite())
            || sprite.upper_left[0] >= sprite.lower_right[0]
            || sprite.upper_left[1] <= sprite.lower_right[1]
        {
            return Err("Invalid compiled billboard geometry".into());
        }
        let [left, top] = sprite.upper_left.map(|value| value * prop.scale);
        let [right, bottom] = sprite.lower_right.map(|value| value * prop.scale);
        let offsets = [[left, top], [left, bottom], [right, bottom], [right, top]];
        if offsets
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err("Compiled billboard offsets must be finite".into());
        }
        Ok(Self {
            origin: prop.origin,
            offsets,
            orientation: prop.orientation,
        })
    }
}

pub fn rest_quads(prop: &DetailProp, sprite: DetailSprite) -> Result<Vec<[DVec3; 4]>, String> {
    if [
        sprite.upper_left,
        sprite.lower_right,
        sprite.texture_upper_left,
        sprite.texture_lower_right,
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.is_finite())
        || !(1..=3).contains(&prop.kind)
        || prop.orientation > 2
        || !prop.scale.is_finite()
        || prop.scale <= 0.0
        || sprite.upper_left[0] >= sprite.lower_right[0]
        || sprite.upper_left[1] <= sprite.lower_right[1]
        || prop.orientation != 0
        || prop.kind == 2 && (prop.shape_angle != 0 || prop.shape_size != 0)
    {
        return Err("Unsupported compiled detail card geometry".into());
    }
    let origin = DVec3::from_array(prop.origin.map(f64::from));
    let radians = DVec3::from_array(prop.angles.map(f64::from)).map(f64::to_radians);
    let rotation = DQuat::from_euler(EulerRot::ZYX, radians.y, radians.x, radians.z);
    let width = f64::from(sprite.lower_right[0]) - f64::from(sprite.upper_left[0]);
    let height = f64::from(sprite.lower_right[1]) - f64::from(sprite.upper_left[1]);
    let mut quads = Vec::with_capacity(4);
    if prop.kind == 2 {
        let base = rotation * DVec3::Z * f64::from(sprite.upper_left[1]);
        let vertical = rotation * DVec3::Z * height;
        for axis in [DVec3::NEG_X, DVec3::Y, DVec3::X, DVec3::NEG_Y] {
            let horizontal = rotation * axis * (width * 0.5);
            quads.push([
                base,
                base + vertical,
                base + vertical + horizontal,
                base + horizontal,
            ]);
        }
    } else {
        let branches = if prop.kind == 3 { 3 } else { 1 };
        for branch in 0..branches {
            let basis = rotation
                * DQuat::from_euler(
                    EulerRot::ZYX,
                    f64::from(branch) * std::f64::consts::TAU / 3.0,
                    if prop.kind == 3 {
                        f64::from(prop.shape_angle).to_radians()
                    } else {
                        0.0
                    },
                    0.0,
                );
            let right = basis * DVec3::NEG_Y;
            let up = basis * DVec3::Z;
            let offset = if prop.kind == 3 {
                basis * DVec3::X * f64::from(f32::from(prop.shape_size) * (1.0_f32 / 255.0)) * width
            } else {
                DVec3::ZERO
            };
            let base = right * f64::from(sprite.upper_left[0])
                + up * f64::from(sprite.upper_left[1])
                + offset;
            let horizontal = right * width;
            let vertical = up * height;
            quads.push([
                base,
                base + vertical,
                base + vertical + horizontal,
                base + horizontal,
            ]);
        }
    }
    for quad in &mut quads {
        for point in quad {
            *point = origin + *point * f64::from(prop.scale);
            if !point.is_finite()
                || point.abs().max_element() > map_core::checks::SOURCE_COORDINATE_LIMIT
            {
                return Err("Compiled detail card bounds exceed finite Source coordinates".into());
            }
        }
    }
    Ok(quads)
}

pub fn model_scene(fs: &GameFs, materials: &mut MaterialLibrary, models: &[Instance]) -> SceneMesh {
    let document = map_core::Document::empty();
    SceneMesh::build_with_reference_models_filtered(
        document.vmf(),
        fs,
        materials,
        (&HashSet::new(), &HashSet::new()),
        &[],
        (&[], &BTreeMap::new()),
        (&BTreeMap::new(), &[]),
        None,
        true,
        false,
        false,
        false,
        models,
        |_| false,
    )
}

impl SceneMesh {
    pub fn add_compiled_detail_card(
        &mut self,
        material: usize,
        prop: &DetailProp,
        sprite: DetailSprite,
    ) -> Result<(), String> {
        if prop.kind == 1 && matches!(prop.orientation, 1 | 2) {
            return self.add_compiled_detail_billboard(material, prop, sprite);
        }
        let quads = rest_quads(prop, sprite)?;
        let vertices = quads.len() * 4;
        let indices = quads.len() * 12;
        self.check_compiled_detail_capacity(vertices, indices)?;
        let first_index = self.indices.len() as u32;
        let uvs = [
            sprite.texture_upper_left,
            [sprite.texture_upper_left[0], sprite.texture_lower_right[1]],
            sprite.texture_lower_right,
            [sprite.texture_lower_right[0], sprite.texture_upper_left[1]],
        ];
        for (branch, quad) in quads.into_iter().enumerate() {
            let first = self.vertices.len() as u32;
            let normal = (quad[1] - quad[0])
                .cross(quad[2] - quad[0])
                .normalize_or_zero()
                .as_vec3()
                .to_array();
            for (index, point) in quad.into_iter().enumerate() {
                let mut uv = uvs[index];
                if prop.kind == 2 {
                    let [left, right] =
                        [sprite.texture_upper_left[0], sprite.texture_lower_right[0]];
                    let middle = (left + right) * 0.5;
                    uv[0] = if index < 2 {
                        middle
                    } else if branch == 0 || branch == 3 {
                        right
                    } else {
                        left
                    };
                }
                self.vertices.push(Vertex {
                    position: point.as_vec3().to_array(),
                    normal,
                    uv,
                    color: [255, 255, 255, 0],
                    lightmap: [0.0, 0.0, -1.0],
                    smoothing: 0,
                    opacity: 1.0,
                    texture_alpha: 0,
                    tangent: [0.0; 4],
                    lighting_receiver: 0,
                });
                self.mins = self.mins.min(point.as_vec3());
                self.maxs = self.maxs.max(point.as_vec3());
            }
            self.indices
                .extend([0, 1, 2, 0, 2, 3, 2, 1, 0, 3, 2, 0].map(|index| first + index));
            for (a, b) in [(0, 1), (1, 2), (2, 3), (3, 0)] {
                self.wire_lines.extend([a, b].map(|index| LineVertex {
                    position: quad[index].as_vec3().to_array(),
                    color: [90, 125, 150, 255],
                }));
            }
        }
        self.batches.push(Batch {
            material,
            first_index,
            index_count: self.indices.len() as u32 - first_index,
            rope: false,
            model_translucent: true,
        });
        Ok(())
    }

    fn add_compiled_detail_billboard(
        &mut self,
        material: usize,
        prop: &DetailProp,
        sprite: DetailSprite,
    ) -> Result<(), String> {
        let billboard = Billboard::new(prop, sprite)?;
        self.check_compiled_detail_capacity(4, 12)?;
        let first = self.vertices.len() as u32;
        let first_index = self.indices.len() as u32;
        let uvs = [
            sprite.texture_upper_left,
            [sprite.texture_upper_left[0], sprite.texture_lower_right[1]],
            sprite.texture_lower_right,
            [sprite.texture_lower_right[0], sprite.texture_upper_left[1]],
        ];
        for (offset, uv) in billboard.offsets.into_iter().zip(uvs) {
            self.vertices.push(Vertex {
                position: prop.origin,
                normal: [0.0, 0.0, 1.0],
                uv,
                color: [255, 255, 255, 0],
                lightmap: [
                    offset[0],
                    offset[1],
                    if prop.orientation == 1 {
                        FULL_BILLBOARD_TAG
                    } else {
                        VERTICAL_BILLBOARD_TAG
                    },
                ],
                smoothing: 0,
                opacity: 1.0,
                texture_alpha: 0,
                tangent: [0.0; 4],
                lighting_receiver: 0,
            });
        }
        self.indices
            .extend([0, 1, 2, 0, 2, 3, 2, 1, 0, 3, 2, 0].map(|index| first + index));
        self.batches.push(Batch {
            material,
            first_index,
            index_count: 12,
            rope: false,
            model_translucent: true,
        });
        let radius = billboard
            .offsets
            .into_iter()
            .map(|[x, y]| x.hypot(y))
            .fold(0.0f32, f32::max);
        let origin = Vec3::from_array(prop.origin);
        self.mins = self.mins.min(origin - Vec3::splat(radius));
        self.maxs = self.maxs.max(origin + Vec3::splat(radius));
        self.detail_billboards.push(billboard);
        Ok(())
    }

    pub fn check_compiled_detail_capacity(
        &self,
        vertices: usize,
        indices: usize,
    ) -> Result<(), String> {
        let vertices = self
            .vertices
            .len()
            .checked_add(vertices)
            .ok_or("Compiled detail vertex count overflow")?;
        let indices = self
            .indices
            .len()
            .checked_add(indices)
            .ok_or("Compiled detail index count overflow")?;
        let bytes = vertices
            .checked_mul(size_of::<Vertex>())
            .and_then(|bytes| {
                bytes.checked_add(
                    indices.checked_mul(size_of::<u32>() + 2 * size_of::<LineVertex>())?,
                )
            })
            .and_then(|bytes| bytes.checked_mul(2))
            .and_then(|bytes| bytes.checked_add(size_of::<Self>()));
        if vertices > MAX_VERTICES
            || indices > MAX_INDICES
            || bytes.is_none_or(|bytes| bytes > MAX_BYTES)
        {
            return Err("Compiled details exceed the 32 MiB geometry budget".into());
        }
        Ok(())
    }

    pub fn compiled_detail_bytes(&self) -> usize {
        size_of::<Self>()
            + self.vertices.capacity() * size_of::<Vertex>()
            + self.indices.capacity() * size_of::<u32>()
            + self.batches.capacity() * size_of::<Batch>()
            + self.wire_lines.capacity() * size_of::<LineVertex>()
            + self.detail_billboards.capacity() * size_of::<Billboard>()
            + self.detail_parts.capacity() * size_of::<Part>()
    }

    pub fn append_compiled_details(
        &mut self,
        details: &Self,
        region: Option<[DVec3; 2]>,
    ) -> Result<(), String> {
        let included = region.map(|region| {
            details
                .detail_parts
                .iter()
                .map(|part| part.intersects(region))
                .collect::<Vec<_>>()
        });
        let mut allowed_vertices = Vec::new();
        if let Some(included) = &included {
            allowed_vertices
                .try_reserve_exact(details.vertices.len())
                .map_err(|_| "Compiled detail filter allocation failed")?;
            allowed_vertices.resize(details.vertices.len(), false);
            let mut cursor = [0usize; 3];
            for (part, enabled) in details.detail_parts.iter().zip(included) {
                for (slot, range, maximum) in [
                    (0, &part.vertices, details.vertices.len()),
                    (1, &part.wires, details.wire_lines.len()),
                    (2, &part.billboards, details.detail_billboards.len()),
                ] {
                    if range.start != cursor[slot] || range.start > range.end || range.end > maximum
                    {
                        return Err("Compiled detail spatial ranges are inconsistent".into());
                    }
                    cursor[slot] = range.end;
                }
                if *enabled {
                    allowed_vertices[part.vertices.clone()].fill(true);
                }
            }
            if cursor
                != [
                    details.vertices.len(),
                    details.wire_lines.len(),
                    details.detail_billboards.len(),
                ]
            {
                return Err("Compiled detail spatial ranges are incomplete".into());
            }
            if !included.iter().any(|enabled| *enabled) {
                return Ok(());
            }
        }
        for (existing, extra) in [
            (self.vertices.len(), details.vertices.len()),
            (self.indices.len(), details.indices.len()),
            (self.wire_lines.len(), details.wire_lines.len()),
        ] {
            if existing
                .checked_add(extra)
                .is_none_or(|count| count > u32::MAX as usize)
            {
                return Err("Combined detail scene exceeds renderer index bounds".into());
            }
        }
        self.vertices
            .try_reserve_exact(details.vertices.len())
            .map_err(|_| "Compiled detail vertex allocation failed")?;
        self.indices
            .try_reserve_exact(details.indices.len())
            .map_err(|_| "Compiled detail index allocation failed")?;
        self.batches
            .try_reserve_exact(details.batches.len())
            .map_err(|_| "Compiled detail batch allocation failed")?;
        self.wire_lines
            .try_reserve_exact(details.wire_lines.len())
            .map_err(|_| "Compiled detail line allocation failed")?;
        self.detail_billboards
            .try_reserve_exact(details.detail_billboards.len())
            .map_err(|_| "Compiled detail billboard allocation failed")?;
        let first_vertex = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&details.vertices);
        if let Some(included) = included {
            for batch in &details.batches {
                let first_index = self.indices.len() as u32;
                let range =
                    batch.first_index as usize..(batch.first_index + batch.index_count) as usize;
                for triangle in details.indices[range].as_chunks::<3>().0 {
                    if triangle
                        .iter()
                        .all(|index| allowed_vertices[*index as usize])
                    {
                        self.indices
                            .extend(triangle.iter().map(|index| first_vertex + index));
                    }
                }
                let index_count = self.indices.len() as u32 - first_index;
                if index_count != 0 {
                    self.batches.push(Batch {
                        first_index,
                        index_count,
                        ..*batch
                    });
                }
            }
            for (part, enabled) in details.detail_parts.iter().zip(included) {
                if !enabled {
                    continue;
                }
                self.wire_lines
                    .extend_from_slice(&details.wire_lines[part.wires.clone()]);
                self.detail_billboards
                    .extend_from_slice(&details.detail_billboards[part.billboards.clone()]);
                self.mins = self.mins.min(part.bounds[0].as_vec3());
                self.maxs = self.maxs.max(part.bounds[1].as_vec3());
            }
        } else {
            let first_index = self.indices.len() as u32;
            self.indices
                .extend(details.indices.iter().map(|index| first_vertex + index));
            self.batches
                .extend(details.batches.iter().map(|batch| Batch {
                    first_index: first_index + batch.first_index,
                    ..*batch
                }));
            self.wire_lines.extend_from_slice(&details.wire_lines);
            self.detail_billboards
                .extend_from_slice(&details.detail_billboards);
            if !details.vertices.is_empty() {
                self.mins = self.mins.min(details.mins);
                self.maxs = self.maxs.max(details.maxs);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_card_cross_and_tri_rest_coordinates_follow_native_records() {
        let sprite = DetailSprite {
            upper_left: [-4.0, 16.0],
            lower_right: [12.0, -16.0],
            ..Default::default()
        };
        let mut prop = DetailProp {
            kind: 1,
            origin: [10.0, 20.0, 30.0],
            scale: 2.0,
            ..Default::default()
        };
        let cards = rest_quads(&prop, sprite).unwrap();
        assert_eq!(
            cards[0],
            [
                DVec3::new(10.0, 28.0, 62.0),
                DVec3::new(10.0, 28.0, -2.0),
                DVec3::new(10.0, -4.0, -2.0),
                DVec3::new(10.0, -4.0, 62.0)
            ]
        );
        prop.kind = 2;
        assert_eq!(
            rest_quads(&prop, sprite).unwrap()[0][3],
            DVec3::new(-6.0, 20.0, 62.0)
        );
        prop.kind = 3;
        prop.shape_size = 255;
        let triangles = rest_quads(&prop, sprite).unwrap();
        assert_eq!(triangles.len(), 3);
        assert_eq!(triangles[0][0], DVec3::new(42.0, 28.0, 62.0));
        prop.orientation = 2;
        assert!(rest_quads(&prop, sprite).is_err());
        prop.kind = 1;
        assert!(rest_quads(&prop, sprite).is_err());
        assert!(Billboard::new(&prop, sprite).is_ok());
        prop.scale = f32::NAN;
        assert!(rest_quads(&prop, sprite).is_err());
    }
}

#[cfg(test)]
mod billboard_tests {
    use super::*;

    #[test]
    fn source_eye_facing_basis_preserves_anchor_vertical_mode_and_singular_views() {
        let sprite = DetailSprite {
            upper_left: [-2.0, 3.0],
            lower_right: [2.0, -1.0],
            ..Default::default()
        };
        let mut prop = DetailProp {
            kind: 1,
            orientation: 1,
            angles: [30.0, 80.0, 10.0],
            ..Default::default()
        };
        let full = Billboard::new(&prop, sprite).unwrap();
        let q = full.quad(Vec3::new(10.0, 0.0, 10.0));
        let diagonal = std::f32::consts::FRAC_1_SQRT_2 * 3.0;
        assert!((q[0] - Vec3::new(-diagonal, 2.0, diagonal)).length() < 1e-5);
        assert_eq!(full.quad(Vec3::Z)[0], Vec3::new(-3.0, 2.0, 0.0));
        assert_eq!(full.quad(Vec3::ZERO)[0], Vec3::new(3.0, 2.0, 0.0));
        prop.orientation = 2;
        let vertical = Billboard::new(&prop, sprite).unwrap();
        assert_eq!(
            vertical.quad(Vec3::new(10.0, 0.0, 10.0))[0],
            Vec3::new(0.0, 2.0, 3.0)
        );
        assert_eq!(vertical.quad(Vec3::Y)[0], Vec3::new(-2.0, 0.0, 3.0));
        assert_eq!(vertical.quad(Vec3::Z)[0], Vec3::new(3.0, 2.0, 0.0));
    }
}
