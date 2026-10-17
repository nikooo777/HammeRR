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
    fog_color: [f32; 4],
    fog_range: [f32; 4],
    forward: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    sky_params: [f32; 4],
    lighting_settings: [f32; 4],
    lighting_lights: [crate::lighting::Light; crate::lighting::MAX_LIGHTS],
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintGuide {
    pub center: glam::Vec2,
    pub radius: f32,
    pub size: (u32, u32),
    pub camera: Camera,
}

/// View filters and resource admission options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewOptions {
    pub lighting_preview: bool,
    pub lighting_quality: crate::lighting::Quality,
    pub lighting_exposure_ev: f32,
    pub normal_maps: bool,
    pub surface: SurfaceView,
    pub smoothing_group: u32,
    pub tool_textures: bool,
    pub sky_preview: bool,
    pub sky_channel: crate::sky::Channel,
    pub sky_exposure_ev: f32,
    pub rope_preview: bool,
    pub selection_outlines: bool,
    pub entity_images: bool,
    pub additive_sprite_guides: bool,
    pub entity_model_guides: bool,
    pub fog: Option<crate::fog::Fog>,
    pub leak_path: bool,
    pub grid_visible: bool,
    pub grid_size: f32,
    pub grid_elevation: f32,
    pub block_preview: Option<[glam::Vec3; 2]>,
    pub clip_preview: Option<ClipGuide>,
    pub transform_handles: Option<crate::handles::Guide>,
    pub handle_pixel_ratio: f32,
    pub area_preview: Option<AreaGuide>,
    pub paint_preview: Option<PaintGuide>,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            surface: SurfaceView::Textured,
            normal_maps: false,
            lighting_preview: false,
            lighting_quality: crate::lighting::Quality::Detail,
            lighting_exposure_ev: 0.0,
            smoothing_group: 1,
            tool_textures: true,
            sky_preview: false,
            sky_channel: crate::sky::Channel::Ldr,
            sky_exposure_ev: 0.0,
            rope_preview: false,
            selection_outlines: true,
            entity_images: true,
            additive_sprite_guides: false,
            entity_model_guides: true,
            fog: None,
            leak_path: true,
            grid_visible: true,
            grid_size: 16.0,
            grid_elevation: 0.0,
            block_preview: None,
            clip_preview: None,
            transform_handles: None,
            handle_pixel_ratio: 1.0,
            area_preview: None,
            paint_preview: None,
        }
    }
}

pub type SceneKey = (
    u64,
    crate::sky::Channel,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
);

