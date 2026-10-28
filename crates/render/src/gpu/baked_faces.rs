use std::collections::HashMap;
use std::mem::size_of;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use wgpu::util::DeviceExt;

use super::GpuContext;
use crate::compiled_faces::materials::{
    Kind, Material, NormalEncoding, Texture, validate_directional_inputs,
};
use crate::compiled_faces::{
    BAKED_FACE_RECEIVER, LightingData, MAX_EXPOSURE_EV, MAX_GPU_BYTES, MIN_EXPOSURE_EV,
};

const RETIRE_TIMEOUT: Duration = Duration::from_secs(1);
const PARAM_BYTES: usize = size_of::<[f32; 4]>();
const RECEIVER_SHIFT: u32 = 4;
const RECEIVER_MASK: u32 = (1 << RECEIVER_SHIFT) - 1;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Face {
    dimensions: [u32; 4],
    map_offsets: [u32; 4],
}

pub(super) struct Reservation {
    allocated: Arc<AtomicUsize>,
    bytes: usize,
}

impl Reservation {
    fn new(allocated: &Arc<AtomicUsize>, bytes: usize) -> Result<Self, String> {
        #[allow(deprecated, reason = "Atomic::try_update requires Rust 1.95")]
        allocated
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(bytes).filter(|total| *total <= MAX_GPU_BYTES)
            })
            .map_err(|_| {
                format!("Baked surface retained GPU payload exceeds {MAX_GPU_BYTES} bytes; release earlier scenes before replacement")
            })?;
        Ok(Self {
            allocated: allocated.clone(),
            bytes,
        })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.allocated.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

pub(super) struct Resources {
    source: Arc<LightingData>,
    pub lighting_binding: Option<wgpu::BindGroup>,
    pub unlit_binding: wgpu::BindGroup,
    _reservation: Reservation,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MaterialParams {
    options: [u32; 4],
    color: [f32; 4],
    diffuse_transform: [[[f32; 4]; 2]; 2],
    normal_transform: [[[f32; 4]; 2]; 2],
}

pub(super) struct MaterialResources {
    source: Arc<Material>,
    pub binding: wgpu::BindGroup,
    pub unlit: bool,
    _reservation: Reservation,
}

pub(super) struct State {
    pub layout: wgpu::BindGroupLayout,
    pub material_layout: wgpu::BindGroupLayout,
    pub unlit_layout: wgpu::BindGroupLayout,
    resources: HashMap<usize, Weak<Resources>>,
    materials: HashMap<usize, Weak<MaterialResources>>,
    geometries: Vec<Weak<Reservation>>,
    allocated: Arc<AtomicUsize>,
    uploaded: usize,
}

impl State {
    pub fn validate(&self, ctx: &GpuContext, mesh: &crate::scene::SceneMesh) -> Result<(), String> {
        let Some(source) = &mesh.baked_faces else {
            if !mesh.baked_face_batches.is_empty() || !mesh.baked_face_materials.is_empty() {
                return Err(
                    "Baked surface geometry or materials lost their lighting payload".into(),
                );
            }
            return Ok(());
        };
        let mut bytes = validate(ctx, source)?;
        if mesh.baked_face_batches.start > mesh.baked_face_batches.end
            || mesh.baked_face_batches.end > mesh.batches.len()
        {
            return Err("Baked surface geometry has invalid batch ranges".into());
        }
        for material in mesh.baked_face_materials.iter().flatten() {
            bytes = bytes
                .checked_add(validate_material(ctx, material)?)
                .ok_or("Baked surface aggregate GPU byte size overflowed")?;
        }
        let vertices = mesh
            .vertices
            .len()
            .checked_mul(size_of::<crate::scene::Vertex>())
            .ok_or("Baked surface vertex bytes overflowed")?;
        let indices = mesh
            .indices
            .len()
            .checked_mul(size_of::<u32>())
            .ok_or("Baked surface index bytes overflowed")?;
        let total = bytes
            .checked_add(geometry_bytes(mesh)?)
            .ok_or("Baked surface aggregate GPU byte size overflowed")?;
        if total > MAX_GPU_BYTES
            || vertices as u64 > ctx.device.limits().max_buffer_size
            || indices as u64 > ctx.device.limits().max_buffer_size
        {
            return Err("Baked surface aggregate geometry, lighting and material payload exceeds GPU limits".into());
        }
        for batch in &mesh.batches[mesh.baked_face_batches.clone()] {
            let material = mesh
                .baked_face_materials
                .get(batch.material)
                .and_then(Option::as_ref)
                .ok_or("Baked surface batch references an absent compiled material")?;
            let end = batch
                .first_index
                .checked_add(batch.index_count)
                .ok_or("Baked surface batch index range overflowed")?
                as usize;
            let indices = mesh
                .indices
                .get(batch.first_index as usize..end)
                .ok_or("Baked surface batch index range is outside geometry")?;
            if indices.is_empty() || !indices.len().is_multiple_of(3) {
                return Err("Baked surface batch requires nonempty triangle indices".into());
            }
            for triangle in indices.as_chunks::<3>().0 {
                let receivers = [triangle[0], triangle[1], triangle[2]].map(|index| {
                    mesh.vertices
                        .get(index as usize)
                        .map(|vertex| vertex.lighting_receiver)
                });
                if receivers[0].is_none()
                    || receivers[0] != receivers[1]
                    || receivers[0] != receivers[2]
                {
                    return Err(
                        "Baked surface triangle crosses distinct face lighting receivers".into(),
                    );
                }
            }
            for &index in indices {
                let vertex = mesh
                    .vertices
                    .get(index as usize)
                    .ok_or("Baked surface vertex index is outside geometry")?;
                let face = source
                    .faces
                    .get((vertex.lighting_receiver >> RECEIVER_SHIFT) as usize)
                    .ok_or("Baked surface vertex references an absent lightmap")?;
                if vertex.lighting_receiver & RECEIVER_MASK != BAKED_FACE_RECEIVER
                    || material.directional != (face.map_count == 4)
                    || material.unlit != (face.map_count == 0)
                    || vertex
                        .lightmap
                        .iter()
                        .chain(&vertex.uv)
                        .any(|value| !value.is_finite())
                    || !vertex.opacity.is_finite()
                    || !(0.0..=1.0).contains(&vertex.opacity)
                    || vertex
                        .position
                        .iter()
                        .chain(&vertex.normal)
                        .chain(&vertex.tangent)
                        .any(|value| !value.is_finite())
                    || face.map_count == 4 && vertex.tangent[3].abs() != 1.0
                {
                    return Err("Baked surface vertex has invalid receiver, tangent or directional material binding".into());
                }
            }
        }
        Ok(())
    }

