use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use wgpu::util::DeviceExt;

use super::{COLOR_FORMAT, DEPTH_FORMAT, Globals, GpuContext, GpuScene, Renderer, ViewOptions};
use crate::sky3d;

const RETIRE_TIMEOUT: Duration = Duration::from_secs(1);
const FRAME_UPLOAD_BYTES: usize = std::mem::size_of::<Globals>() + std::mem::size_of::<[f32; 4]>();

struct Reservation {
    allocated: Arc<AtomicUsize>,
    bytes: usize,
}

impl Reservation {
    fn new(allocated: &Arc<AtomicUsize>, bytes: usize) -> Result<Self, String> {
        #[allow(deprecated, reason = "Atomic::try_update requires Rust 1.95")]
        allocated.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(bytes).filter(|total| *total <= sky3d::MAX_GPU_ASSET_BYTES)
        }).map_err(|_| format!("3D sky retained GPU geometry/material payload exceeds {} bytes; release earlier scenes before replacement", sky3d::MAX_GPU_ASSET_BYTES))?;
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

pub(super) struct Miniature {
    id: u64,
    camera: sky3d::Camera,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    batches: Vec<crate::scene::Batch>,
    materials: Vec<wgpu::BindGroup>,
    globals: wgpu::Buffer,
    binding: wgpu::BindGroup,
    reservation: Reservation,
}

impl Miniature {
    pub(super) fn matches_scene(&self, scene: &sky3d::Scene) -> bool {
        self.id == scene.id
    }

    pub(super) fn camera_projection(
        &self,
        camera: &crate::Camera,
        aspect: f32,
    ) -> Result<glam::Mat4, String> {
        self.camera.view_projection(camera, aspect)
    }
}

struct Target {
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    size: (u32, u32),
    main_size: (u32, u32),
    binding: wgpu::BindGroup,
    params: wgpu::Buffer,
}

pub(super) struct State {
    pub layout: wgpu::BindGroupLayout,
    globals_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    target: Option<Target>,
    scenes: HashMap<u64, Weak<Miniature>>,
    allocated: Arc<AtomicUsize>,
    uploaded: usize,
}

impl State {
    pub(super) fn allocated_bytes(&self) -> usize {
        self.allocated.load(Ordering::Relaxed)
    }

    pub fn new(device: &wgpu::Device, globals_layout: &wgpu::BindGroupLayout) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3D sky composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                super::uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("3D sky composite edges"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            layout,
            globals_layout: globals_layout.clone(),
            sampler,
            target: None,
            scenes: HashMap::new(),
            allocated: Arc::default(),
            uploaded: 0,
        }
    }

    fn ensure_upload_capacity(
        &mut self,
        ctx: &GpuContext,
        bytes: usize,
        retire: bool,
    ) -> Result<(), String> {
        if bytes > sky3d::MAX_GPU_UPLOAD_BYTES {
            return Err("3D sky transient GPU upload exceeds its byte budget".into());
        }
        let retired = retire && self.scenes.values().any(|scene| scene.strong_count() == 0);
        let exceeds = self
            .uploaded
            .checked_add(bytes)
            .is_none_or(|total| total > sky3d::MAX_GPU_UPLOAD_BYTES);
        if retired || exceeds {
            let submission = ctx.queue.submit([]);
            ctx.device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(RETIRE_TIMEOUT),
                })
                .map_err(|error| {
                    format!("3D sky GPU retirement could not finish within one second: {error}")
                })?;
            self.uploaded = 0;
            self.scenes.retain(|_, scene| scene.strong_count() > 0);
        }
        Ok(())
    }

    fn prepare_upload(&mut self, ctx: &GpuContext, bytes: usize) -> Result<(), String> {
        self.ensure_upload_capacity(ctx, bytes, true)?;
        self.uploaded = self
            .uploaded
            .checked_add(bytes)
            .filter(|total| *total <= sky3d::MAX_GPU_UPLOAD_BYTES)
            .ok_or_else(|| "3D sky aggregate transient GPU upload budget exhausted".to_owned())?;
        Ok(())
    }

    pub(super) fn prepare_render(&mut self, ctx: &GpuContext) -> Result<(), String> {
        self.ensure_upload_capacity(ctx, FRAME_UPLOAD_BYTES, false)
    }

    fn target(&mut self, ctx: &GpuContext, main_size: (u32, u32)) -> Result<&Target, String> {
        let size = sky3d::target_size(main_size, self.allocated.load(Ordering::Relaxed))?;
        let capacity = sky3d::target_size(
            (sky3d::MAX_TARGET_DIMENSION, sky3d::MAX_TARGET_DIMENSION),
            self.allocated.load(Ordering::Relaxed),
        )?;
        let maximum = ctx.device.limits().max_texture_dimension_2d;
        if capacity.0 > maximum || capacity.1 > maximum {
            return Err(format!(
                "3D sky target exceeds GPU dimension limit {maximum}"
            ));
        }
        let params = [
            main_size.0 as f32,
            main_size.1 as f32,
            size.0 as f32,
            size.1 as f32,
        ];
        if let Some(target) = &mut self.target {
            if target.size != size || target.main_size != main_size {
                ctx.queue
                    .write_buffer(&target.params, 0, bytemuck::cast_slice(&params));
                target.size = size;
                target.main_size = main_size;
            }
            return Ok(self.target.as_ref().unwrap());
        }
        let texture = |label, format, usage| {
            ctx.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: capacity.0,
                        height: capacity.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let color = texture(
            "3D sky color",
            COLOR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = texture(
            "3D sky depth",
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let params = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("3D sky main target size"),
                contents: bytemuck::cast_slice(&params),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let binding = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("3D sky composite"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params.as_entire_binding(),
                },
            ],
        });
        self.target = Some(Target {
            color,
            depth,
            size,
            main_size,
            binding,
            params,
        });
        Ok(self.target.as_ref().unwrap())
    }
}

