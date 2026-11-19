use std::collections::BTreeMap;

use glam::DVec3;
use map_core::{
    DisplacementMesh, ObjectId, overlay,
    overlay_projection::{CLIP_EPSILON, Frame},
};
use source_formats::{
    bsp,
    gamefs::{GameFs, asset_relative},
    vmf::Vmf,
    vmt::texture_path,
    vtf::ImageFormat,
};

use crate::materials::{MaterialId, MaterialInfo, MaterialLibrary, resolved_material_limited};

use super::{SceneIssue, SceneIssues, Vertex};

const MAX_MATERIALS: usize = 128;
const MAX_TEXTURE_BYTES: usize = 16 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_TRIANGLE_VISITS: usize = 1_000_000;
const MAX_VERTICES: usize = 262_144;
const SURFACE_OFFSET: f64 = 0.1;

pub struct Draft {
    pub entity: Option<u32>,
    pub water_transition: bool,
    pub definition: overlay::Definition,
}

pub struct Preview {
    pub draft: Option<Draft>,
}

pub(crate) const UNLIT_RECEIVER: u32 = 6;
type SurfaceTriangle = ([DVec3; 3], [DVec3; 3], [DVec3; 3]);

#[derive(Default)]
pub(super) struct Projections {
    by_side: BTreeMap<u32, Vec<usize>>,
    layers: Vec<Layer>,
    triangle_visits: usize,
    vertices: usize,
}

struct Layer {
    entity: Option<u32>,
    sides_remaining: usize,
    order: u16,
    material: String,
    fade: bool,
    visible_receiver: bool,
    frame: Frame,
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    issue: Option<String>,
}

#[derive(Clone, Copy)]
struct ProjectionMaterial {
    id: MaterialId,
    unlit: bool,
    texture_alpha: bool,
    transform: [f32; 4],
}

fn load_material(
    fs: &GameFs,
    library: &mut MaterialLibrary,
    name: &str,
    remaining_image_bytes: &mut usize,
) -> Result<ProjectionMaterial, String> {
    let material = resolved_material_limited(fs, name, 1024 * 1024)?
        .ok_or_else(|| format!("material {name} is missing"))?;
    let unlit = material.shader == "unlitgeneric";
    if (!unlit && material.shader != "lightmappedgeneric") || !material.blocks.is_empty() {
        return Err(format!(
            "material {name} requires a plain LightmappedGeneric or UnlitGeneric shader"
        ));
    }
    for (key, value) in &material.params {
        let supported = match key.as_str() {
            "$basetexture" | "$basetexturetransform" | "$surfaceprop" | "%keywords" => true,
            "$decal" | "$translucent" => matches!(value.as_str(), "0" | "1"),
            "$decalscale" => value
                .parse::<f32>()
                .is_ok_and(|scale| scale.is_finite() && scale > 0.0),
            "$alphatest" | "$frame" | "$selfillum" | "$additive" => value == "0",
            "$alpha" => value == "1",
            _ => false,
        };
        if !supported {
            return Err(format!("material {name} has unsupported parameter {key}"));
        }
    }
    let transform = crate::sky::texture_transform(material.get("$basetexturetransform"))?;
    let base = material
        .get("$basetexture")
        .ok_or("material needs a base texture")?;
    if base.trim().is_empty() {
        return Err("material base texture is blank".into());
    }
    asset_relative(base).map_err(|error| error.to_string())?;
    let path = texture_path(base);
    let mut texture_alpha = false;
    let id = library.load_validated(fs, name, [1024 * 1024, MAX_TEXTURE_BYTES], |info| {
        if let Some(error) = &info.error {
            return Err(error.clone());
        }
        let texture = info
            .base
            .as_ref()
            .filter(|_| {
                info.matches_material(&material)
                    && !info.missing
                    && !info.tool
                    && info.display_texture.as_deref() == Some(path.as_str())
            })
            .ok_or_else(|| format!("material {name} has no current base texture {path}"))?;
        if texture.encoded_size() > MAX_TEXTURE_BYTES {
            return Err(format!(
                "{path} exceeds the {MAX_TEXTURE_BYTES}-byte overlay texture limit"
            ));
        }
        texture_alpha = texture.has_alpha();
        crate::normal_maps::validate_static_texture(texture, 2048)?;
        texture
            .validate_native_mips()
            .map_err(|error| format!("{path}: {error}"))?;
        if texture.flags & 0x2a01_c88d != 0
            || !matches!(
                texture.format,
                ImageFormat::Rgb888
                    | ImageFormat::Bgr888
                    | ImageFormat::Rgba8888
                    | ImageFormat::Bgra8888
                    | ImageFormat::Bgrx8888
                    | ImageFormat::Dxt1
                    | ImageFormat::Dxt1OneBitAlpha
                    | ImageFormat::Dxt3
                    | ImageFormat::Dxt5
            )
        {
            return Err(format!(
                "{path} requires an ordinary static repeating RGB/RGBA/DXT texture"
            ));
        }
        let bytes = (0..texture.mip_count)
            .map(|mip| {
                let (width, height) = texture.mip_size(mip);
                width as usize * height as usize * 4
            })
            .sum::<usize>();
        *remaining_image_bytes = remaining_image_bytes
            .checked_sub(bytes)
            .ok_or("overlay preview exceeds its decoded texture budget")?;
        Ok(())
    })?;
    Ok(ProjectionMaterial {
        id,
        unlit,
        texture_alpha,
        transform,
    })
}

