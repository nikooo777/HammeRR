use std::{io, ops::Range, sync::Arc};

use glam::{DVec3, Vec3};
use source_formats::{
    bsp::StaticProp,
    model::{HardwareLayout, Model},
    vhv::{self, Bgra8},
};

use super::{
    BAKED_PROP_RECEIVER, LightingSettings, MAX_CPU_BYTES, MAX_GPU_BYTES, bytes,
    materials::{Material, MaterialLighting},
    sum_bytes,
};
use crate::{
    LightingChannel, SceneMesh,
    scene::{Batch, LineVertex, Vertex},
};

pub const POLICY: &str = "Final native static-prop ordinals, selected skins and poses; exact ordered MDL/VVD/DX90-VTX hardware occurrences and independently baked VHV channels. RGB bytes decode per vertex as pow(2*byte/255, f64::from(2.2_f32)) before interpolation. VHV alpha is retained data, not opacity. Baked-static weight is unity with no runtime-style decomposition; shared EV follows compiled diffuse and material coverage, then Reinhard and one sRGB output. Runtime point/spot style contributions are not simulated. No native adaptation or appearance parity.";
const MAX_PROPS: usize = 1024;
const MAX_SECTIONS: usize = 256;
const MAX_NAME_BYTES: usize = 1024;
const WIRE_COLOR: [u8; 4] = [151, 169, 191, 255];

#[derive(Debug, serde::Serialize)]
pub struct PropInfo {
    pub ordinal: u32,
    pub model_name: String,
    pub skin: i32,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub lods: Vec<u8>,
    pub vertices: usize,
    pub triangles: usize,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
}

pub struct Input<'a> {
    pub ordinal: u32,
    pub model_name: &'a str,
    pub prop: &'a StaticProp,
    pub model: &'a Model,
    pub layout: &'a HardwareLayout,
    pub materials: &'a [usize],
    pub channels: [Option<&'a [u8]>; 2],
}

#[derive(Debug, serde::Serialize)]
pub struct VertexSample {
    pub bgra: [u8; 4],
    pub decoded_linear: [f64; 3],
    pub pre_ev: [f64; 3],
    pub exposed: [f64; 3],
}

#[derive(Debug)]
struct Section {
    prop: usize,
    lod: u8,
    material: usize,
    vertices: Range<usize>,
    indices: Range<usize>,
    mins: Vec3,
    maxs: Vec3,
}

#[derive(Debug)]
pub struct Scene {
    props: Vec<PropInfo>,
    sections: Vec<Section>,
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    colors: [Option<Vec<Bgra8>>; 2],
    retained_bytes: usize,
}

impl Scene {
    pub fn new(channels: Vec<LightingChannel>) -> Result<Self, String> {
        if channels.is_empty() || channels.len() > 2 {
            return Err("Compiled props require one or two exact VHV channels".into());
        }
        let mut colors = [None, None];
        for channel in channels {
            if colors[index(channel)].is_some() {
                return Err("Compiled prop lighting channels must be distinct".into());
            }
            colors[index(channel)] = Some(Vec::new());
        }
        Ok(Self {
            props: Vec::new(),
            sections: Vec::new(),
            vertices: Vec::new(),
            indices: Vec::new(),
            colors,
            retained_bytes: size_of::<Self>(),
        })
    }