    pub fn new(device: &wgpu::Device) -> Self {
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("baked surface lighting"),
            entries: &[
                storage(0),
                storage(1),
                super::uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("baked surface material"),
            entries: &[
                texture(0),
                texture(1),
                texture(4),
                texture(5),
                super::uniform_entry(6, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let unlit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("compiled unlit surface settings"),
            entries: &[super::uniform_entry(2, wgpu::ShaderStages::FRAGMENT)],
        });
        Self {
            layout,
            material_layout,
            unlit_layout,
            resources: HashMap::new(),
            materials: HashMap::new(),
            geometries: Vec::new(),
            allocated: Arc::new(AtomicUsize::new(0)),
            uploaded: 0,
        }
    }

    pub fn reserve_geometry(
        &mut self,
        ctx: &GpuContext,
        mesh: &crate::scene::SceneMesh,
    ) -> Result<Option<Arc<Reservation>>, String> {
        if mesh.baked_faces.is_none() {
            return Ok(None);
        }
        let bytes = geometry_bytes(mesh)?;
        self.prepare_upload(ctx, bytes)?;
        let reservation = Arc::new(Reservation::new(&self.allocated, bytes)?);
        self.geometries.push(Arc::downgrade(&reservation));
        self.uploaded += bytes;
        Ok(Some(reservation))
    }

    pub fn upload(
        &mut self,
        ctx: &GpuContext,
        source: &Arc<LightingData>,
    ) -> Result<Arc<Resources>, String> {
        let id = Arc::as_ptr(source) as usize;
        if let Some(resources) = self.resources.get(&id).and_then(Weak::upgrade)
            && Arc::ptr_eq(&resources.source, source)
        {
            return Ok(resources);
        }
        let bytes = validate(ctx, source)?;
        self.prepare_upload(ctx, bytes)?;
        let reservation = Reservation::new(&self.allocated, bytes)?;
        let mut faces = Vec::new();
        let descriptor_count = if source.samples.is_empty() {
            0
        } else {
            source.faces.len()
        };
        faces.try_reserve_exact(descriptor_count).map_err(|error| {
            format!("Baked surface GPU face descriptors could not allocate: {error}")
        })?;
        faces.extend(source.faces.iter().take(descriptor_count).map(|face| Face {
            dimensions: [face.width, face.height, face.map_count, 0],
            map_offsets: face.map_offsets,
        }));
        let allocation = ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal = ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let validation = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let (lighting_binding, unlit_binding) = binding(
            &ctx.device,
            &self.layout,
            &self.unlit_layout,
            &source.samples,
            &faces,
            [2f32.powf(source.exposure_ev), 0.0, 0.0, 0.0],
        );
        self.uploaded += bytes;
        let submission = ctx.queue.submit([]);
        let completion = ctx.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(RETIRE_TIMEOUT),
        });
        let validation = pollster::block_on(validation.pop());
        let internal = pollster::block_on(internal.pop());
        let allocation = pollster::block_on(allocation.pop());
        if let Some(error) = validation.or(internal).or(allocation) {
            return Err(format!("Baked surface GPU preparation failed: {error}"));
        }
        completion.map_err(|error| format!("Baked surface GPU upload failed: {error}"))?;
        let resources = Arc::new(Resources {
            source: source.clone(),
            lighting_binding,
            unlit_binding,
            _reservation: reservation,
        });
        self.resources.insert(id, Arc::downgrade(&resources));
        Ok(resources)
    }

