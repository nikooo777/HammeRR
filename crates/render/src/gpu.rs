//! wgpu device setup, pipelines, and drawing.

use std::mem::size_of;

use source_formats::vtf::{ImageFormat, Vtf};
use wgpu::util::DeviceExt;

use crate::camera::Camera;
use crate::materials::{MaterialId, MaterialInfo, MaterialLibrary};
use crate::scene::{Batch, LineVertex, SceneMesh, Vertex};

/// The view format the pipelines render into; see [`RenderTarget`].
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Linear-space clear color; a dark slate like most modern editors.
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.012,
    g: 0.014,
    b: 0.018,
    a: 1.0,
};

#[derive(Debug, Clone)]
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl GpuContext {
    /// Picks the high-performance adapter and enables BC texture compression
    /// when available, so DXT textures upload without decoding.
    pub fn new() -> Result<Self, String> {
        let descriptor = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        let backends = descriptor.backends;
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU adapter: {e}"))?;
        let supports_view_formats = |adapter: &wgpu::Adapter| {
            adapter
                .get_downlevel_capabilities()
                .flags
                .contains(wgpu::DownlevelFlags::VIEW_FORMATS)
        };
        let adapter = if supports_view_formats(&adapter) {
            adapter
        } else {
            let name = adapter.get_info().name;
            pollster::block_on(instance.enumerate_adapters(backends)).into_iter().find(supports_view_formats)
                .ok_or_else(|| format!("GPU adapter {name:?} cannot render texture views, and no compatible adapter is available. Check GPU access and Vulkan driver availability"))?
        };
        let features = adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("map renderer"),
            required_features: features,
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| format!("could not open GPU device: {e}"))?;
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
        })
    }

    fn supports_bc(&self) -> bool {
        self.device
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
    }
}

/// A colour target that Slint can display. Slint composites in sRGB space,
/// so the texture is plain `Rgba8Unorm` holding sRGB-encoded bytes, written
/// through an sRGB view that does the encoding.
pub struct RenderTarget {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

impl RenderTarget {
    pub fn new(ctx: &GpuContext, width: u32, height: u32) -> Self {
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("render target"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[COLOR_FORMAT],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(COLOR_FORMAT),
            ..Default::default()
        });
        Self { texture, view }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.texture.width(), self.texture.height())
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    light_dir: [f32; 4],
    eye: [f32; 4],
    surface_view: [u32; 4],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SurfaceView {
    #[default]
    Textured,
    Flat,
    Wireframe,
    LightmapDensity,
    SmoothingGroups,
}

impl SurfaceView {
    pub const ALL: [Self; 5] = [
        Self::Textured,
        Self::Flat,
        Self::Wireframe,
        Self::LightmapDensity,
        Self::SmoothingGroups,
    ];

    pub fn index(self) -> i32 {
        Self::ALL.iter().position(|mode| *mode == self).unwrap() as i32
    }