    pub fn push(
        &mut self,
        input: Input<'_>,
        maximum_bytes: usize,
        check: &mut dyn FnMut() -> io::Result<()>,
    ) -> Result<(), String> {
        check().map_err(message)?;
        if self.props.len() >= MAX_PROPS
            || self.props.iter().any(|prop| prop.ordinal == input.ordinal)
            || input.model_name.is_empty()
            || input.model_name.len() > MAX_NAME_BYTES
            || input.layout.sections.is_empty()
            || input.layout.sections.len() > MAX_SECTIONS
            || input.layout.root_lod != 0
            || input.layout.lods == 0
            || input.layout.lods > vhv::MAX_LODS as usize
            || input.model.body_groups.len() != 1
            || input.model.body_groups[0].variants.len() != 1
            || input.prop.dx_levels != [0; 2]
            || input.layout.checksum != input.model.header.checksum
            || !input.model.header.is_static_prop()
            || input.materials.len() != input.layout.sections.len()
        {
            return Err("Compiled prop identity or admitted hardware layout is invalid".into());
        }
        let skin = usize::try_from(input.prop.skin)
            .ok()
            .and_then(|skin| input.model.skins.get(skin))
            .ok_or("Compiled prop selected skin is unavailable")?;
        let transform = input.prop.transform().map_err(message)?;
        let views = input.channels.map(|data| {
            data.map(|data| vhv::View::parse(data, vhv::Limits::default()).map_err(message))
                .transpose()
        });
        let [ldr, hdr] = views;
        let views = [ldr?, hdr?];
        let mut vertex_count = 0usize;
        let mut index_count = 0usize;
        let mut lods = [false; vhv::MAX_LODS as usize];
        for (section_index, section) in input.layout.sections.iter().enumerate() {
            check().map_err(message)?;
            let roots = input
                .layout
                .vertices
                .get(section.vertices.clone())
                .filter(|roots| !roots.is_empty())
                .ok_or("Compiled prop hardware vertex range is invalid")?;
            let part = input
                .model
                .parts
                .get(section.part)
                .ok_or("Compiled prop hardware material part is unavailable")?;
            let selected = skin
                .get(part.material)
                .and_then(|&material| input.model.materials.get(material));
            if selected.is_none()
                || section.body_part != 0
                || section.model != 0
                || section.lod >= input.layout.lods
                || !section.triangle_indices.len().is_multiple_of(3)
                || section
                    .triangle_indices
                    .iter()
                    .any(|&vertex| vertex as usize >= roots.len())
            {
                return Err(
                    "Compiled prop local topology, LOD or selected material is invalid".into(),
                );
            }
            lods[section.lod] = true;
            vertex_count = vertex_count
                .checked_add(roots.len())
                .ok_or("Compiled prop vertex count overflow")?;
            index_count = index_count
                .checked_add(section.triangle_indices.len())
                .ok_or("Compiled prop index count overflow")?;
            for (local, &root) in roots.iter().enumerate() {
                if local.is_multiple_of(1024) {
                    check().map_err(message)?;
                }
                let vertex = input
                    .model
                    .vertices
                    .get(root as usize)
                    .ok_or("Compiled prop root vertex is unavailable")?;
                transformed(vertex, transform)?;
            }
            for (channel, view) in views.iter().enumerate() {
                if self.colors[channel].is_some() != view.is_some() {
                    return Err(
                        "Compiled prop VHV channels differ from the retained receiver set".into(),
                    );
                }
                if let Some(view) = view {
                    let metadata = view.metadata();
                    let colors = view.section(section_index).map_err(message)?;
                    if metadata.checksum != input.layout.checksum as u32
                        || metadata.mesh_count != input.layout.sections.len()
                        || colors.layout.lod as usize != section.lod
                        || colors.layout.vertex_count as usize != roots.len()
                    {
                        return Err(
                            "Compiled prop VHV descriptors differ from the exact hardware layout"
                                .into(),
                        );
                    }
                }
            }
        }
        if lods[..input.layout.lods].iter().any(|present| !present)
            || views
                .iter()
                .flatten()
                .any(|view| view.metadata().vertex_count != vertex_count)
        {
            return Err("Compiled prop all-LOD hardware or VHV vertex count is incomplete".into());
        }
        let new_vertices = self
            .vertices
            .len()
            .checked_add(vertex_count)
            .filter(|&count| count <= u32::MAX as usize)
            .ok_or("Compiled prop combined vertex range overflow")?;
        let added = sum_bytes(&[
            size_of::<PropInfo>(),
            input.model_name.len(),
            input.layout.lods,
            bytes(input.layout.sections.len(), size_of::<Section>())?,
            bytes(vertex_count, size_of::<Vertex>())?,
            bytes(index_count, size_of::<u32>())?,
            bytes(
                vertex_count,
                size_of::<Bgra8>() * views.iter().flatten().count(),
            )?,
        ])?;
        let retained = sum_bytes(&[self.retained_bytes, added])?;
        if retained > maximum_bytes.min(MAX_CPU_BYTES) {
            return Err(
                "Compiled prop retained geometry and VHV exceed the combined CPU working budget"
                    .into(),
            );
        }
        reserve(&mut self.props, 1)?;
        reserve(&mut self.sections, input.layout.sections.len())?;
        reserve(&mut self.vertices, vertex_count)?;
        reserve(&mut self.indices, index_count)?;
        for colors in self.colors.iter_mut().flatten() {
            reserve(colors, vertex_count)?;
        }
        let mut model_name = String::new();
        model_name
            .try_reserve_exact(input.model_name.len())
            .map_err(|_| "Compiled prop model name allocation failed")?;
        model_name.push_str(input.model_name);
        let mut available_lods = Vec::new();
        reserve(&mut available_lods, input.layout.lods)?;
        available_lods.extend((0..input.layout.lods).map(|lod| lod as u8));
        let mut mins = Vec3::splat(f32::INFINITY);
        let mut maxs = Vec3::splat(f32::NEG_INFINITY);
        for (section_index, section) in input.layout.sections.iter().enumerate() {
            let start = self.vertices.len();
            let mut section_mins = Vec3::splat(f32::INFINITY);
            let mut section_maxs = Vec3::splat(f32::NEG_INFINITY);
            for &root in &input.layout.vertices[section.vertices.clone()] {
                let vertex = transformed(&input.model.vertices[root as usize], transform)?;
                let point = Vec3::from_array(vertex.position);
                section_mins = section_mins.min(point);
                section_maxs = section_maxs.max(point);
                self.vertices.push(vertex);
            }
            let first_index = self.indices.len();
            self.indices.extend(
                section
                    .triangle_indices
                    .iter()
                    .map(|&local| start as u32 + local),
            );
            for (channel, view) in views.iter().enumerate() {
                if let Some(view) = view {
                    let colors = view.section(section_index).map_err(message)?;
                    self.colors[channel].as_mut().unwrap().extend_from_slice(
                        colors
                            .page(0, colors.layout.vertex_count as usize)
                            .map_err(message)?,
                    );
                }
            }
            self.sections.push(Section {
                prop: self.props.len(),
                lod: section.lod as u8,
                material: input.materials[section_index],
                vertices: start..self.vertices.len(),
                indices: first_index..self.indices.len(),
                mins: section_mins,
                maxs: section_maxs,
            });
            mins = mins.min(section_mins);
            maxs = maxs.max(section_maxs);
        }
        debug_assert_eq!(self.vertices.len(), new_vertices);
        self.props.push(PropInfo {
            ordinal: input.ordinal,
            model_name,
            skin: input.prop.skin,
            origin: input.prop.origin,
            angles: input.prop.angles,
            lods: available_lods,
            vertices: vertex_count,
            triangles: index_count / 3,
            mins: mins.to_array(),
            maxs: maxs.to_array(),
        });
        self.retained_bytes = retained;
        check().map_err(message)
    }