    pub fn upload_material(
        &mut self,
        ctx: &GpuContext,
        source: &Arc<Material>,
    ) -> Result<Arc<MaterialResources>, String> {
        let id = Arc::as_ptr(source) as usize;
        if let Some(resources) = self.materials.get(&id).and_then(Weak::upgrade)
            && Arc::ptr_eq(&resources.source, source)
        {
            return Ok(resources);
        }
        let bytes = validate_material(ctx, source)?;
        self.prepare_upload(ctx, bytes)?;
        let reservation = Reservation::new(&self.allocated, bytes)?;
        let allocation = ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal = ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let validation = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let group = material_binding(ctx, &self.material_layout, source);
        self.uploaded += bytes;
        let submission = ctx.queue.submit([]);
        let completion = ctx.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(RETIRE_TIMEOUT),
        });
        let validation = pollster::block_on(validation.pop());
        let internal = pollster::block_on(internal.pop());
        let allocation = pollster::block_on(allocation.pop());
        if let Some(error) = validation.or(internal).or(allocation) {
            return Err(format!(
                "Baked surface material GPU preparation failed: {error}"
            ));
        }
        completion.map_err(|error| format!("Baked surface material GPU upload failed: {error}"))?;
        let resources = Arc::new(MaterialResources {
            source: source.clone(),
            binding: group,
            unlit: source.unlit,
            _reservation: reservation,
        });
        self.materials.insert(id, Arc::downgrade(&resources));
        Ok(resources)
    }

    fn prepare_upload(&mut self, ctx: &GpuContext, bytes: usize) -> Result<(), String> {
        if self
            .resources
            .values()
            .any(|resource| resource.strong_count() == 0)
            || self
                .materials
                .values()
                .any(|resource| resource.strong_count() == 0)
            || self
                .geometries
                .iter()
                .any(|resource| resource.strong_count() == 0)
            || self
                .uploaded
                .checked_add(bytes)
                .is_none_or(|total| total > MAX_GPU_BYTES)
        {
            let submission = ctx.queue.submit([]);
            ctx.device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(RETIRE_TIMEOUT),
                })
                .map_err(|error| format!("Baked surface GPU retirement failed: {error}"))?;
            self.uploaded = 0;
            self.resources
                .retain(|_, resource| resource.strong_count() > 0);
            self.materials
                .retain(|_, resource| resource.strong_count() > 0);
            self.geometries
                .retain(|resource| resource.strong_count() > 0);
        }
        Ok(())
    }
}