impl ViewOptions {
    pub fn scene_key(self, serial: u64) -> SceneKey {
        (
            serial,
            self.sky_channel,
            self.entity_images,
            self.entity_model_guides,
            self.normal_maps,
            self.normal_maps && self.surface == SurfaceView::Textured,
            self.lighting_preview,
            self.lighting_preview && self.surface == SurfaceView::Textured,
            self.additive_sprite_guides,
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct DrawBatch {
    batch: Batch,
    center: glam::Vec3,
    tool: bool,
    helper: bool,
    sky: bool,
}

fn translucent_order<'a>(
    batches: &'a [DrawBatch],
    camera: &Camera,
) -> std::borrow::Cow<'a, [DrawBatch]> {
    if !batches.iter().any(|draw| draw.batch.model_translucent) {
        return std::borrow::Cow::Borrowed(batches);
    }
    let mut ordered = batches.to_vec();
    let depth = |draw: &DrawBatch| (draw.center - camera.position).dot(camera.forward());
    ordered.sort_by(|a, b| depth(b).total_cmp(&depth(a)));
    std::borrow::Cow::Owned(ordered)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct IconVertex {
    center: [f32; 3],
    offset: [f32; 2],
    uv: [f32; 2],
}
struct IconDraw {
    center: glam::Vec3,
    first_vertex: u32,
    binding: wgpu::BindGroup,
    blend: crate::icons::Blend,
}

/// Geometry uploaded by a [`Renderer`]; draw it with the same renderer.
pub struct GpuScene {
    lighting: crate::lighting::Preview,
    icons: Option<wgpu::Buffer>,
    icon_draws: Vec<IconDraw>,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    skybox: Option<wgpu::BindGroup>,
    sky_channel: crate::sky::Channel,
    opaque: Vec<DrawBatch>,
    translucent: Vec<DrawBatch>,
    lines: Option<(wgpu::Buffer, u32)>,
    guide_line_ranges: Vec<std::ops::Range<u32>>,
    leak_lines: Option<(wgpu::Buffer, u32)>,
    wire_lines: Option<(wgpu::Buffer, u32)>,
    tool_wire_lines: Option<(wgpu::Buffer, u32)>,
    rope_lines: Option<(wgpu::Buffer, u32)>,
}

pub struct Renderer {
    globals: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    icon_sampler: wgpu::Sampler,
    icon_pipeline: wgpu::RenderPipeline,
    additive_icon_pipeline: wgpu::RenderPipeline,
    icon_bindings: std::collections::HashMap<u64, wgpu::BindGroup>,
    opaque_pipeline: wgpu::RenderPipeline,
    translucent_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    sky_layout: wgpu::BindGroupLayout,
    sky_sampler: wgpu::Sampler,
    skyboxes:
        std::collections::HashMap<(String, crate::sky::Channel, u64), Option<wgpu::BindGroup>>,
    /// Bind groups by [`MaterialId`], uploaded on first use.
    materials: Vec<Option<wgpu::BindGroup>>,
    material_generation: Option<u64>,
    white: wgpu::Texture,
    missing: wgpu::Texture,
    depth: Option<(wgpu::TextureView, (u32, u32))>,
    overlay: wgpu::Buffer,
}

impl Renderer {
    /// Material IDs are local to a library; clear bindings when replacing it.
    pub fn clear_materials(&mut self) {
        self.material_generation = None;
        self.materials.clear();
        self.skyboxes.clear();
        self.icon_bindings.clear();
    }

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

        let icon_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("FGD icon image edges"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let sky_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("2D sky"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                texture_entry(2),
                texture_entry(3),
                texture_entry(4),
                texture_entry(5),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform_entry(7, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let sky_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("2D sky edges"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let sky_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("2D sky"),
            bind_group_layouts: &[Some(&globals_layout), Some(&sky_layout)],
            immediate_size: 0,
        });
        let sky_shader = device.create_shader_module(wgpu::include_wgsl!("sky.wgsl"));
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("map"),
            bind_group_layouts: &[Some(&globals_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let pipeline = |label,
                        blend,
                        depth_write,
                        layout: &wgpu::PipelineLayout,
                        shader: &wgpu::ShaderModule| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
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
                            6 => Float32,
                            7 => Uint32,
                            8 => Float32x4,
                            9 => Uint32,
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
                    module: shader,
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
        let opaque_pipeline = pipeline("opaque", None, true, &layout, &shader);
        let sky_pipeline = pipeline("2D sky", None, true, &sky_pipeline_layout, &sky_shader);
        let translucent_pipeline = pipeline(
            "translucent",
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
            &layout,
            &shader,
        );

        let icon_shader = device.create_shader_module(wgpu::include_wgsl!("icons.wgsl"));
        let icon_pipeline = |label, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &icon_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<IconVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x2],
                })],
            },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &icon_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
        };
        let additive_icon_pipeline = icon_pipeline(
            "Authored Additive Sprite guides",
            wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            },
        );
        let icon_pipeline = icon_pipeline("FGD editor icons", wgpu::BlendState::ALPHA_BLENDING);

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
            icon_sampler,
            icon_pipeline,
            additive_icon_pipeline,
            icon_bindings: std::collections::HashMap::new(),
            opaque_pipeline,
            translucent_pipeline,
            line_pipeline,
            sky_pipeline,
            sky_layout,
            sky_sampler,
            skyboxes: std::collections::HashMap::new(),
            materials: Vec::new(),
            material_generation: None,
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
        if self.material_generation != Some(library.generation()) {
            self.clear_materials();
            self.material_generation = Some(library.generation());
        }
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
        let rope_lines = wire_buffer("rope selection", &mesh.rope_lines);
        for &batch in &mesh.batches {
            self.ensure_material(ctx, library, batch.material);
            let info = library.get(batch.material);
            let range =
                batch.first_index as usize..batch.first_index as usize + batch.index_count as usize;
            let (mins, maxs) = mesh.indices[range]
                .iter()
                .map(|&index| glam::Vec3::from_array(mesh.vertices[index as usize].position))
                .fold((glam::Vec3::MAX, glam::Vec3::MIN), |(mins, maxs), point| {
                    (mins.min(point), maxs.max(point))
                });
            let draw = DrawBatch {
                batch,
                center: (mins + maxs) * 0.5,
                tool: info.tool,
                sky: info.sky,
                helper: batch.material == MaterialLibrary::EDITOR || info.helper.is_some(),
            };
            if info.translucent || batch.model_translucent {
                translucent.push(draw);
            } else {
                opaque.push(draw);
            }
        }
        let skybox = mesh
            .skybox
            .as_ref()
            .filter(|skybox| skybox.ready())
            .and_then(|skybox| {
                let key = (skybox.name.clone(), skybox.channel, skybox.payload_id);
                if !self.skyboxes.contains_key(&key) {
                    if skybox.channel == crate::sky::Channel::Hdr {
                        self.skyboxes
                            .retain(|(_, channel, _), _| *channel != crate::sky::Channel::Hdr);
                    }
                    let binding = self.sky_bind_group(ctx, skybox);
                    self.skyboxes.insert(key.clone(), binding);
                }
                self.skyboxes.get(&key).cloned().flatten()
            });
        let mut icon_vertices = Vec::new();
        let mut icon_draws = Vec::new();
        for instance in mesh.icons.iter().take(crate::icons::MAX_INSTANCES) {
            let Some(material) = &instance.asset.material else {
                continue;
            };
            let id = instance.asset.id;
            if !self.icon_bindings.contains_key(&id) {
                if self.icon_bindings.len() >= crate::icons::MAX_ASSETS {
                    self.icon_bindings.clear();
                }
                let binding =
                    self.material_bind_group_with_sampler(ctx, material, false, &self.icon_sampler);
                self.icon_bindings.insert(id, binding);
            }
            let half = instance.world_size.map(|size| size * 0.5);
            let first_vertex = icon_vertices.len() as u32;
            for (offset, uv) in [
                ([-half[0], half[1]], [0.0, 0.0]),
                ([-half[0], -half[1]], [0.0, 1.0]),
                ([half[0], -half[1]], [1.0, 1.0]),
                ([-half[0], half[1]], [0.0, 0.0]),
                ([half[0], -half[1]], [1.0, 1.0]),
                ([half[0], half[1]], [1.0, 0.0]),
            ] {
                icon_vertices.push(IconVertex {
                    center: instance.center,
                    offset,
                    uv,
                });
            }
            icon_draws.push(IconDraw {
                center: instance.center.into(),
                first_vertex,
                binding: self.icon_bindings[&id].clone(),
                blend: instance.blend,
            });
        }
        let icons = (!icon_vertices.is_empty()).then(|| {
            ctx.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("FGD icon instances"),
                    contents: bytemuck::cast_slice(&icon_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });
        GpuScene {
            lighting: mesh.lighting.clone(),
            icons,
            icon_draws,
            skybox,
            sky_channel: mesh
                .skybox
                .as_ref()
                .map_or(crate::sky::Channel::Ldr, |sky| sky.channel),
            vertices,
            indices,
            opaque,
            translucent,
            lines,
            guide_line_ranges: mesh.guide_line_ranges(),
            leak_lines,
            wire_lines,
            tool_wire_lines,
            rope_lines,
        }
    }

    fn sky_bind_group(
        &self,
        ctx: &GpuContext,
        skybox: &crate::sky::Skybox,
    ) -> Option<wgpu::BindGroup> {
        let mut hdr_textures = std::collections::HashMap::new();
        let textures = skybox
            .faces
            .iter()
            .map(|face| {
                if skybox.channel == crate::sky::Channel::Ldr {
                    return upload_vtf(ctx, face.texture.as_ref()?);
                }
                let path = face.texture_path.as_ref()?;
                if let Some(texture) = hdr_textures.get(path) {
                    return Some(wgpu::Texture::clone(texture));
                }
                let texture = upload_hdr_vtf(ctx, face.image()?)?;
                hdr_textures.insert(path.clone(), texture.clone());
                Some(texture)
            })
            .collect::<Option<Vec<_>>>()?;
        let views = textures
            .iter()
            .map(|texture| texture.create_view(&Default::default()))
            .collect::<Vec<_>>();
        let mut entries = views
            .iter()
            .enumerate()
            .map(|(binding, view)| wgpu::BindGroupEntry {
                binding: binding as u32,
                resource: wgpu::BindingResource::TextureView(view),
            })
            .collect::<Vec<_>>();
        entries.push(wgpu::BindGroupEntry {
            binding: 6,
            resource: wgpu::BindingResource::Sampler(&self.sky_sampler),
        });
        let transforms = skybox.faces.each_ref().map(|face| face.transform);
        let params = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("sky texture transforms"),
                contents: bytemuck::cast_slice(&transforms),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        entries.push(wgpu::BindGroupEntry {
            binding: 7,
            resource: params.as_entire_binding(),
        });
        Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&skybox.name),
            layout: &self.sky_layout,
            entries: &entries,
        }))
    }

    fn ensure_material(&mut self, ctx: &GpuContext, library: &MaterialLibrary, id: MaterialId) {
        if self.materials.len() <= id {
            self.materials.resize(id + 1, None);
        }
        if self.materials[id].is_none() {
            self.materials[id] =
                Some(self.material_bind_group(ctx, library.get(id), id == MaterialLibrary::EDITOR));
        }
    }

    fn material_bind_group(
        &self,
        ctx: &GpuContext,
        info: &MaterialInfo,
        editor: bool,
    ) -> wgpu::BindGroup {
        self.material_bind_group_with_sampler(ctx, info, editor, &self.sampler)
    }

    fn material_bind_group_with_sampler(
        &self,
        ctx: &GpuContext,
        info: &MaterialInfo,
        editor: bool,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
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
        let normal_asset = info.normal.as_ref().filter(|asset| asset.image.is_some());
        let transform = normal_asset.map_or([1.0, 1.0, 0.0, 0.0], |asset| asset.report.transform);
        let params = [
            flag(info.alpha_test),
            flag(info.blend.is_some()),
            flag(info.translucent),
            flag(untextured),
            flag(info.helper.is_some()),
            flag(editor || info.sky),
            flag(normal_asset.is_some()),
            0.0,
            transform[0],
            transform[1],
            transform[2],
            transform[3],
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
            .map(|helper| upload_helper_texture(ctx, &helper.normal, false))
            .or_else(|| {
                normal_asset
                    .and_then(|asset| asset.image.as_ref())
                    .map(|image| upload_normal(ctx, image))
            });
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
                    resource: wgpu::BindingResource::Sampler(sampler),
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
        let (fog_color, fog_range) = options
            .fog
            .filter(|_| {
                options.surface == SurfaceView::Textured
                    && camera.projection == crate::Projection::Perspective
            })
            .map(crate::fog::Fog::uniforms)
            .unwrap_or(([0.0; 4], [0.0; 4]));
        let mut lighting_lights = [crate::lighting::Light::default(); crate::lighting::MAX_LIGHTS];
        let lighting_active = options.lighting_preview
            && options.surface == SurfaceView::Textured
            && scene.lighting.report.active
            && scene.lighting.lights.len() <= crate::lighting::MAX_LIGHTS;
        if lighting_active {
            lighting_lights[..scene.lighting.lights.len()].copy_from_slice(&scene.lighting.lights);
        }
        let globals = Globals {
            lighting_settings: [
                scene.lighting.lights.len() as f32,
                if options.lighting_quality == crate::lighting::Quality::Fast {
                    0.0
                } else {
                    1.0
                },
                if lighting_active { 1.0 } else { 0.0 },
                2f32.powf(options.lighting_exposure_ev) / 255.0,
            ],
            lighting_lights,
            view_proj: camera.view_projection(aspect).to_cols_array_2d(),
            light_dir: light.extend(0.0).to_array(),
            eye: camera.position.extend(1.0).to_array(),
            fog_color,
            fog_range,
            forward: camera.forward().extend(0.0).to_array(),
            right: camera.right().extend(0.0).to_array(),
            up: camera.up().extend(0.0).to_array(),
            sky_params: [
                if options.sky_channel == crate::sky::Channel::Hdr {
                    1.0
                } else {
                    0.0
                },
                2f32.powf(options.sky_exposure_ev),
                0.0,
                0.0,
            ],
            surface_view: [
                match options.surface {
                    SurfaceView::Textured => 0,
                    SurfaceView::Flat => 3,
                    SurfaceView::Wireframe => 4,
                    SurfaceView::LightmapDensity => 1,
                    SurfaceView::SmoothingGroups => 2,
                },
                options.smoothing_group.clamp(1, 32) - 1,
                u32::from(options.normal_maps && options.surface == SurfaceView::Textured),
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
            let translucent = translucent_order(&scene.translucent, camera);
            for (pipeline, batches) in [
                (&self.opaque_pipeline, scene.opaque.as_slice()),
                (&self.translucent_pipeline, translucent.as_ref()),
            ] {
                for draw in batches {
                    if draw.batch.rope && !options.rope_preview {
                        continue;
                    }
                    if options.surface == SurfaceView::Wireframe && !draw.helper {
                        continue;
                    }
                    if !options.tool_textures && draw.tool {
                        continue;
                    }
                    let Some(Some(material)) = self.materials.get(draw.batch.material) else {
                        continue;
                    };
                    if draw.sky
                        && options.sky_preview
                        && scene.sky_channel == options.sky_channel
                        && options.surface == SurfaceView::Textured
                        && camera.projection == crate::Projection::Perspective
                        && let Some(skybox) = &scene.skybox
                    {
                        pass.set_pipeline(&self.sky_pipeline);
                        pass.set_bind_group(1, skybox, &[]);
                        let batch = draw.batch;
                        pass.draw_indexed(
                            batch.first_index..batch.first_index + batch.index_count,
                            0,
                            0..1,
                        );
                        continue;
                    }
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
            if let Some(icons) = &scene.icons {
                pass.set_pipeline(&self.icon_pipeline);
                pass.set_vertex_buffer(0, icons.slice(..));
                let mut draws = scene.icon_draws.iter().collect::<Vec<_>>();
                let depth =
                    |draw: &&IconDraw| (draw.center - camera.position).dot(camera.forward());
                draws.sort_by(|a, b| depth(b).total_cmp(&depth(a)));
                let mut blend = crate::icons::Blend::Alpha;
                for draw in draws {
                    if draw.blend != blend {
                        pass.set_pipeline(match draw.blend {
                            crate::icons::Blend::Alpha => &self.icon_pipeline,
                            crate::icons::Blend::Additive => &self.additive_icon_pipeline,
                        });
                        blend = draw.blend;
                    }
                    pass.set_bind_group(1, &draw.binding, &[]);
                    pass.draw(draw.first_vertex..draw.first_vertex + 6, 0..1);
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
                if options.selection_outlines {
                    pass.draw(0..*count, 0..1);
                } else {
                    for range in &scene.guide_line_ranges {
                        pass.draw(range.clone(), 0..1);
                    }
                }
            }
            if options.rope_preview
                && options.selection_outlines
                && let Some((lines, count)) = &scene.rope_lines
            {
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

fn upload_hdr_vtf(ctx: &GpuContext, vtf: &Vtf) -> Option<wgpu::Texture> {
    let texture = texture_2d(
        ctx,
        "linear HDR sky",
        (vtf.width, vtf.height),
        u32::from(vtf.mip_count),
        wgpu::TextureFormat::Rgba16Float,
    );
    for mip in 0..vtf.mip_count {
        let bytes = crate::sky::hdr_mip_bytes(vtf, mip).ok()?;
        let (width, height) = vtf.mip_size(mip);
        write_mip(
            ctx,
            &texture,
            mip.into(),
            &bytes,
            width * 8,
            height,
            (width, height),
        );
    }
    Some(texture)
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

fn upload_normal(ctx: &GpuContext, image: &source_formats::vtf::RgbaImage) -> wgpu::Texture {
    let texture = texture_2d(
        ctx,
        "brush normal map",
        (image.width, image.height),
        1,
        wgpu::TextureFormat::Rgba8Unorm,
    );
    write_mip(
        ctx,
        &texture,
        0,
        &image.pixels,
        image.width * 4,
        image.height,
        (image.width, image.height),
    );
    texture
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
    fn transparent_model_order_tracks_render_camera_and_keeps_normal_order() {
        use super::*;
        let batch = |center, model_translucent| DrawBatch {
            batch: Batch {
                material: 0,
                first_index: 0,
                index_count: 3,
                rope: false,
                model_translucent,
            },
            center,
            tool: false,
            helper: false,
            sky: false,
        };
        let above = Camera::looking_at(glam::Vec3::new(0.0, 0.0, 64.0), glam::Vec3::ZERO);
        let below = Camera::looking_at(glam::Vec3::new(0.0, 0.0, -64.0), glam::Vec3::ZERO);
        let near = batch(glam::Vec3::new(0.0, 0.0, 16.0), true);
        let far = batch(glam::Vec3::new(0.0, 0.0, -16.0), true);
        let models = [near, far];
        let ordered = translucent_order(&models, &above);
        assert_eq!(ordered[0].center, far.center);
        let reversed = translucent_order(&models, &below);
        assert_eq!(reversed[0].center, near.center);
        let normal = [batch(near.center, false), batch(far.center, false)];
        assert_eq!(translucent_order(&normal, &above)[0].center, near.center);
    }

    #[test]
    fn surface_and_sky_shaders_parse_and_validate_without_a_gpu() {
        use wgpu::naga::{front::wgsl, valid};

        for source in [
            include_str!("shader.wgsl"),
            include_str!("sky.wgsl"),
            include_str!("icons.wgsl"),
        ] {
            let module = wgsl::parse_str(source)
                .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
            valid::Validator::new(valid::ValidationFlags::all(), valid::Capabilities::empty())
                .validate(&module)
                .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
        }
    }
}
