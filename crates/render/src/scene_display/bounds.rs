use crate::compiled_details::{
    BAKED_RADIANCE_RECEIVER, LIGHTING_RECEIVER_KIND_BITS, LIGHTING_RECEIVER_KIND_MASK,
};
use crate::compiled_faces::materials::{Kind, Material, MaterialLighting, NormalEncoding, Texture};
use crate::lighting::{Light, Preview};
use crate::scene::{Batch, Vertex};
use crate::{Camera, MaterialLibrary, SceneMesh, ViewOptions};

use super::MAX_HDR_RADIANCE;

const AUTHORED_RECEIVER: u32 = 1;
const BYTE_NORMALIZATION: f64 = 1.0 / 255.0;
const FLOAT_UNCERTAINTY: f64 = 64.0 * f32::EPSILON as f64;
const COMPOSITION_HEADROOM: f64 = 1.0 + FLOAT_UNCERTAINTY;

#[derive(Debug)]
pub struct SourceRadianceBounds {
    source: Result<Bounds, String>,
}

#[derive(Debug)]
struct Bounds {
    batches: Vec<BatchBounds>,
    compiled_details: Result<(), String>,
    authored: Result<[f64; 3], String>,
    authored_active: bool,
    sky: Option<(crate::sky::Channel, Result<[f64; 3], String>)>,
    miniature: Option<Result<(), String>>,
}

#[derive(Debug)]
struct BatchBounds {
    radiance: Result<[f64; 3], String>,
    authored: bool,
    reference: bool,
    rope: bool,
    tool: bool,
    sky: bool,
    sky3d_eligible: bool,
    helper: bool,
}

#[derive(Clone, Copy)]
struct FaceBounds {
    maps: [[f64; 3]; 4],
    count: u32,
}

#[derive(Clone, Copy)]
struct MaterialBounds {
    diffuse: [f64; 3],
    tint: [f64; 3],
    ssbump: Option<[f64; 3]>,
}

impl SourceRadianceBounds {
    pub fn from_scene(mesh: &SceneMesh, library: &MaterialLibrary) -> Self {
        Self {
            source: Bounds::from_scene(mesh, library),
        }
    }

    pub fn validate_frame(&self, options: &ViewOptions, camera: &Camera) -> Result<(), String> {
        let resolved = options.scene_display.resolve(camera, options.surface)?;
        if !resolved.active {
            return Ok(());
        }
        let bounds = self.source.as_ref().map_err(Clone::clone)?;
        bounds.compiled_details.as_ref().map_err(Clone::clone)?;
        let mut needs_authored = false;
        let mut needs_sky = false;
        let mut needs_miniature = false;
        for batch in &bounds.batches {
            if batch.helper
                || batch.reference && !options.instance_contents
                || batch.rope && !options.rope_preview
                || !options.tool_textures
                    && batch.tool
                    && !(batch.sky && options.sky_preview)
                    && !(batch.sky3d_eligible && options.sky3d_preview)
            {
                continue;
            }
            if batch.sky && options.sky_preview {
                needs_sky = true;
            } else {
                admit(
                    "Scene surface",
                    *batch.radiance.as_ref().map_err(Clone::clone)?,
                )?;
                needs_authored |= batch.authored;
            }
            needs_miniature |= batch.sky3d_eligible && options.sky3d_preview;
        }
        if needs_authored && options.lighting_preview && bounds.authored_active {
            admit(
                "Authored lighting",
                *bounds.authored.as_ref().map_err(Clone::clone)?,
            )?;
        }
        if needs_sky {
            let (channel, bound) = bounds
                .sky
                .as_ref()
                .ok_or("HDR scene requires the requested sky assets to be ready")?;
            if *channel != options.sky_channel {
                return Err(
                    "HDR scene sky assets do not match the requested content channel".into(),
                );
            }
            admit("Sky background", *bound.as_ref().map_err(Clone::clone)?)?;
        }
        if needs_miniature {
            bounds
                .miniature
                .as_ref()
                .ok_or("HDR scene requires the requested 3D sky assets to be ready")?
                .as_ref()
                .map_err(Clone::clone)?;
        }
        if let Some(fog) = crate::fog::for_color(options.fog, camera, options.surface) {
            admit(
                "World/water fog",
                fog.color_for_camera(camera).map(f64::from),
            )?;
        }
        Ok(())
    }
}