    pub fn props(&self) -> &[PropInfo] {
        &self.props
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn available_channels(&self) -> Vec<LightingChannel> {
        [LightingChannel::Ldr, LightingChannel::Hdr]
            .into_iter()
            .filter(|&channel| self.colors[index(channel)].is_some())
            .collect()
    }

    pub fn validate_settings(
        &self,
        settings: &LightingSettings,
        draw_lod: u8,
    ) -> Result<(), String> {
        settings.validate()?;
        self.channel(settings.channel)?;
        for prop in &self.props {
            if !prop.lods.contains(&draw_lod) {
                return Err(format!(
                    "Compiled prop {} has no requested draw LOD {draw_lod}",
                    prop.ordinal
                ));
            }
        }
        Ok(())
    }

    pub fn bounds(
        &self,
        draw_lod: u8,
        ordinal: Option<u32>,
        include: impl Fn(&PropInfo) -> bool,
    ) -> Result<Option<[[f32; 3]; 2]>, String> {
        if draw_lod >= vhv::MAX_LODS as u8
            || ordinal.is_some_and(|ordinal| !self.props.iter().any(|prop| prop.ordinal == ordinal))
        {
            return Err("Compiled prop ordinal or requested draw LOD is unavailable".into());
        }
        let mut bounds: Option<(Vec3, Vec3)> = None;
        for prop in &self.props {
            if ordinal.is_some_and(|ordinal| prop.ordinal != ordinal) || !include(prop) {
                continue;
            }
            if !prop.lods.contains(&draw_lod) {
                return Err(format!(
                    "Compiled prop {} has no requested draw LOD {draw_lod}",
                    prop.ordinal
                ));
            }
        }
        for section in &self.sections {
            let prop = &self.props[section.prop];
            if section.lod != draw_lod
                || ordinal.is_some_and(|ordinal| prop.ordinal != ordinal)
                || !include(prop)
            {
                continue;
            }
            bounds = Some(bounds.map_or((section.mins, section.maxs), |(mins, maxs)| {
                (mins.min(section.mins), maxs.max(section.maxs))
            }));
        }
        Ok(bounds.map(|(mins, maxs)| [mins.to_array(), maxs.to_array()]))
    }

    pub fn vertex_sample(
        &self,
        ordinal: u32,
        section: usize,
        vertex: usize,
        settings: &LightingSettings,
    ) -> Result<VertexSample, String> {
        settings.validate()?;
        let prop = self
            .props
            .iter()
            .position(|prop| prop.ordinal == ordinal)
            .ok_or("Compiled prop ordinal is unavailable")?;
        let section = self
            .sections
            .iter()
            .filter(|section| section.prop == prop)
            .nth(section)
            .ok_or("Compiled prop section is unavailable")?;
        if vertex >= section.vertices.len() {
            return Err("Compiled prop local hardware vertex is unavailable".into());
        }
        let color = self.channel(settings.channel)?[section.vertices.start + vertex];
        let decoded = color.linear_rgb();
        Ok(VertexSample {
            bgra: [color.blue, color.green, color.red, color.alpha],
            decoded_linear: decoded,
            pre_ev: decoded,
            exposed: decoded.map(|value| value * settings.exposure_ev.exp2()),
        })
    }

    pub fn draw_lod_sample(
        &self,
        ordinal: u32,
        draw_lod: u8,
        settings: &LightingSettings,
    ) -> Result<VertexSample, String> {
        self.validate_settings(settings, draw_lod)?;
        let prop = self
            .props
            .iter()
            .position(|prop| prop.ordinal == ordinal)
            .ok_or("Compiled prop ordinal is unavailable")?;
        let section = self
            .sections
            .iter()
            .filter(|section| section.prop == prop)
            .position(|section| section.lod == draw_lod && !section.vertices.is_empty())
            .ok_or("Compiled prop requested draw LOD has no hardware vertices")?;
        self.vertex_sample(ordinal, section, 0, settings)
    }

    pub fn mesh_bytes(&self, channel: LightingChannel, draw_lod: u8) -> Result<usize, String> {
        self.channel(channel)?;
        let (vertices, indices, parts) = self.counts(draw_lod)?;
        sum_bytes(&[
            bytes(vertices, size_of::<Vertex>())?,
            bytes(indices, size_of::<u32>())?,
            bytes(parts, size_of::<Batch>())?,
            bytes(indices, 2 * size_of::<LineVertex>())?,
        ])
    }

    pub fn append_prepared_filtered(
        &self,
        mesh: &mut SceneMesh,
        materials: &[Option<Arc<Material>>],
        settings: &LightingSettings,
        draw_lod: u8,
        extra_cpu_bytes: usize,
        include: impl Fn(&PropInfo) -> bool,
    ) -> Result<(), String> {
        self.validate_settings(settings, draw_lod)?;
        let lighting = mesh
            .baked_faces
            .as_ref()
            .ok_or("Compiled props require the shared baked surface lighting layer")?;
        if lighting.channel != settings.channel
            || lighting.exposure_ev != settings.exposure_ev as f32
            || mesh.baked_face_batches.end != mesh.batches.len()
        {
            return Err(
                "Compiled props require matching shared channel/EV and contiguous baked batches"
                    .into(),
            );
        }
        if self.sections.iter().any(|section| {
            materials
                .get(section.material)
                .and_then(Option::as_ref)
                .is_none_or(|material| material.lighting != MaterialLighting::Vertex)
        }) {
            return Err("Compiled props require explicitly admitted vertex-lit materials".into());
        }
        let (vertices, indices, parts) = self.counts(draw_lod)?;
        let vertex_count = mesh
            .vertices
            .len()
            .checked_add(vertices)
            .filter(|&count| count <= u32::MAX as usize)
            .ok_or("Compiled prop scene vertex range overflow")?;
        let index_count = mesh
            .indices
            .len()
            .checked_add(indices)
            .filter(|&count| count <= u32::MAX as usize)
            .ok_or("Compiled prop scene index range overflow")?;
        let line_count = [
            mesh.lines.len(),
            mesh.reference_lines.len(),
            mesh.leak_lines.len(),
            mesh.wire_lines.len(),
            mesh.tool_wire_lines.len(),
            mesh.rope_lines.len(),
            indices
                .checked_mul(2)
                .ok_or("Compiled prop wire range overflow")?,
        ]
        .into_iter()
        .try_fold(0usize, usize::checked_add)
        .ok_or("Compiled prop scene line range overflow")?;
        let mesh_bytes = sum_bytes(&[
            bytes(vertex_count, size_of::<Vertex>())?,
            bytes(index_count, size_of::<u32>())?,
            bytes(
                mesh.batches
                    .len()
                    .checked_add(parts)
                    .ok_or("Compiled prop batch range overflow")?,
                size_of::<Batch>(),
            )?,
            bytes(line_count, size_of::<LineVertex>())?,
            bytes(mesh.icons.len(), size_of::<crate::icons::Instance>())?,
            bytes(
                mesh.detail_billboards.len(),
                size_of::<crate::compiled_details::Billboard>(),
            )?,
            bytes(
                mesh.detail_parts.len(),
                size_of::<crate::compiled_details::Part>(),
            )?,
            bytes(
                mesh.baked_face_materials
                    .len()
                    .checked_add(materials.len())
                    .ok_or("Compiled prop material range overflow")?,
                size_of::<Option<Arc<Material>>>(),
            )?,
        ])?;
        let material_cpu = materials
            .iter()
            .flatten()
            .try_fold(0usize, |total, material| {
                total.checked_add(material.decoded_bytes())
            })
            .ok_or("Compiled prop material bytes overflow")?;
        let material_gpu = mesh
            .baked_face_materials
            .iter()
            .chain(materials)
            .flatten()
            .try_fold(0usize, |total, material| {
                total.checked_add(material.gpu_bytes())
            })
            .ok_or("Compiled prop material GPU bytes overflow")?;
        if sum_bytes(&[
            self.retained_bytes,
            extra_cpu_bytes,
            material_cpu,
            mesh_bytes,
        ])? > MAX_CPU_BYTES
            || sum_bytes(&[lighting.byte_len()?, material_gpu, mesh_bytes])? > MAX_GPU_BYTES
        {
            return Err(
                "Compiled props and surfaces exceed the combined CPU or GPU working budget".into(),
            );
        }
        reserve(&mut mesh.vertices, vertices)?;
        reserve(&mut mesh.indices, indices)?;
        reserve(&mut mesh.batches, parts)?;
        reserve(&mut mesh.wire_lines, indices * 2)?;
        reserve(&mut mesh.baked_face_materials, materials.len())?;
        let material_base = mesh.baked_face_materials.len();
        let colors = self.channel(settings.channel)?;
        for section in &self.sections {
            if section.lod != draw_lod || !include(&self.props[section.prop]) {
                continue;
            }
            let vertex_base = mesh.vertices.len() as u32;
            for source in section.vertices.clone() {
                let mut vertex = self.vertices[source];
                vertex.baked_irradiance = colors[source].linear_rgb().map(|value| value as f32);
                mesh.vertices.push(vertex);
            }
            let first_index = mesh.indices.len() as u32;
            mesh.indices.extend(
                self.indices[section.indices.clone()]
                    .iter()
                    .map(|&vertex| vertex_base + (vertex - section.vertices.start as u32)),
            );
            for triangle in self.indices[section.indices.clone()].as_chunks::<3>().0 {
                for (a, b) in [
                    (triangle[0], triangle[1]),
                    (triangle[1], triangle[2]),
                    (triangle[2], triangle[0]),
                ] {
                    for vertex in [a, b] {
                        mesh.wire_lines.push(LineVertex {
                            position: self.vertices[vertex as usize].position,
                            color: WIRE_COLOR,
                        });
                    }
                }
            }
            if !section.indices.is_empty() {
                mesh.batches.push(Batch {
                    material: material_base + section.material,
                    first_index,
                    index_count: section.indices.len() as u32,
                    rope: false,
                    model_translucent: false,
                });
            }
            mesh.mins = mesh.mins.min(section.mins);
            mesh.maxs = mesh.maxs.max(section.maxs);
        }
        mesh.baked_face_materials.extend_from_slice(materials);
        mesh.baked_face_batches.end = mesh.batches.len();
        Ok(())
    }

    fn counts(&self, draw_lod: u8) -> Result<(usize, usize, usize), String> {
        if draw_lod >= vhv::MAX_LODS as u8
            || self.props.iter().any(|prop| !prop.lods.contains(&draw_lod))
        {
            return Err("Requested compiled prop draw LOD is unavailable".into());
        }
        self.sections
            .iter()
            .filter(|section| section.lod == draw_lod)
            .try_fold(
                (0usize, 0usize, 0usize),
                |(vertices, indices, parts), section| {
                    Ok((
                        vertices
                            .checked_add(section.vertices.len())
                            .ok_or("Compiled prop vertex count overflow")?,
                        indices
                            .checked_add(section.indices.len())
                            .ok_or("Compiled prop index count overflow")?,
                        parts
                            .checked_add(usize::from(!section.indices.is_empty()))
                            .ok_or("Compiled prop section count overflow")?,
                    ))
                },
            )
    }

    fn channel(&self, channel: LightingChannel) -> Result<&[Bgra8], String> {
        self.colors[index(channel)].as_deref().ok_or_else(|| {
            format!(
                "Compiled props have no exact {} VHV channel",
                channel.name()
            )
        })
    }
}

fn transformed(
    vertex: &source_formats::model::Vertex,
    transform: glam::DAffine3,
) -> Result<Vertex, String> {
    let position = transform.transform_point3(DVec3::from_array(vertex.position.map(f64::from)));
    let normal = transform
        .transform_vector3(DVec3::from_array(vertex.normal.map(f64::from)))
        .normalize_or_zero();
    if !position.is_finite()
        || position.abs().max_element() > map_core::checks::SOURCE_COORDINATE_LIMIT
        || !normal.is_finite()
        || normal.length_squared() == 0.0
        || vertex.uv.iter().any(|value| !value.is_finite())
    {
        return Err("Compiled prop transformed geometry exceeds finite Source bounds".into());
    }
    Ok(Vertex {
        position: position.as_vec3().to_array(),
        normal: normal.as_vec3().to_array(),
        uv: vertex.uv,
        color: [255; 4],
        lightmap: [0.0, 0.0, -1.0],
        smoothing: 0,
        opacity: 1.0,
        texture_alpha: 0,
        tangent: [0.0; 4],
        lighting_receiver: BAKED_PROP_RECEIVER,
        baked_irradiance: [0.0; 3],
    })
}

fn index(channel: LightingChannel) -> usize {
    match channel {
        LightingChannel::Ldr => 0,
        LightingChannel::Hdr => 1,
    }
}
fn message(error: io::Error) -> String {
    error.to_string()
}
fn reserve<T>(values: &mut Vec<T>, count: usize) -> Result<(), String> {
    values
        .try_reserve_exact(count)
        .map_err(|error| format!("Compiled prop allocation failed: {error}"))
}