impl Projections {
    pub(super) fn prepare(
        vmf: &Vmf,
        draft: Option<Draft>,
        include: &impl Fn(ObjectId) -> bool,
        issues: &mut SceneIssues,
    ) -> Self {
        let mut projections = Self::default();
        let new = draft.as_ref().filter(|draft| draft.entity.is_none());
        let mut limits = [bsp::MAX_OVERLAYS, bsp::MAX_WATER_OVERLAYS];
        if let Some(draft) = new {
            limits[usize::from(draft.water_transition)] -= 1;
        }
        let mut counts = [0; 2];
        let mut exhausted = [false; 2];
        for entity in vmf.entities.iter().filter(|entity| !entity.hidden) {
            let water = overlay::water::is_transition(entity);
            let family = usize::from(water);
            if exhausted[family]
                || (!water
                    && !entity
                        .classname()
                        .is_some_and(|class| class.eq_ignore_ascii_case("info_overlay")))
                || !include(ObjectId::Entity(entity.id))
            {
                continue;
            }
            let definitions = if let Some(draft) = draft
                .as_ref()
                .filter(|draft| draft.entity == Some(entity.id))
            {
                Ok(vec![draft.definition.clone()])
            } else if water {
                overlay::water::parse(entity)
            } else {
                overlay::parse(entity).map(|definition| vec![definition])
            };
            match definitions {
                Ok(definitions) if counts[family] + definitions.len() <= limits[family] => {
                    counts[family] += definitions.len();
                    for definition in definitions {
                        projections.add(Some(entity.id), definition, issues);
                    }
                }
                Ok(_) => {
                    let family_label = if water { "water transition" } else { "overlay" };
                    let limit = limits[family];
                    issues.push(SceneIssue::Overlay {
                        entity: Some(entity.id),
                        message: format!(
                            "{family_label} preview exceeds its {limit}-projection limit"
                        ),
                    });
                    exhausted[family] = true;
                }
                Err(message) => issues.push(SceneIssue::Overlay {
                    entity: Some(entity.id),
                    message,
                }),
            }
        }
        if let Some(draft) = draft.filter(|draft| draft.entity.is_none()) {
            projections.add(None, draft.definition, issues);
        }
        projections
    }

    fn add(
        &mut self,
        entity: Option<u32>,
        definition: overlay::Definition,
        issues: &mut SceneIssues,
    ) {
        let frame = match Frame::new(&definition) {
            Ok(frame) => frame,
            Err(message) => {
                issues.push(SceneIssue::Overlay { entity, message });
                return;
            }
        };
        for side in &definition.sides {
            self.by_side
                .entry(*side)
                .or_default()
                .push(self.layers.len());
        }
        self.layers.push(Layer {
            entity,
            sides_remaining: definition.sides.len(),
            order: definition.options.render_order,
            material: definition.options.material,
            fade: definition.options.fade_max > 0.0,
            visible_receiver: false,
            frame,
            vertices: Vec::new(),
            indices: Vec::new(),
            issue: None,
        });
    }

    pub(super) fn hide_sides(&mut self, solid: &source_formats::vmf::Solid) {
        for side in &solid.sides {
            if let Some(layers) = self.by_side.get(&side.id) {
                for &index in layers {
                    self.layers[index].sides_remaining =
                        self.layers[index].sides_remaining.saturating_sub(1);
                }
            }
        }
    }