impl Bounds {
    fn from_scene(mesh: &SceneMesh, library: &MaterialLibrary) -> Result<Self, String> {
        let faces = face_bounds(mesh);
        let materials = material_bounds(mesh);
        let mut batches = Vec::new();
        batches
            .try_reserve_exact(mesh.batches.len())
            .map_err(|error| format!("HDR source bounds allocation failed: {error}"))?;
        for (index, batch) in mesh.batches.iter().enumerate() {
            let baked = mesh.baked_face_batches.contains(&index);
            let info = (!baked).then(|| library.get(batch.material));
            let vertices = batch_vertices(mesh, batch);
            let authored = vertices.as_ref().is_ok_and(|indices| {
                indices.iter().any(|&index| {
                    mesh.vertices[index as usize].lighting_receiver == AUTHORED_RECEIVER
                })
            });
            let radiance = if baked {
                baked_bound(mesh, batch, &faces, &materials)
            } else {
                ordinary_bound(mesh, batch)
            };
            batches.push(BatchBounds {
                radiance,
                authored,
                reference: info.is_some_and(|info| info.reference),
                rope: batch.rope,
                tool: info.is_some_and(|info| info.tool),
                sky: info.is_some_and(|info| info.sky),
                sky3d_eligible: info.is_some_and(|info| info.sky3d_eligible),
                helper: !baked && batch.material == MaterialLibrary::EDITOR,
            });
        }
        Ok(Self {
            batches,
            compiled_details: mesh.validate_compiled_detail_hdr(),
            authored: authored_bound(&mesh.lighting),
            authored_active: mesh.lighting.report.active,
            sky: mesh
                .skybox
                .as_ref()
                .map(|sky| (sky.channel, sky_bound(sky))),
            miniature: mesh.sky3d.as_ref().map(|scene| miniature_bound(scene)),
        })
    }
}

fn admit(producer: &str, bound: [f64; 3]) -> Result<(), String> {
    if bound
        .into_iter()
        .any(|value| !value.is_finite() || !(0.0..=MAX_HDR_RADIANCE).contains(&value))
    {
        return Err(format!(
            "{producer} exceeds finite RGBA16Float scene radiance 0..{MAX_HDR_RADIANCE}: {bound:?}"
        ));
    }
    Ok(())
}

fn batch_vertices<'a>(mesh: &'a SceneMesh, batch: &Batch) -> Result<&'a [u32], String> {
    let end = batch
        .first_index
        .checked_add(batch.index_count)
        .ok_or("HDR source batch index range overflowed")?;
    let indices = mesh
        .indices
        .get(batch.first_index as usize..end as usize)
        .ok_or("HDR source batch index range is unavailable")?;
    if indices
        .iter()
        .any(|&index| index as usize >= mesh.vertices.len())
    {
        return Err("HDR source batch vertex is unavailable".into());
    }
    Ok(indices)
}

fn ordinary_bound(mesh: &SceneMesh, batch: &Batch) -> Result<[f64; 3], String> {
    let mut bound = [1.0_f64; 3];
    for &index in batch_vertices(mesh, batch)? {
        let vertex = &mesh.vertices[index as usize];
        validate_vertex(vertex)?;
        match vertex.lighting_receiver & LIGHTING_RECEIVER_KIND_MASK {
            0..=2 => {}
            BAKED_RADIANCE_RECEIVER => {
                let ordinal = vertex.lighting_receiver >> LIGHTING_RECEIVER_KIND_BITS;
                let table = mesh.compiled_detail_hdr.as_ref().ok_or_else(|| {
                    format!("Detail record {ordinal} has no retained raw HDR radiance table")
                })?;
                let raw = table.max_drawn(&[ordinal])?;
                for channel in 0..3 {
                    bound[channel] = bound[channel].max(raw[channel] * COMPOSITION_HEADROOM);
                }
            }
            _ => return Err("HDR ordinary batch has an unqualified lighting receiver".into()),
        }
    }
    Ok(bound)
}