fn validate(ctx: &GpuContext, source: &LightingData) -> Result<usize, String> {
    if source.faces.is_empty() {
        return Err("Compiled surface lighting requires nonempty faces".into());
    }
    if source.faces.iter().all(|face| face.map_count == 0) && !source.samples.is_empty() {
        return Err("Unlit compiled surfaces unexpectedly contain baked samples".into());
    }
    if !source.exposure_ev.is_finite()
        || !(MIN_EXPOSURE_EV as f32..=MAX_EXPOSURE_EV as f32).contains(&source.exposure_ev)
    {
        return Err("Baked surface exposure must be finite and between -8 and 8 EV".into());
    }
    let samples = source.samples.len().checked_mul(size_of::<[f32; 4]>());
    let face_count = if source.samples.is_empty() {
        0
    } else {
        source.faces.len()
    };
    let faces = face_count.checked_mul(size_of::<Face>());
    let (samples, faces) = samples
        .zip(faces)
        .ok_or("Baked surface GPU byte size overflowed")?;
    let bytes = samples
        .checked_add(faces)
        .and_then(|bytes| bytes.checked_add(PARAM_BYTES))
        .filter(|bytes| *bytes <= MAX_GPU_BYTES)
        .ok_or("Baked surface GPU payload exceeds its byte budget")?;
    let limits = ctx.device.limits();
    let maximum = limits
        .max_buffer_size
        .min(limits.max_storage_buffer_binding_size);
    if samples as u64 > maximum
        || faces as u64 > maximum
        || source.samples.len() > u32::MAX as usize
        || source.faces.len() > (u32::MAX >> RECEIVER_SHIFT) as usize
        || face_count > 0 && limits.max_storage_buffers_per_shader_stage < 2
        || limits.max_bind_groups < 3
    {
        return Err("Baked surface lighting exceeds GPU storage or binding limits".into());
    }
    for face in &source.faces {
        if face.map_count == 0 {
            if face.width != 0 || face.height != 0 || face.map_offsets != [0; 4] {
                return Err(
                    "Unlit compiled surface unexpectedly contains lightmap dimensions or offsets"
                        .into(),
                );
            }
            continue;
        }
        if face.width == 0 || face.height == 0 || !matches!(face.map_count, 1 | 4) {
            return Err(
                "Baked surface lighting has invalid dimensions or directional map count".into(),
            );
        }
        let count = face
            .width
            .checked_mul(face.height)
            .ok_or("Baked surface sample dimensions overflowed")?;
        for &offset in &face.map_offsets[..face.map_count as usize] {
            if offset
                .checked_add(count)
                .is_none_or(|end| end as usize > source.samples.len())
            {
                return Err(
                    "Baked surface lightmap sample range is outside its retained buffer".into(),
                );
            }
        }
    }
    if source.samples.iter().any(|sample| {
        sample
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
    }) {
        return Err("Baked surface lighting contains invalid linear radiance".into());
    }
    Ok(bytes)
}

fn geometry_bytes(mesh: &crate::scene::SceneMesh) -> Result<usize, String> {
    let vertices = mesh
        .vertices
        .len()
        .checked_mul(size_of::<crate::scene::Vertex>());
    let indices = mesh.indices.len().checked_mul(size_of::<u32>());
    let mut bytes = vertices
        .zip(indices)
        .and_then(|(vertices, indices)| vertices.max(16).checked_add(indices.max(16)))
        .ok_or("Baked surface geometry GPU byte size overflowed")?;
    for lines in [
        &mesh.lines,
        &mesh.leak_lines,
        &mesh.reference_lines,
        &mesh.wire_lines,
        &mesh.tool_wire_lines,
        &mesh.rope_lines,
    ] {
        bytes = lines
            .len()
            .checked_mul(size_of::<crate::scene::LineVertex>())
            .and_then(|size| bytes.checked_add(size))
            .ok_or("Baked surface line GPU byte size overflowed")?;
    }
    let icons = mesh
        .icons
        .iter()
        .take(crate::icons::MAX_INSTANCES)
        .filter(|icon| icon.asset.material.is_some())
        .count();
    bytes = icons
        .checked_mul(6 * size_of::<super::IconVertex>())
        .and_then(|size| bytes.checked_add(size))
        .ok_or("Baked surface icon GPU byte size overflowed")?;
    mesh.detail_billboards
        .len()
        .checked_mul(8 * size_of::<crate::scene::LineVertex>())
        .and_then(|size| bytes.checked_add(size))
        .ok_or_else(|| "Baked surface billboard GPU byte size overflowed".into())
}

fn binding(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    unlit_layout: &wgpu::BindGroupLayout,
    samples: &[[f32; 4]],
    faces: &[Face],
    params: [f32; 4],
) -> (Option<wgpu::BindGroup>, wgpu::BindGroup) {
    let buffer = |label, bytes: &[u8], usage| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytes,
            usage,
        })
    };
    let params = buffer(
        "baked surface settings",
        bytemuck::bytes_of(&params),
        wgpu::BufferUsages::UNIFORM,
    );
    let unlit = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("compiled unlit surface settings"),
        layout: unlit_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 2,
            resource: params.as_entire_binding(),
        }],
    });
    let lighting = (!samples.is_empty()).then(|| {
        let samples = buffer(
            "baked surface samples",
            bytemuck::cast_slice(samples),
            wgpu::BufferUsages::STORAGE,
        );
        let faces = buffer(
            "baked surface faces",
            bytemuck::cast_slice(faces),
            wgpu::BufferUsages::STORAGE,
        );
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("baked surface lighting"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: samples.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: faces.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params.as_entire_binding(),
                },
            ],
        })
    });
    (lighting, unlit)
}

