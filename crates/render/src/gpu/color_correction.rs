use std::time::Duration;

use wgpu::util::DeviceExt;

use super::{COLOR_FORMAT, GpuContext, RenderTarget};
use crate::color_correction::{self, EDGE, Preview};

#[derive(Default)]
pub(super) struct State {
    resources: Option<Resources>,
    failed: Option<((u64, u32, u32), String)>,
    needs_drain: bool,
}

struct Resources {
    identity: (u64, u32, u32),
    target: RenderTarget,
    pipeline: wgpu::RenderPipeline,
    binding: wgpu::BindGroup,
    settings: wgpu::Buffer,
    weight: f32,
}

impl State {
    fn retire(&mut self, ctx: &GpuContext) -> Result<(), String> {
        if self.resources.is_some() || self.needs_drain {
            let submission = ctx.queue.submit([]);
            ctx.device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(Duration::from_secs(1)),
                })
                .map_err(|error| format!("Color-correction GPU retirement failed: {error}"))?;
            self.resources = None;
            self.needs_drain = false;
        }
        Ok(())
    }

    pub(super) fn disable(&mut self, ctx: &GpuContext) -> Result<(), String> {
        self.failed = None;
        self.retire(ctx)
    }

    pub(super) fn prepare(
        &mut self,
        ctx: &GpuContext,
        preview: &Preview,
        size: (u32, u32),
    ) -> Result<(), String> {
        preview.validate()?;
        color_correction::target_bytes(size)?;
        let identity = (preview.lut.identity(), size.0, size.1);
        if let Some((failed, error)) = &self.failed
            && *failed == identity
        {
            return Err(error.clone());
        }
        if self
            .resources
            .as_ref()
            .is_none_or(|state| state.identity != identity)
        {
            self.retire(ctx)?;
            self.needs_drain = true;
            match Resources::new(ctx, preview, size) {
                Ok(resources) => {
                    self.resources = Some(resources);
                    self.failed = None;
                    self.needs_drain = false;
                }
                Err(error) => {
                    self.failed = Some((identity, error.clone()));
                    return Err(error);
                }
            }
        }
        self.resources.as_mut().unwrap().weight = preview.weight;
        Ok(())
    }

    pub(super) fn target(&self) -> &wgpu::TextureView {
        &self.resources.as_ref().unwrap().target.view
    }

    pub(super) fn draw(&self, ctx: &GpuContext, target: &wgpu::TextureView) -> Result<(), String> {
        let resources = self.resources.as_ref().unwrap();
        let allocation = ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal = ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let validation = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        ctx.queue.write_buffer(
            &resources.settings,
            0,
            bytemuck::cast_slice(&[resources.weight, 0.0, 0.0, 0.0]),
        );
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("color correction"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&resources.pipeline);
            pass.set_bind_group(0, &resources.binding, &[]);
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit([encoder.finish()]);
        let validation = pollster::block_on(validation.pop());
        let internal = pollster::block_on(internal.pop());
        let allocation = pollster::block_on(allocation.pop());
        if let Some(error) = validation.or(internal).or(allocation) {
            return Err(format!("Color-correction GPU frame failed: {error}"));
        }
        Ok(())
    }
}

impl Resources {
    fn new(ctx: &GpuContext, preview: &Preview, size: (u32, u32)) -> Result<Self, String> {
        let limits = ctx.device.limits();
        if limits.max_texture_dimension_3d < EDGE
            || size.0 > limits.max_texture_dimension_2d
            || size.1 > limits.max_texture_dimension_2d
        {
            return Err("Color-correction textures exceed GPU dimension limits".into());
        }
        let rgba = preview.lut.rgba()?;
        let allocation = ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal = ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let validation = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let resources = Self::create(ctx, preview, size, &rgba);
        let submission = ctx.queue.submit([]);
        let completion = ctx.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(1)),
        });
        let validation = pollster::block_on(validation.pop());
        let internal = pollster::block_on(internal.pop());
        let allocation = pollster::block_on(allocation.pop());
        if let Some(error) = validation.or(internal).or(allocation) {
            return Err(format!("Color-correction GPU preparation failed: {error}"));
        }
        completion.map_err(|error| format!("Color-correction LUT upload failed: {error}"))?;
        Ok(resources)
    }

    fn create(ctx: &GpuContext, preview: &Preview, size: (u32, u32), rgba: &[u8]) -> Self {
        let device = &ctx.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("color correction"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                super::uniform_entry(3, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("color correction"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../color_correction.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("color correction"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("color correction"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let lut = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("color-correction LUT"),
            size: wgpu::Extent3d {
                width: EDGE,
                height: EDGE,
                depth_or_array_layers: EDGE,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        ctx.queue.write_texture(
            lut.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(EDGE * 4),
                rows_per_image: Some(EDGE),
            },
            lut.size(),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("color-correction trilinear"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let target = RenderTarget::new(ctx, size.0, size.1);
        let display = target.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            ..Default::default()
        });
        let settings = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("color-correction weight"),
            contents: bytemuck::cast_slice(&[preview.weight, 0.0, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("color correction"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&display),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &lut.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: settings.as_entire_binding(),
                },
            ],
        });
        Self {
            identity: (preview.lut.identity(), size.0, size.1),
            target,
            pipeline,
            binding,
            settings,
            weight: preview.weight,
        }
    }
}

#[cfg(test)]
mod tests;
