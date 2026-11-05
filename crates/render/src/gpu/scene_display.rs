use std::collections::VecDeque;
use std::time::Duration;

use wgpu::util::DeviceExt;

use super::{COLOR_FORMAT, DEPTH_FORMAT, GpuContext, MAX_SCRATCH_TARGETS};

pub(super) const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const MAX_HDR_BYTES: u64 = 128 * 1024 * 1024;
const MAX_DEPTH_BYTES: u64 = 64 * 1024 * 1024;
const RETIRE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_CANVAS_PIXELS: u64 = 16_777_216;
pub(super) const MAX_READBACK_BYTES: u64 = 80 * 1024 * 1024;
const PRESENTATION_AND_REPLACEMENT_BYTES: u64 = 192 * 1024 * 1024;
const DRIVER_HEADROOM_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FRAME_RESOURCE_BYTES: u64 = 640 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Targets {
    hdr: VecDeque<Color>,
    depth: VecDeque<Depth>,
    display: Option<Display>,
}

struct Color {
    view: wgpu::TextureView,
    binding: wgpu::BindGroup,
    size: (u32, u32),
    bytes: u64,
}

struct Depth {
    view: wgpu::TextureView,
    size: (u32, u32),
    bytes: u64,
}

struct Display {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    settings: wgpu::Buffer,
}

pub(super) fn target_bytes(
    ctx: &GpuContext,
    size: (u32, u32),
    pixel_bytes: u64,
) -> Result<u64, String> {
    let maximum = ctx.device.limits().max_texture_dimension_2d;
    if size.0 == 0 || size.1 == 0 || size.0 > maximum || size.1 > maximum {
        return Err(format!(
            "Scene target requires 1..{maximum} pixels per axis"
        ));
    }
    u64::from(size.0)
        .checked_mul(u64::from(size.1))
        .filter(|pixels| *pixels <= MAX_CANVAS_PIXELS)
        .and_then(|pixels| pixels.checked_mul(pixel_bytes))
        .ok_or_else(|| "Scene target exceeds the 16,777,216-pixel canvas budget".to_owned())
}

pub(super) fn drain(ctx: &GpuContext) -> Result<(), String> {
    let submission = ctx.queue.submit([]);
    ctx.device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(RETIRE_TIMEOUT),
        })
        .map_err(|error| format!("Scene GPU retirement failed: {error}"))?;
    Ok(())
}

impl Targets {
    pub(super) fn admit_frame(&self, ctx: &GpuContext, size: (u32, u32)) -> Result<(), String> {
        target_bytes(ctx, size, 8)?;
        let total = MAX_HDR_BYTES
            .checked_add(MAX_DEPTH_BYTES)
            .and_then(|bytes| bytes.checked_add(crate::color_correction::MAX_TARGET_BYTES))
            .and_then(|bytes| bytes.checked_add(crate::sky3d::MAX_TARGET_BYTES as u64))
            .and_then(|bytes| bytes.checked_add(PRESENTATION_AND_REPLACEMENT_BYTES))
            .and_then(|bytes| bytes.checked_add(MAX_READBACK_BYTES))
            .and_then(|bytes| bytes.checked_add(DRIVER_HEADROOM_BYTES))
            .ok_or("HDR frame resource accounting overflowed")?;
        if total > MAX_FRAME_RESOURCE_BYTES {
            return Err(
                "HDR frame resources and replacement overlap exceed the 640 MiB target budget"
                    .into(),
            );
        }
        Ok(())
    }