    pub fn from_index(index: i32) -> Option<Self> {
        usize::try_from(index)
            .ok()
            .and_then(|index| Self::ALL.get(index).copied())
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Textured => "textured",
            Self::Flat => "flat",
            Self::Wireframe => "wireframe",
            Self::LightmapDensity => "lightmap_density",
            Self::SmoothingGroups => "smoothing_groups",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "textured" => Ok(Self::Textured),
            "flat" => Ok(Self::Flat),
            "wireframe" => Ok(Self::Wireframe),
            "lightmap_density" => Ok(Self::LightmapDensity),
            "smoothing_groups" => Ok(Self::SmoothingGroups),
            _ => Err("Surface view must be textured, flat, wireframe, lightmap_density or smoothing_groups".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipGuide {
    pub normal: glam::Vec3,
    pub distance: f32,
    pub bounds: [glam::Vec3; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AreaGuide {
    pub corners: [glam::Vec2; 2],
    pub camera: Camera,
    pub aspect: f32,
    pub contained: bool,
}

/// Per-frame visibility filters, applied without rebuilding the scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewOptions {
    pub surface: SurfaceView,
    pub smoothing_group: u32,
    pub tool_textures: bool,
    pub leak_path: bool,
    pub grid_visible: bool,
    pub grid_size: f32,
    pub grid_elevation: f32,
    pub block_preview: Option<[glam::Vec3; 2]>,
    pub clip_preview: Option<ClipGuide>,
    pub transform_handles: Option<crate::handles::Guide>,
    pub handle_pixel_ratio: f32,
    pub area_preview: Option<AreaGuide>,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            surface: SurfaceView::Textured,
            smoothing_group: 1,
            tool_textures: true,
            leak_path: true,
            grid_visible: true,
            grid_size: 16.0,
            grid_elevation: 0.0,
            block_preview: None,
            clip_preview: None,
            transform_handles: None,
            handle_pixel_ratio: 1.0,
            area_preview: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct DrawBatch {
    batch: Batch,
    tool: bool,
    helper: bool,
}

/// Geometry uploaded by a [`Renderer`]; draw it with the same renderer.
pub struct GpuScene {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    opaque: Vec<DrawBatch>,
    translucent: Vec<DrawBatch>,
    lines: Option<(wgpu::Buffer, u32)>,
    leak_lines: Option<(wgpu::Buffer, u32)>,
    wire_lines: Option<(wgpu::Buffer, u32)>,
    tool_wire_lines: Option<(wgpu::Buffer, u32)>,
}

pub struct Renderer {
    globals: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    opaque_pipeline: wgpu::RenderPipeline,
    translucent_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    /// Bind groups by [`MaterialId`], uploaded on first use.
    materials: Vec<Option<wgpu::BindGroup>>,
    white: wgpu::Texture,
    missing: wgpu::Texture,
    depth: Option<(wgpu::TextureView, (u32, u32))>,
    overlay: wgpu::Buffer,
}

impl Renderer {
    pub fn new(ctx: &GpuContext) -> Self {
        let device = &ctx.device;
        let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });

        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform_entry(3, wgpu::ShaderStages::FRAGMENT),
                texture_entry(4),
                texture_entry(5),
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("map"),
            bind_group_layouts: &[Some(&globals_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let pipeline = |label, blend, depth_write| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x3,
                            1 => Float32x3,
                            2 => Float32x2,
                            3 => Unorm8x4,
                            4 => Float32x3,
                            5 => Uint32,
                        ],
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // VMF faces wind clockwise when seen from outside.
                    front_face: wgpu::FrontFace::Cw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write),
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
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let opaque_pipeline = pipeline("opaque", None, true);
        let translucent_pipeline =
            pipeline("translucent", Some(wgpu::BlendState::ALPHA_BLENDING), false);

        let line_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("lines"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });
        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("lines"),
            layout: Some(&line_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_line"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<LineVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            // Selection outlines stay visible through other geometry.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_line"),
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

        Self {
            globals,
            globals_bind_group,
            material_layout,
            sampler,
            opaque_pipeline,
            translucent_pipeline,
            line_pipeline,
            materials: Vec::new(),
            white: solid_texture(ctx, [255, 255, 255, 255]),
            missing: checkerboard_texture(ctx),
            depth: None,
            overlay: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("spatial overlays"),
                size: 65_536,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        }
    }

    pub fn upload(
        &mut self,
        ctx: &GpuContext,
        mesh: &SceneMesh,
        library: &MaterialLibrary,
    ) -> GpuScene {
        let device = &ctx.device;
        let buffer = |label, contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        // Zero-sized buffers can't be bound, so empty maps get a dummy one.
        let non_empty = |bytes: &[u8]| {
            if bytes.is_empty() {
                vec![0; 16]
            } else {
                bytes.to_vec()
            }
        };
        let vertices = buffer(
            "map vertices",
            &non_empty(bytemuck::cast_slice(&mesh.vertices)),
            wgpu::BufferUsages::VERTEX,
        );
        let indices = buffer(
            "map indices",
            &non_empty(bytemuck::cast_slice(&mesh.indices)),
            wgpu::BufferUsages::INDEX,
        );
        let lines = (!mesh.lines.is_empty()).then(|| {
            let lines = buffer(
                "selection lines",
                bytemuck::cast_slice(&mesh.lines),
                wgpu::BufferUsages::VERTEX,
            );
            (lines, mesh.lines.len() as u32)
        });
        let leak_lines = (!mesh.leak_lines.is_empty()).then(|| {
            let lines = buffer(
                "leak path",
                bytemuck::cast_slice(&mesh.leak_lines),
                wgpu::BufferUsages::VERTEX,
            );
            (lines, mesh.leak_lines.len() as u32)
        });

        let (mut opaque, mut translucent) = (Vec::new(), Vec::new());
        let wire_buffer = |label, lines: &[LineVertex]| {
            (!lines.is_empty()).then(|| {
                (
                    buffer(
                        label,
                        bytemuck::cast_slice(lines),
                        wgpu::BufferUsages::VERTEX,
                    ),
                    lines.len() as u32,
                )
            })
        };
        let wire_lines = wire_buffer("brush wireframes", &mesh.wire_lines);
        let tool_wire_lines = wire_buffer("tool brush wireframes", &mesh.tool_wire_lines);
        for &batch in &mesh.batches {
            self.ensure_material(ctx, library, batch.material);
            let info = library.get(batch.material);
            let draw = DrawBatch {
                batch,
                tool: info.tool,
                helper: batch.material == MaterialLibrary::EDITOR || info.helper.is_some(),
            };
            if info.translucent {
                translucent.push(draw);
            } else {
                opaque.push(draw);
            }
        }
        GpuScene {
            vertices,
            indices,
            opaque,
            translucent,
            lines,
            leak_lines,
            wire_lines,
            tool_wire_lines,
        }
    }

    fn ensure_material(&mut self, ctx: &GpuContext, library: &MaterialLibrary, id: MaterialId) {
        if self.materials.len() <= id {
            self.materials.resize(id + 1, None);
        }
        if self.materials[id].is_none() {
            self.materials[id] = Some(self.material_bind_group(ctx, library.get(id)));
        }
    }

    fn material_bind_group(&self, ctx: &GpuContext, info: &MaterialInfo) -> wgpu::BindGroup {
        let (white, missing) = (&self.white, &self.missing);
        let untextured = info.base.is_none() && info.helper.is_none() && !info.missing;
        let base = match (info.helper, &info.base) {
            (Some(helper), _) => upload_helper_texture(ctx, &helper.diffuse, true),
            (_, Some(vtf)) => upload_vtf(ctx, vtf).unwrap_or_else(|| missing.clone()),
            (_, None) if untextured => white.clone(),
            (_, None) => missing.clone(),
        };
        let blend = info
            .blend
            .as_ref()
            .and_then(|vtf| upload_vtf(ctx, vtf))
            .unwrap_or_else(|| base.clone());
        let flag = |on: bool| if on { 1.0f32 } else { 0.0 };
        let params = [
            flag(info.alpha_test),
            flag(info.blend.is_some()),
            flag(info.translucent),
            flag(untextured),
            flag(info.helper.is_some()),
            0.0,
            0.0,
            0.0,
        ];
        let params = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&info.name),
                contents: bytemuck::cast_slice(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let base_view = base.create_view(&Default::default());
        let blend_view = blend.create_view(&Default::default());
        let normal = info
            .helper
            .map(|helper| upload_helper_texture(ctx, &helper.normal, false));
        let surface = info
            .helper
            .map(|helper| upload_helper_texture(ctx, &helper.surface, false));
        let normal_view = normal
            .as_ref()
            .unwrap_or(white)
            .create_view(&Default::default());
        let surface_view = surface
            .as_ref()
            .unwrap_or(white)
            .create_view(&Default::default());
        ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&info.name),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&base_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&blend_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&normal_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&surface_view),
                },
            ],
        })
    }

    fn depth_view(&mut self, ctx: &GpuContext, size: (u32, u32)) -> wgpu::TextureView {
        if let Some((view, current)) = &self.depth
            && *current == size
        {
            return view.clone();
        }
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        self.depth = Some((view.clone(), size));
        view
    }

    /// Draws the scene into `target`, which must be `COLOR_FORMAT`.
    pub fn render(
        &mut self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &Camera,
        options: &ViewOptions,
        target: &wgpu::TextureView,
        size: (u32, u32),
    ) {
        let depth = self.depth_view(ctx, size);
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        let light = glam::Vec3::new(0.35, 0.55, 0.75).normalize();
        let globals = Globals {
            view_proj: camera.view_projection(aspect).to_cols_array_2d(),
            light_dir: light.extend(0.0).to_array(),
            eye: camera.position.extend(1.0).to_array(),
            surface_view: [
                match options.surface {
                    SurfaceView::Textured => 0,
                    SurfaceView::Flat => 3,
                    SurfaceView::Wireframe => 4,
                    SurfaceView::LightmapDensity => 1,
                    SurfaceView::SmoothingGroups => 2,
                },
                options.smoothing_group.clamp(1, 32) - 1,
                0,
                0,
            ],
        };
        ctx.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        let (overlay, grid_count) = crate::grid::overlay_lines(camera, size, options);
        if !overlay.is_empty() {
            ctx.queue
                .write_buffer(&self.overlay, 0, bytemuck::cast_slice(&overlay));
        }

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("map") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("map"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
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
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            if grid_count > 0 {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_vertex_buffer(0, self.overlay.slice(..));
                pass.draw(0..grid_count as u32, 0..1);
            }
            pass.set_vertex_buffer(0, scene.vertices.slice(..));
            pass.set_index_buffer(scene.indices.slice(..), wgpu::IndexFormat::Uint32);
            for (pipeline, batches) in [
                (&self.opaque_pipeline, &scene.opaque),
                (&self.translucent_pipeline, &scene.translucent),
            ] {
                for draw in batches {
                    if options.surface == SurfaceView::Wireframe && !draw.helper {
                        continue;
                    }
                    if !options.tool_textures && draw.tool {
                        continue;
                    }
                    let Some(Some(material)) = self.materials.get(draw.batch.material) else {
                        continue;
                    };
                    pass.set_pipeline(if options.surface == SurfaceView::Textured || draw.helper {
                        pipeline
                    } else {
                        &self.opaque_pipeline
                    });
                    pass.set_bind_group(1, material, &[]);
                    let batch = draw.batch;
                    pass.draw_indexed(
                        batch.first_index..batch.first_index + batch.index_count,
                        0,
                        0..1,
                    );
                }
            }
            if options.surface == SurfaceView::Wireframe {
                for lines in [&scene.wire_lines, &scene.tool_wire_lines]
                    .into_iter()
                    .take(if options.tool_textures { 2 } else { 1 })
                    .flatten()
                {
                    pass.set_pipeline(&self.line_pipeline);
                    pass.set_vertex_buffer(0, lines.0.slice(..));
                    pass.draw(0..lines.1, 0..1);
                }
            }
            if let Some((lines, count)) = &scene.lines {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_vertex_buffer(0, lines.slice(..));
                pass.draw(0..*count, 0..1);
            }
            if overlay.len() > grid_count {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_vertex_buffer(0, self.overlay.slice(..));
                pass.draw(grid_count as u32..overlay.len() as u32, 0..1);
            }
            if options.leak_path
                && let Some((lines, count)) = &scene.leak_lines
            {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_vertex_buffer(0, lines.slice(..));
                pass.draw(0..*count, 0..1);
            }
        }
        ctx.queue.submit([encoder.finish()]);
    }

    /// Renders offscreen and reads the image back as tightly packed RGBA8.
    pub fn render_to_rgba(
        &mut self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &Camera,
        options: &ViewOptions,
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        let target = RenderTarget::new(ctx, width, height);
        self.render(ctx, scene, camera, options, &target.view, (width, height));
        let texture = target.texture;

        let row = width * 4;
        let padded_row =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(padded_row * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            texture.size(),
        );
        ctx.queue.submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |result| {
            result.expect("map readback buffer");
        });
        ctx.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("wait for GPU");
        let mapped = buffer
            .get_mapped_range(..)
            .expect("readback buffer is mapped");
        mapped
            .chunks(padded_row as usize)
            .flat_map(|r| &r[..row as usize])
            .copied()
            .collect()
    }
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_2d(
    ctx: &GpuContext,
    label: &str,
    size: (u32, u32),
    mips: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn write_mip(
    ctx: &GpuContext,
    texture: &wgpu::Texture,
    mip: u32,
    data: &[u8],
    bytes_per_row: u32,
    rows: u32,
    extent: (u32, u32),
) {
    ctx.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: mip,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(rows),
        },
        wgpu::Extent3d {
            width: extent.0,
            height: extent.1,
            depth_or_array_layers: 1,
        },
    );
}