    pub(super) fn flat(
        &mut self,
        side: u32,
        base: &[DVec3],
        normal: DVec3,
        receiver: &MaterialInfo,
    ) {
        self.surface(side, normal, receiver, || {
            (1..base.len().saturating_sub(1)).map(|index| {
                let points = [base[0], base[index], base[index + 1]];
                (points, points, [normal; 3])
            })
        });
    }

    pub(super) fn terrain(
        &mut self,
        side: u32,
        mesh: &DisplacementMesh,
        normals: &[[f64; 3]],
        normal: DVec3,
        receiver: &MaterialInfo,
    ) {
        if !self.by_side.contains_key(&side) {
            return;
        }
        let [start, left, opposite, right] =
            map_core::displacement::native::stored_base_corners(mesh);
        let last = mesh.size - 1;
        let [a, b, c, d] = [0, last * mesh.size, mesh.size * mesh.size - 1, last]
            .map(|index| mesh.base_positions[index]);
        if !(b + d - a).abs_diff_eq(c, CLIP_EPSILON) {
            let Some(layers) = self.by_side.remove(&side) else {
                return;
            };
            for index in layers {
                self.layers[index].issue = Some(format!(
                    "Side {side}: projected preview requires a parallelogram terrain base"
                ));
            }
            return;
        }
        let intervals = (mesh.size - 1) as f64;
        let base = |index: usize| {
            let u = (index % mesh.size) as f64 / intervals;
            let v = (index / mesh.size) as f64 / intervals;
            start.lerp(right, u).lerp(left.lerp(opposite, u), v)
        };
        self.surface(side, normal, receiver, || {
            mesh.triangles().map(|indices| {
                (
                    indices.map(base),
                    indices.map(|index| mesh.positions[index]),
                    indices.map(|index| DVec3::from_array(normals[index])),
                )
            })
        });
    }

    fn surface<I: Iterator<Item = SurfaceTriangle>>(
        &mut self,
        side: u32,
        normal: DVec3,
        receiver: &MaterialInfo,
        triangles: impl Fn() -> I,
    ) {
        let Some(layers) = self.by_side.remove(&side) else {
            return;
        };
        for index in layers {
            let layer = &mut self.layers[index];
            layer.sides_remaining = layer.sides_remaining.saturating_sub(1);
            layer.visible_receiver = true;
            if layer.issue.is_some() {
                continue;
            }
            let result =
                if receiver.tool || receiver.missing || receiver.alpha_test || receiver.translucent
                {
                    Err("the receiver needs an available opaque surface".into())
                } else if normal.dot(layer.frame.direction()).abs() < 1e-5 {
                    Err("the projection direction is tangent to a receiver".into())
                } else {
                    triangles().try_for_each(|(base, positions, normals)| {
                        if self.triangle_visits == MAX_TRIANGLE_VISITS {
                            return Err(
                                "overlay preview exceeds its projected triangle work limit".into(),
                            );
                        }
                        self.triangle_visits += 1;
                        layer.triangle(base, positions, normals, &mut self.vertices)
                    })
                };
            if let Err(error) = result {
                layer.issue = Some(format!("Side {side}: {error}"));
            }
        }
    }