    pub(super) fn depth(
        &mut self,
        ctx: &GpuContext,
        size: (u32, u32),
    ) -> Result<wgpu::TextureView, String> {
        let bytes = target_bytes(ctx, size, 4)?;
        if let Some(index) = self.depth.iter().position(|entry| entry.size == size) {
            let entry = self.depth.remove(index).unwrap();
            let view = entry.view.clone();
            self.depth.push_back(entry);
            return Ok(view);
        }
        let mut retained = self.depth.iter().map(|entry| entry.bytes).sum::<u64>();
        if self.depth.len() >= MAX_SCRATCH_TARGETS || retained + bytes > MAX_DEPTH_BYTES {
            drain(ctx)?;
            while self.depth.len() >= MAX_SCRATCH_TARGETS || retained + bytes > MAX_DEPTH_BYTES {
                let entry = self
                    .depth
                    .pop_front()
                    .ok_or("Depth target exceeds the cache budget")?;
                retained -= entry.bytes;
            }
        }
        let allocation = ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal = ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let validation = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let view = create_texture(
            ctx,
            "scene depth",
            size,
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let completion = drain(ctx);
        let validation = pollster::block_on(validation.pop());
        let internal = pollster::block_on(internal.pop());
        let allocation = pollster::block_on(allocation.pop());
        if let Some(error) = validation.or(internal).or(allocation) {
            return Err(format!("Depth target allocation failed: {error}"));
        }
        completion?;
        self.depth.push_back(Depth {
            view: view.clone(),
            size,
            bytes,
        });
        Ok(view)
    }

    pub(super) fn hdr(
        &mut self,
        ctx: &GpuContext,
        size: (u32, u32),
    ) -> Result<wgpu::TextureView, String> {
        let bytes = target_bytes(ctx, size, 8)?;
        let features = ctx.adapter.get_texture_format_features(HDR_FORMAT);
        if !features
            .allowed_usages
            .contains(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING)
            || !features
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::BLENDABLE)
        {
            return Err("GPU does not support blendable RGBA16F scene targets".into());
        }
        if let Some(index) = self.hdr.iter().position(|entry| entry.size == size) {
            let entry = self.hdr.remove(index).unwrap();
            let view = entry.view.clone();
            self.hdr.push_back(entry);
            return Ok(view);
        }
        let mut retained = self.hdr.iter().map(|entry| entry.bytes).sum::<u64>();
        if self.hdr.len() >= MAX_SCRATCH_TARGETS || retained + bytes > MAX_HDR_BYTES {
            drain(ctx)?;
            while self.hdr.len() >= MAX_SCRATCH_TARGETS || retained + bytes > MAX_HDR_BYTES {
                let entry = self
                    .hdr
                    .pop_front()
                    .ok_or("HDR target exceeds the cache budget")?;
                retained -= entry.bytes;
            }
        }
        let allocation = ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal = ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let validation = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let display = self.display.is_none().then(|| Display::new(&ctx.device));
        let shared = self.display.as_ref().or(display.as_ref()).unwrap();
        let view = create_texture(
            ctx,
            "linear HDR scene",
            size,
            HDR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let binding = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("HDR scene display"),
            layout: &shared.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: shared.settings.as_entire_binding(),
                },
            ],
        });
        let completion = drain(ctx);
        let validation = pollster::block_on(validation.pop());
        let internal = pollster::block_on(internal.pop());
        let allocation = pollster::block_on(allocation.pop());
        if let Some(error) = validation.or(internal).or(allocation) {
            return Err(format!("HDR scene target allocation failed: {error}"));
        }
        completion?;
        if let Some(display) = display {
            self.display = Some(display);
        }
        self.hdr.push_back(Color {
            view: view.clone(),
            binding,
            size,
            bytes,
        });
        Ok(view)
    }

    pub(super) fn present(
        &self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        exposure_gain: f32,
    ) {
        let display = self.display.as_ref().unwrap();
        let source = self.hdr.back().unwrap();
        ctx.queue.write_buffer(
            &display.settings,
            0,
            bytemuck::cast_slice(&[exposure_gain, 0.0_f32, 0.0, 0.0]),
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("HDR scene display"),
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
        pass.set_pipeline(&display.pipeline);
        pass.set_bind_group(0, &source.binding, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn create_texture(
    ctx: &GpuContext,
    label: &str,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::TextureView {
    ctx.device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
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
}

impl Display {
    fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("HDR scene display"),
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
                super::uniform_entry(1, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("HDR scene display"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("HDR display"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../scene_display.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("HDR display"),
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
        let settings = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("HDR scene exposure"),
            contents: bytemuck::cast_slice(&[1.0_f32, 0.0, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        Self {
            layout,
            pipeline,
            settings,
        }
    }
}