/// Uploads every mip of a VTF's first frame. DXT data goes up as-is when
/// the device supports BC; other formats are decoded to RGBA8.
fn upload_vtf(ctx: &GpuContext, vtf: &Vtf) -> Option<wgpu::Texture> {
    let block_format = match vtf.format {
        ImageFormat::Dxt1 | ImageFormat::Dxt1OneBitAlpha => {
            Some((wgpu::TextureFormat::Bc1RgbaUnormSrgb, 8))
        }
        ImageFormat::Dxt3 => Some((wgpu::TextureFormat::Bc2RgbaUnormSrgb, 16)),
        ImageFormat::Dxt5 => Some((wgpu::TextureFormat::Bc3RgbaUnormSrgb, 16)),
        _ => None,
    }
    .filter(|_| ctx.supports_bc() && vtf.width.is_multiple_of(4) && vtf.height.is_multiple_of(4));
    let mips = u32::from(vtf.mip_count);
    let size = (vtf.width, vtf.height);

    if let Some((format, block_bytes)) = block_format {
        let texture = texture_2d(ctx, "vtf", size, mips, format);
        for mip in 0..vtf.mip_count {
            let data = vtf.image_data(mip, 0, 0).ok()?;
            let (w, h) = vtf.mip_size(mip);
            let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
            write_mip(
                ctx,
                &texture,
                mip.into(),
                data,
                bw * block_bytes,
                bh,
                (bw * 4, bh * 4),
            );
        }
        return Some(texture);
    }

    let texture = texture_2d(ctx, "vtf", size, mips, wgpu::TextureFormat::Rgba8UnormSrgb);
    for mip in 0..vtf.mip_count {
        let image = vtf.decode_rgba8(mip, 0, 0).ok()?;
        write_mip(
            ctx,
            &texture,
            mip.into(),
            &image.pixels,
            image.width * 4,
            image.height,
            (image.width, image.height),
        );
    }
    Some(texture)
}