fn validate_material(ctx: &GpuContext, source: &Material) -> Result<usize, String> {
    let mut bytes = size_of::<MaterialParams>() + size_of::<[f32; 4]>();
    let maximum = ctx.device.limits().max_texture_dimension_2d;
    for texture in source.diffuse.iter().flatten().chain(
        source
            .normals
            .iter()
            .flatten()
            .map(|normal| &normal.texture),
    ) {
        let count = texture
            .width
            .checked_mul(texture.height)
            .ok_or("Baked surface texture dimensions overflowed")?;
        if texture.width == 0
            || texture.height == 0
            || texture.width > maximum
            || texture.height > maximum
            || count as usize != texture.pixels.len()
            || texture
                .width
                .checked_mul(size_of::<[f32; 4]>() as u32)
                .is_none()
            || texture
                .pixels
                .iter()
                .any(|pixel| pixel.iter().any(|value| !value.is_finite()))
        {
            return Err(format!(
                "Baked surface material {} has invalid texture data or exceeds GPU dimensions",
                source.name
            ));
        }
        bytes = texture
            .pixels
            .len()
            .checked_mul(size_of::<[f32; 4]>())
            .and_then(|size| bytes.checked_add(size))
            .ok_or("Baked surface material GPU byte size overflowed")?;
    }
    if bytes > MAX_GPU_BYTES {
        return Err("Baked surface material GPU payload exceeds its byte budget".into());
    }
    if source
        .color
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
        || source
            .diffuse_transform
            .iter()
            .chain(&source.normal_transform)
            .flatten()
            .flatten()
            .any(|value| !value.is_finite())
        || source.diffuse.iter().flatten().any(|texture| {
            texture.pixels.iter().any(|pixel| {
                pixel[..3].iter().any(|value| *value < 0.0) || !(0.0..=1.0).contains(&pixel[3])
            })
        })
    {
        return Err(format!(
            "Baked surface material {} has invalid linear colour or transforms",
            source.name
        ));
    }
    validate_directional_inputs(source)?;
    Ok(bytes)
}

fn material_binding(
    ctx: &GpuContext,
    layout: &wgpu::BindGroupLayout,
    source: &Material,
) -> wgpu::BindGroup {
    let absent = texture(
        ctx,
        &Texture {
            width: 1,
            height: 1,
            pixels: vec![[1.0; 4]],
        },
    );
    let diffuse = source
        .diffuse
        .each_ref()
        .map(|image| image.as_ref().map(|image| texture(ctx, image)));
    let normals = source
        .normals
        .each_ref()
        .map(|normal| normal.as_ref().map(|normal| texture(ctx, &normal.texture)));
    let views = [
        diffuse[0].as_ref(),
        diffuse[1].as_ref(),
        normals[0].as_ref(),
        normals[1].as_ref(),
    ]
    .map(|image| image.unwrap_or(&absent).create_view(&Default::default()));
    let encoding = |index: usize| match source.normals[index].as_ref().map(|normal| normal.encoding)
    {
        None => 0,
        Some(NormalEncoding::Tangent) => 1,
        Some(NormalEncoding::SsBump) => 2,
    };
    let transform = |rows: &[[[f32; 3]; 2]; 2]| {
        rows.map(|transform| transform.map(|row| [row[0], row[1], row[2], 0.0]))
    };
    let params = MaterialParams {
        options: [
            match source.kind {
                Kind::Single => 0,
                Kind::VertexTransition => 1,
                Kind::TwoTextureBlend => u32::from(source.diffuse[1].is_some()) * 2,
            },
            encoding(0),
            encoding(1),
            u32::from(source.detail_alpha_mask) | (u32::from(source.diffuse[1].is_some()) << 1),
        ],
        color: [source.color[0], source.color[1], source.color[2], 0.0],
        diffuse_transform: transform(&source.diffuse_transform),
        normal_transform: transform(&source.normal_transform),
    };
    let params = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("baked surface material settings"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(&source.name),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&views[0]),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&views[1]),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&views[2]),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&views[3]),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: params.as_entire_binding(),
            },
        ],
    })
}

fn texture(ctx: &GpuContext, source: &Texture) -> wgpu::Texture {
    let texture = super::texture_2d(
        ctx,
        "baked surface linear material",
        (source.width, source.height),
        1,
        wgpu::TextureFormat::Rgba32Float,
    );
    super::write_mip(
        ctx,
        &texture,
        0,
        bytemuck::cast_slice(&source.pixels),
        source.width * size_of::<[f32; 4]>() as u32,
        source.height,
        (source.width, source.height),
    );
    texture
}