fn validate_vertex(vertex: &Vertex) -> Result<(), String> {
    let normal = glam::Vec3::from_array(vertex.normal);
    let position = glam::Vec3::from_array(vertex.position);
    if !vertex.opacity.is_finite()
        || !(0.0..=1.0).contains(&vertex.opacity)
        || !position.is_finite()
        || position.abs().max_element() > map_core::checks::SOURCE_COORDINATE_LIMIT as f32
        || !normal.is_finite()
        || !normal.length_squared().is_finite()
        || normal.length_squared() == 0.0
    {
        return Err("HDR source has invalid opacity, position or shading normal".into());
    }
    Ok(())
}

fn face_bounds(mesh: &SceneMesh) -> Result<Vec<FaceBounds>, String> {
    let mut result = Vec::new();
    let Some(lighting) = &mesh.baked_faces else {
        return Ok(result);
    };
    result
        .try_reserve_exact(lighting.faces.len())
        .map_err(|error| format!("HDR face bounds allocation failed: {error}"))?;
    for face in &lighting.faces {
        let mut maps = [[0.0_f64; 3]; 4];
        if !matches!(face.map_count, 0 | 1 | 4) {
            return Err("HDR baked face has an unqualified lightmap count".into());
        }
        let count = (face.width as usize)
            .checked_mul(face.height as usize)
            .ok_or("HDR face sample count overflowed")?;
        if face.map_count != 0 && count == 0 {
            return Err("HDR lit face has no retained lighting samples".into());
        }
        for (maxima, offset) in maps
            .iter_mut()
            .zip(face.map_offsets.iter())
            .take(face.map_count as usize)
        {
            let start = *offset as usize;
            let end = start
                .checked_add(count)
                .ok_or("HDR face sample range overflowed")?;
            let samples = lighting
                .samples
                .get(start..end)
                .ok_or("HDR face samples are incomplete")?;
            for sample in samples {
                for (maximum, sample) in maxima.iter_mut().zip(sample.iter()) {
                    let value = f64::from(*sample);
                    nonnegative("Baked face sample", value)?;
                    *maximum = maximum.max(value);
                }
            }
        }
        result.push(FaceBounds {
            maps,
            count: face.map_count,
        });
    }
    Ok(result)
}

fn material_bounds(mesh: &SceneMesh) -> Result<Vec<Option<MaterialBounds>>, String> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(mesh.baked_face_materials.len())
        .map_err(|error| format!("HDR material bounds allocation failed: {error}"))?;
    for material in &mesh.baked_face_materials {
        result.push(
            material
                .as_ref()
                .map(|material| material_bound(material))
                .transpose()?,
        );
    }
    Ok(result)
}

fn material_bound(material: &Material) -> Result<MaterialBounds, String> {
    let mut diffuse = texture_bound(material.diffuse[0].as_deref(), true)?;
    if material.kind != Kind::Single {
        let second = texture_bound(material.diffuse[1].as_deref(), true)?;
        for channel in 0..3 {
            diffuse[channel] = diffuse[channel].max(second[channel]);
        }
    }
    let tint = material.color.map(f64::from);
    for value in tint {
        nonnegative("Compiled material tint", value)?;
    }
    let ssbump = if material.normals[0]
        .as_ref()
        .is_some_and(|normal| normal.encoding == NormalEncoding::SsBump)
    {
        let mut weights = texture_bound(
            material.normals[0]
                .as_ref()
                .map(|normal| normal.texture.as_ref()),
            false,
        )?;
        if material.kind == Kind::VertexTransition && material.normals[1].is_some() {
            let second = texture_bound(
                material.normals[1]
                    .as_ref()
                    .map(|normal| normal.texture.as_ref()),
                false,
            )?;
            for channel in 0..3 {
                weights[channel] = weights[channel].max(second[channel]);
            }
        }
        Some(weights)
    } else {
        None
    };
    Ok(MaterialBounds {
        diffuse,
        tint,
        ssbump,
    })
}