    pub(super) fn finish(
        mut self,
        fs: &GameFs,
        library: &mut MaterialLibrary,
        (vertices, indices, batches): (&mut Vec<Vertex>, &mut Vec<u32>, &mut Vec<super::Batch>),
        issues: &mut SceneIssues,
    ) {
        let mut materials = BTreeMap::new();
        let mut remaining_image_bytes = MAX_IMAGE_BYTES;
        self.layers.sort_by_key(|layer| layer.order);
        for mut layer in self.layers {
            let entity = layer.entity;
            let result = (|| {
                if let Some(issue) = layer.issue.take() {
                    return Err(issue);
                }
                if layer.sides_remaining != 0 {
                    return Err("one or more bound sides have no eligible preview surface".into());
                }
                if !layer.visible_receiver {
                    return Ok(());
                }
                if layer.indices.is_empty() {
                    return Err("the projection does not intersect any visible bound side".into());
                }
                if layer.fade {
                    return Err("distance fades are unavailable in the editor preview".into());
                }
                let name = &layer.material;
                if !materials.contains_key(name) && materials.len() == MAX_MATERIALS {
                    return Err("overlay preview exceeds its unique material limit".into());
                }
                let material = materials
                    .entry(name.clone())
                    .or_insert_with(|| load_material(fs, library, name, &mut remaining_image_bytes))
                    .clone()?;
                layer.apply_material(material)?;
                let base = u32::try_from(vertices.len())
                    .map_err(|_| "overlay preview exceeds the scene vertex limit")?;
                if u32::try_from(vertices.len().saturating_add(layer.vertices.len())).is_err() {
                    return Err("overlay preview exceeds the scene vertex limit".into());
                }
                let first_index = u32::try_from(indices.len())
                    .map_err(|_| "overlay preview exceeds the scene index limit")?;
                if u32::try_from(indices.len().saturating_add(layer.indices.len())).is_err() {
                    return Err("overlay preview exceeds the scene index limit".into());
                }
                batches.push(super::Batch {
                    material: material.id,
                    first_index,
                    index_count: layer.indices.len() as u32,
                    rope: false,
                    model_translucent: false,
                });
                vertices.extend(layer.vertices);
                indices.extend(layer.indices.into_iter().map(|index| base + index));
                Ok(())
            })();
            if let Err(message) = result {
                issues.push(SceneIssue::Overlay { entity, message });
            }
        }
    }
}

impl Layer {
    fn apply_material(&mut self, material: ProjectionMaterial) -> Result<(), String> {
        let [sx, sy, tx, ty] = material.transform;
        for vertex in &mut self.vertices {
            let [u, v] = vertex.uv;
            vertex.uv = [u * sx + tx, v * sy + ty];
            if !vertex.uv.iter().all(|value| value.is_finite()) {
                return Err("overlay texture transform exceeds native float bounds".into());
            }
            vertex.texture_alpha = u32::from(material.texture_alpha);
            if material.unlit {
                vertex.lighting_receiver = UNLIT_RECEIVER;
            }
        }
        Ok(())
    }

    fn triangle(
        &mut self,
        base: [DVec3; 3],
        positions: [DVec3; 3],
        normals: [DVec3; 3],
        vertex_budget: &mut usize,
    ) -> Result<(), String> {
        let points = base.map(|point| self.frame.point(point));
        let positions =
            std::array::from_fn(|index| positions[index] + normals[index] * SURFACE_OFFSET);
        let [a, b, c] = points.map(|p| p.truncate());
        let cross = (b - a).perp_dot(c - a);
        if !cross.is_finite() || cross.abs() <= CLIP_EPSILON {
            return Err("receiver triangle has degenerate projected coordinates".into());
        }
        let polygon = self.frame.clip(points.to_vec());
        if polygon.len() < 3 {
            return Ok(());
        }
        let first = self.vertices.len() as u32;
        let index_start = self.indices.len();
        for index in 1..polygon.len() - 1 {
            let [a, b, c] = [polygon[0], polygon[index], polygon[index + 1]].map(|p| p.truncate());
            if (b - a).perp_dot(c - a).abs() > CLIP_EPSILON {
                self.indices
                    .extend([first, first + index as u32, first + index as u32 + 1]);
            }
        }
        if self.indices.len() == index_start {
            return Ok(());
        }
        *vertex_budget += polygon.len();
        if *vertex_budget > MAX_VERTICES {
            return Err("overlay preview exceeds its projected vertex limit".into());
        }
        for point in polygon {
            let p = point.truncate();
            let y = (p - a).perp_dot(c - a) / cross;
            let z = (b - a).perp_dot(p - a) / cross;
            let interpolate = |[a, b, c]: [DVec3; 3]| a + (b - a) * y + (c - a) * z;
            let position = interpolate(positions).as_vec3();
            let normal = interpolate(normals)
                .try_normalize()
                .ok_or("invalid projected normal")?
                .as_vec3();
            let uv = self.frame.texcoord(p)?.as_vec2();
            if !position.is_finite() || !uv.is_finite() {
                return Err("overlay projection exceeds native float bounds".into());
            }
            self.vertices.push(Vertex {
                position: position.to_array(),
                normal: normal.to_array(),
                uv: uv.to_array(),
                color: [255; 4],
                lightmap: [0.0; 3],
                smoothing: 0,
                opacity: 1.0,
                texture_alpha: 0,
                tangent: [0.0; 4],
                lighting_receiver: 0,
                baked_irradiance: [0.0; 3],
            });
        }
        Ok(())
    }
}