impl GpuScene {
    pub fn sky3d_error(&self) -> Option<&str> {
        self.sky3d_error.as_deref()
    }

    pub fn sky3d_render_error(
        &self,
        camera: &crate::Camera,
        options: &ViewOptions,
    ) -> Option<String> {
        if !options.sky3d_active(camera) {
            return None;
        }
        if let Some(error) = &self.sky3d_error {
            return Some(error.clone());
        }
        match &self.sky3d {
            Some(scene) => scene.camera.view_projection(camera, 1.0).err().or_else(|| {
                sky3d::target_size(
                    (sky3d::MAX_TARGET_DIMENSION, sky3d::MAX_TARGET_DIMENSION),
                    scene.reservation.allocated.load(Ordering::Relaxed),
                )
                .err()
            }),
            None => Some("3D sky preview requested without a ready miniature scene".into()),
        }
    }
}

impl Renderer {
    pub(super) fn upload_sky3d(
        &mut self,
        ctx: &GpuContext,
        scene: &sky3d::Scene,
    ) -> Result<Arc<Miniature>, String> {
        scene.camera.validate()?;
        if let Some(scene) = self.sky3d.scenes.get(&scene.id).and_then(Weak::upgrade) {
            return Ok(scene);
        }
        let limits = ctx.device.limits();
        if scene.vertices.len() * std::mem::size_of::<crate::scene::Vertex>()
            > limits.max_buffer_size as usize
            || scene.indices.len() * std::mem::size_of::<u32>() > limits.max_buffer_size as usize
            || limits.max_texture_dimension_2d < sky3d::MAX_TARGET_DIMENSION
            || scene.materials.iter().any(|material| {
                material.info.width > limits.max_texture_dimension_2d
                    || material.info.height > limits.max_texture_dimension_2d
            })
        {
            return Err("3D sky geometry/material/target exceeds GPU device limits".into());
        }
        self.sky3d.prepare_upload(ctx, scene.upload_bytes)?;
        let reservation = Reservation::new(&self.sky3d.allocated, scene.gpu_bytes)?;
        let mut materials = Vec::with_capacity(scene.materials.len());
        for material in &scene.materials {
            let info = &material.info;
            let texture = info
                .base
                .as_ref()
                .ok_or_else(|| format!("3D sky material {} lost its base texture", info.name))?;
            let texture = super::upload_vtf(ctx, texture).ok_or_else(|| {
                format!("3D sky material {} could not upload its texture", info.name)
            })?;
            materials.push(self.material_bind_group_with_base(
                ctx,
                info,
                material.unlit,
                &self.sampler,
                texture,
            ));
        }
        let buffer = |label, bytes: &[u8], usage| {
            ctx.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: if bytes.is_empty() { &[0; 16] } else { bytes },
                    usage,
                })
        };
        let vertices = buffer(
            "3D sky vertices",
            bytemuck::cast_slice(&scene.vertices),
            wgpu::BufferUsages::VERTEX,
        );
        let indices = buffer(
            "3D sky indices",
            bytemuck::cast_slice(&scene.indices),
            wgpu::BufferUsages::INDEX,
        );
        let globals = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("3D sky globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let binding = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("3D sky globals"),
            layout: &self.sky3d.globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let miniature = Arc::new(Miniature {
            id: scene.id,
            camera: scene.camera,
            vertices,
            indices,
            batches: scene.batches.clone(),
            materials,
            globals,
            binding,
            reservation,
        });
        self.sky3d
            .scenes
            .insert(scene.id, Arc::downgrade(&miniature));
        Ok(miniature)
    }

    pub(super) fn render_sky3d(
        &mut self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &crate::Camera,
        globals: &Globals,
        size: (u32, u32),
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<wgpu::BindGroup, String> {
        let scene = scene
            .sky3d
            .as_ref()
            .ok_or_else(|| "3D sky scene is unavailable".to_owned())?;
        self.sky3d.prepare_render(ctx)?;
        self.sky3d.uploaded += FRAME_UPLOAD_BYTES;
        let target = self.sky3d.target(ctx, size)?;
        let mut globals = *globals;
        globals.view_proj = scene
            .camera
            .view_projection(camera, size.0 as f32 / size.1.max(1) as f32)?
            .to_cols_array_2d();
        globals.eye = scene
            .camera
            .transform(camera)?
            .position
            .extend(1.0)
            .to_array();
        globals.fog_color = [0.0; 4];
        globals.fog_range = [0.0; 4];
        globals.lighting_settings = [0.0; 4];
        globals.surface_view = [0; 4];
        ctx.queue
            .write_buffer(&scene.globals, 0, bytemuck::bytes_of(&globals));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("3D sky miniature"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &target.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.sky3d_surface_pipeline);
        pass.set_viewport(
            0.0,
            0.0,
            target.size.0 as f32,
            target.size.1 as f32,
            0.0,
            1.0,
        );
        pass.set_scissor_rect(0, 0, target.size.0, target.size.1);
        pass.set_bind_group(0, &scene.binding, &[]);
        pass.set_vertex_buffer(0, scene.vertices.slice(..));
        pass.set_index_buffer(scene.indices.slice(..), wgpu::IndexFormat::Uint32);
        for batch in &scene.batches {
            pass.set_bind_group(1, &scene.materials[batch.material], &[]);
            pass.draw_indexed(
                batch.first_index..batch.first_index + batch.index_count,
                0,
                0..1,
            );
        }
        Ok(target.binding.clone())
    }
}
