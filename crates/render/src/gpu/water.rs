use std::{mem::offset_of, ops::Range};

use source_formats::vtf::Vtf;
use wgpu::util::DeviceExt;

use super::{
    COLOR_FORMAT, DEPTH_FORMAT, GpuContext, Vertex, scene_display, scene_shader, uniform_entry,
    upload_vtf_color_space,
};

pub(super) const CAPTURE_SIZE: (u32, u32) = (1024, 1024);

#[derive(Default)]
pub(super) struct State {
    capture: Option<Capture>,
    pipeline: Option<Pipeline>,
}

pub(super) struct Capture {
    pub(super) color: wgpu::TextureView,
    pub(super) depth: wgpu::TextureView,
}

struct Pipeline {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    capture_sampler: wgpu::Sampler,
    normal_sampler: wgpu::Sampler,
}

pub(super) struct Surface {
    normal: wgpu::TextureView,
    bump_transform: [f32; 4],
    refract_amount: f32,
    fog_color: [f32; 3],
}

pub(super) struct Draw<'a> {
    pipeline: &'a wgpu::RenderPipeline,
    binding: wgpu::BindGroup,
}

impl Draw<'_> {
    pub(super) fn run(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        globals: &wgpu::BindGroup,
        indices: Range<u32>,
    ) {
        pass.set_pipeline(self.pipeline);
        pass.set_bind_group(0, globals, &[]);
        pass.set_bind_group(1, &self.binding, &[]);
        pass.draw_indexed(indices, 0, 0..1);
    }
}

impl State {
    pub(super) fn capture(&mut self, ctx: &GpuContext) -> Result<&Capture, String> {
        if self.capture.is_none() {
            scene_display::target_bytes(ctx, CAPTURE_SIZE, 8)?;
            let capture = checked_gpu(ctx, "Water refraction capture", || Capture::new(ctx))?;
            self.capture = Some(capture);
        }
        self.capture
            .as_ref()
            .ok_or_else(|| "Water refraction capture is unavailable".into())
    }

    pub(super) fn prepare_surface(
        &mut self,
        ctx: &GpuContext,
        globals_layout: &wgpu::BindGroupLayout,
        normal: &Vtf,
        bump_transform: [f32; 4],
        refract_amount: f32,
        fog_color: [f32; 3],
    ) -> Result<Surface, String> {
        if !refract_amount.is_finite()
            || bump_transform.iter().any(|value| !value.is_finite())
            || fog_color.iter().any(|value| !(0.0..=1.0).contains(value))
        {
            return Err("Water refraction inputs must be finite and fog RGB must fit 0–1".into());
        }
        let needs_pipeline = self.pipeline.is_none();
        let (pipeline, texture) = checked_gpu(ctx, "Water refraction surface", || {
            (
                needs_pipeline.then(|| Pipeline::new(ctx, globals_layout)),
                upload_vtf_color_space(ctx, normal, false),
            )
        })?;
        let texture = texture.ok_or("Water normal texture could not be uploaded")?;
        if let Some(pipeline) = pipeline {
            self.pipeline = Some(pipeline);
        }
        Ok(Surface {
            normal: texture.create_view(&Default::default()),
            bump_transform,
            refract_amount,
            fog_color,
        })
    }

    pub(super) fn draw(
        &self,
        ctx: &GpuContext,
        surface: &Surface,
        main_size: (u32, u32),
    ) -> Result<Draw<'_>, String> {
        let capture = self
            .capture
            .as_ref()
            .ok_or("Water refraction has no prepared capture")?;
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or("Water refraction has no prepared surface pipeline")?;
        if main_size.0 == 0 || main_size.1 == 0 {
            return Err("Water refraction requires a nonempty main target".into());
        }
        let [r, g, b] = surface.fog_color;
        let settings = [
            [
                (main_size.0 as f32).recip(),
                (main_size.1 as f32).recip(),
                surface.refract_amount,
                0.0,
            ],
            surface.bump_transform,
            [r, g, b, 0.0],
        ];
        let buffer = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Water refraction parameters"),
                contents: bytemuck::cast_slice(&settings),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bindings = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Water refraction surface"),
            layout: &pipeline.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&capture.color),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&pipeline.capture_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&surface.normal),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&pipeline.normal_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: buffer.as_entire_binding(),
                },
            ],
        });
        Ok(Draw {
            pipeline: &pipeline.pipeline,
            binding: bindings,
        })
    }
}

impl Capture {
    fn new(ctx: &GpuContext) -> Self {
        let create = |label, format, usage| {
            ctx.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: CAPTURE_SIZE.0,
                        height: CAPTURE_SIZE.1,
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
        Self {
            color: create(
                "Water refraction color and height",
                COLOR_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            ),
            depth: create(
                "Water refraction depth",
                DEPTH_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            ),
        }
    }
}

impl Pipeline {
    fn new(ctx: &GpuContext, globals_layout: &wgpu::BindGroupLayout) -> Self {
        let device = &ctx.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Water refraction surface"),
            entries: &[
                texture_entry(0),
                sampler_entry(1),
                texture_entry(2),
                sampler_entry(3),
                uniform_entry(4, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Water refraction surface"),
            bind_group_layouts: &[Some(globals_layout), Some(&layout)],
            immediate_size: 0,
        });
        let shader = scene_shader(
            device,
            "Water refraction shader",
            include_str!("../water.wgsl"),
        );
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Water refraction surface"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: offset_of!(Vertex, position) as u64,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: offset_of!(Vertex, uv) as u64,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: offset_of!(Vertex, tangent) as u64,
                            shader_location: 8,
                        },
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Cw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
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
        let capture_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Water refraction capture clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let normal_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Water normal repeat"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Self {
            layout,
            pipeline,
            capture_sampler,
            normal_sampler,
        }
    }
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn checked_gpu<T>(
    ctx: &GpuContext,
    label: &str,
    operation: impl FnOnce() -> T,
) -> Result<T, String> {
    let allocation = ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal = ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
    let validation = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let output = operation();
    let completion = scene_display::drain(ctx);
    let validation = pollster::block_on(validation.pop());
    let internal = pollster::block_on(internal.pop());
    let allocation = pollster::block_on(allocation.pop());
    if let Some(error) = validation.or(internal).or(allocation) {
        return Err(format!("{label} failed: {error}"));
    }
    completion?;
    Ok(output)
}