fn texture_bound(texture: Option<&Texture>, alpha: bool) -> Result<[f64; 3], String> {
    let Some(texture) = texture else {
        return Ok([1.0; 3]);
    };
    let count = (texture.width as usize)
        .checked_mul(texture.height as usize)
        .ok_or("HDR material texture dimensions overflowed")?;
    if count == 0 || count != texture.pixels.len() {
        return Err("HDR material texture pixels are incomplete".into());
    }
    let mut bound = [0.0_f64; 3];
    for pixel in &texture.pixels {
        if !pixel[3].is_finite() || alpha && !(0.0..=1.0).contains(&pixel[3]) {
            return Err("HDR material texture alpha is outside 0..1".into());
        }
        for channel in 0..3 {
            let value = f64::from(pixel[channel]);
            nonnegative("Compiled material texture", value)?;
            bound[channel] = bound[channel].max(value);
        }
    }
    Ok(bound)
}

fn baked_bound(
    mesh: &SceneMesh,
    batch: &Batch,
    faces: &Result<Vec<FaceBounds>, String>,
    materials: &Result<Vec<Option<MaterialBounds>>, String>,
) -> Result<[f64; 3], String> {
    let material = mesh
        .baked_face_materials
        .get(batch.material)
        .and_then(Option::as_ref)
        .ok_or("HDR compiled material is unavailable")?;
    let bounds = materials
        .as_ref()
        .map_err(Clone::clone)?
        .get(batch.material)
        .and_then(Option::as_ref)
        .ok_or("HDR compiled material bounds are unavailable")?;
    let indices = batch_vertices(mesh, batch)?;
    let mut vertex_tint = [0.0_f64; 3];
    let mut prop_light = [0.0_f64; 3];
    for &index in indices {
        let vertex = &mesh.vertices[index as usize];
        validate_vertex(vertex)?;
        for channel in 0..3 {
            vertex_tint[channel] =
                vertex_tint[channel].max(f64::from(vertex.color[channel]) * BYTE_NORMALIZATION);
            if material.lighting == MaterialLighting::Vertex {
                let value = f64::from(vertex.baked_irradiance[channel]);
                nonnegative("Compiled prop irradiance", value)?;
                prop_light[channel] = prop_light[channel].max(value);
            }
        }
    }
    let mut result = [0.0_f64; 3];
    for &index in indices {
        let vertex = &mesh.vertices[index as usize];
        let (light, scale, weight, final_light) = match material.lighting {
            MaterialLighting::Unlit => ([1.0; 3], [1.0; 3], 1.0, [1.0; 3]),
            MaterialLighting::Vertex => (prop_light, [1.0; 3], 1.0, prop_light),
            MaterialLighting::Lightmap { .. } => {
                let ordinal = (vertex.lighting_receiver >> LIGHTING_RECEIVER_KIND_BITS) as usize;
                let face = faces
                    .as_ref()
                    .map_err(Clone::clone)?
                    .get(ordinal)
                    .ok_or("HDR baked face bounds are unavailable")?;
                if face.count == 1 {
                    (face.maps[0], [1.0; 3], 1.0, face.maps[0])
                } else if face.count == 4 {
                    let scale = std::array::from_fn(|channel| {
                        face.maps[1][channel]
                            .max(face.maps[2][channel])
                            .max(face.maps[3][channel])
                    });
                    let weight = bounds
                        .ssbump
                        .map_or(1.0, |weights| weights.into_iter().fold(0.0_f64, f64::max));
                    let final_light = std::array::from_fn(|channel| {
                        bounds.ssbump.map_or(scale[channel], |weights| {
                            (0..3)
                                .map(|map| face.maps[map + 1][channel] * weights[map])
                                .sum()
                        })
                    });
                    ([3.0; 3], scale, weight, final_light)
                } else {
                    return Err("HDR lit face has no qualified lighting maps".into());
                }
            }
        };
        for channel in 0..3 {
            let tint = vertex_tint[channel] * bounds.tint[channel];
            let diffuse_tint = bounds.diffuse[channel] * tint;
            let prefix = diffuse_tint * light[channel];
            let scaled = prefix * scale[channel];
            for value in [tint, diffuse_tint, prefix, scaled, scaled * weight] {
                if !value.is_finite() || value * COMPOSITION_HEADROOM > f64::from(f32::MAX) {
                    return Err(
                        "HDR compiled radiance overflows a floating-point shader intermediate"
                            .into(),
                    );
                }
            }
            result[channel] =
                result[channel].max(diffuse_tint * final_light[channel] * COMPOSITION_HEADROOM);
        }
    }
    Ok(result)
}