fn solid_texture(ctx: &GpuContext, rgba: [u8; 4]) -> wgpu::Texture {
    let texture = texture_2d(ctx, "solid", (1, 1), 1, wgpu::TextureFormat::Rgba8UnormSrgb);
    write_mip(ctx, &texture, 0, &rgba, 4, 1, (1, 1));
    texture
}

fn upload_helper_texture(
    ctx: &GpuContext,
    mips: &[source_formats::vtf::RgbaImage],
    srgb: bool,
) -> wgpu::Texture {
    let first = &mips[0];
    let format = if srgb {
        wgpu::TextureFormat::Rgba8UnormSrgb
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    };
    let texture = texture_2d(
        ctx,
        "helper",
        (first.width, first.height),
        mips.len() as u32,
        format,
    );
    for (level, image) in mips.iter().enumerate() {
        write_mip(
            ctx,
            &texture,
            level as u32,
            &image.pixels,
            image.width * 4,
            image.height,
            (image.width, image.height),
        );
    }
    texture
}

/// The engine's missing-texture pattern: purple and black squares.
fn checkerboard_texture(ctx: &GpuContext) -> wgpu::Texture {
    const SIZE: u32 = 64;
    let pixels: Vec<u8> = (0..SIZE * SIZE)
        .flat_map(|i| {
            let (x, y) = (i % SIZE / 32, i / SIZE / 32);
            if (x + y) % 2 == 0 {
                [255, 0, 255, 255]
            } else {
                [0, 0, 0, 255]
            }
        })
        .collect();
    let texture = texture_2d(
        ctx,
        "missing",
        (SIZE, SIZE),
        1,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    );
    write_mip(ctx, &texture, 0, &pixels, SIZE * 4, SIZE, (SIZE, SIZE));
    texture
}

#[cfg(test)]
mod tests {
    #[test]
    fn surface_shader_parses_and_validates_without_a_gpu() {
        use wgpu::naga::{front::wgsl, valid};

        let source = include_str!("shader.wgsl");
        let module = wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
        valid::Validator::new(valid::ValidationFlags::all(), valid::Capabilities::empty())
            .validate(&module)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
    }
}