fn nonnegative(producer: &str, value: f64) -> Result<(), String> {
    if !value.is_finite() || value < 0.0 {
        return Err(format!(
            "{producer} has nonfinite or negative linear radiance"
        ));
    }
    Ok(())
}

fn authored_bound(preview: &Preview) -> Result<[f64; 3], String> {
    if preview.lights.len() > crate::lighting::MAX_LIGHTS {
        return Err("HDR authored lighting exceeds the admitted light count".into());
    }
    let mut bound = [0.0_f64; 3];
    for light in &preview.lights {
        let contribution = light_bound(light)?;
        for channel in 0..3 {
            bound[channel] += contribution[channel];
        }
    }
    Ok(bound.map(|value| value * BYTE_NORMALIZATION * COMPOSITION_HEADROOM))
}

fn light_bound(light: &Light) -> Result<[f64; 3], String> {
    if light
        .origin_radius
        .iter()
        .chain(&light.intensity_cap)
        .chain(&light.coefficients_spot)
        .chain(&light.direction_inner)
        .chain(&light.outer_exponent_fade)
        .any(|value| !value.is_finite())
    {
        return Err("HDR authored light contains nonfinite parameters".into());
    }
    let intensity = [
        light.intensity_cap[0],
        light.intensity_cap[1],
        light.intensity_cap[2],
    ]
    .map(f64::from);
    for value in intensity {
        nonnegative("Authored light", value)?;
    }
    if light.coefficients_spot[3] != crate::lighting::POINT_KIND {
        let direction = glam::Vec3::new(
            light.direction_inner[0],
            light.direction_inner[1],
            light.direction_inner[2],
        );
        if direction.length_squared() == 0.0 || !direction.length_squared().is_finite() {
            return Err("HDR authored light has an unqualified direction".into());
        }
    }
    if light.coefficients_spot[3] == crate::lighting::SUN_KIND {
        let ambient = light.origin_radius[..3].iter().copied().map(f64::from);
        let mut result = intensity;
        for (channel, ambient) in ambient.enumerate() {
            nonnegative("Authored sky ambient", ambient)?;
            result[channel] += ambient;
        }
        return Ok(result);
    }
    if ![crate::lighting::POINT_KIND, crate::lighting::SPOT_KIND]
        .contains(&light.coefficients_spot[3])
    {
        return Err("HDR authored light has an unqualified light kind".into());
    }
    let maximum_distance = 2.0 * map_core::checks::SOURCE_COORDINATE_LIMIT * 3.0_f64.sqrt();
    let radius = f64::from(light.origin_radius[3]);
    let cap = f64::from(light.intensity_cap[3]);
    let fade_end = f64::from(light.outer_exponent_fade[3]);
    if radius < 0.0 || cap < 0.0 || fade_end < 0.0 {
        return Err("HDR authored light has unqualified attenuation limits".into());
    }
    if light.origin_radius[..3]
        .iter()
        .any(|value| f64::from(*value).abs() > map_core::checks::SOURCE_COORDINATE_LIMIT)
        || fade_end > 0.0 && fade_end <= f64::from(light.outer_exponent_fade[2])
        || light.coefficients_spot[3] == crate::lighting::SPOT_KIND
            && (light.outer_exponent_fade[1] < 0.0
                || !(-1.0..=1.0).contains(&light.outer_exponent_fade[0])
                || !(-1.0..=1.0).contains(&light.direction_inner[3])
                || light.direction_inner[3] < light.outer_exponent_fade[0])
    {
        return Err("HDR authored light has unqualified position, fade or cone parameters".into());
    }
    let mut high = maximum_distance;
    if radius > 0.0 {
        high = high.min(radius);
    }
    if fade_end > 0.0 {
        high = high.min(fade_end);
    }
    high = high.max(1.0);
    let mut low = 1.0_f64;
    if cap > 0.0 {
        low = low.min(cap);
        high = high.min(cap);
    }
    let [constant, linear, quadratic] = [
        light.coefficients_spot[0],
        light.coefficients_spot[1],
        light.coefficients_spot[2],
    ]
    .map(f64::from);
    let stationary = if quadratic > 0.0 {
        (-linear / (2.0 * quadratic)).clamp(low, high)
    } else {
        low
    };
    let mut minimum = f64::INFINITY;
    for distance in [low, high, stationary] {
        let terms = [constant, distance * linear, distance * distance * quadratic];
        let divisor = terms.into_iter().sum::<f64>();
        let uncertainty = terms.into_iter().map(f64::abs).sum::<f64>() * FLOAT_UNCERTAINTY;
        if !divisor.is_finite() || divisor <= uncertainty {
            return Err(
                "HDR authored light cannot qualify a finite positive attenuation divisor".into(),
            );
        }
        minimum = minimum.min(divisor - uncertainty);
    }
    if minimum.recip() * COMPOSITION_HEADROOM > f64::from(f32::MAX) {
        return Err(
            "HDR authored attenuation can overflow a floating-point shader intermediate".into(),
        );
    }
    Ok(intensity.map(|value| value / minimum))
}

fn sky_bound(sky: &crate::sky::Skybox) -> Result<[f64; 3], String> {
    if !sky.ready() {
        return Err("HDR scene sky assets are unavailable".into());
    }
    if sky.channel == crate::sky::Channel::Ldr {
        return Ok([1.0; 3]);
    }
    let mut bound = [0.0_f64; 3];
    for face in &sky.faces {
        let texture = face.image().ok_or("HDR sky face is unavailable")?;
        crate::sky::validate_hdr_texture(texture)?;
        if texture.mip_count == 0 || texture.width == 0 || texture.height == 0 {
            return Err("HDR sky face has no retained image data".into());
        }
        for mip in 0..texture.mip_count {
            let bytes = texture
                .image_data(mip, 0, 0)
                .map_err(|error| error.to_string())?;
            for pixel in bytes.as_chunks::<8>().0 {
                for channel in 0..3 {
                    let bits = u16::from_le_bytes([pixel[channel * 2], pixel[channel * 2 + 1]]);
                    let value = if texture.format == source_formats::vtf::ImageFormat::Rgba16161616F
                    {
                        half::f16::from_bits(bits).to_f32()
                    } else {
                        half::f16::from_f32(f32::from(bits) / 65535.0 * 16.0).to_f32()
                    };
                    let value = f64::from(value);
                    nonnegative("HDR sky texture", value)?;
                    bound[channel] = bound[channel].max(value);
                }
            }
        }
    }
    Ok(bound)
}

fn miniature_bound(scene: &crate::sky3d::Scene) -> Result<(), String> {
    scene.camera.validate()?;
    scene.fog.validate()?;
    for vertex in &scene.vertices {
        validate_vertex(vertex)?;
    }
    Ok(())
}
