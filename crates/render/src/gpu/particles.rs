use std::ops::Range;

use crate::icons::Blend;
use crate::particle_preview::{self, MAX_QUADS};

use super::{GpuContext, IconVertex};

pub(super) struct Emitter {
    pub(super) source: particle_preview::Emitter,
    pub(super) binding: wgpu::BindGroup,
}

pub(super) struct Draw<'a> {
    pub(super) binding: &'a wgpu::BindGroup,
    pub(super) vertices: Range<u32>,
    pub(super) blend: Blend,
}

pub(super) fn prepare<'a>(
    ctx: &GpuContext,
    emitters: &'a [Emitter],
    camera: &crate::Camera,
    time: f32,
    buffer: &mut Option<wgpu::Buffer>,
) -> Result<Vec<Draw<'a>>, String> {
    const MAX_VERTICES: usize = MAX_QUADS * 6;
    const QUAD_UVS: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let mut vertices = Vec::new();
    let mut draws = Vec::new();
    let eye = camera.position;
    let mut ordered = emitters.iter().collect::<Vec<_>>();
    super::sort_back_to_front(&mut ordered, camera, |emitter| emitter.source.origin.into());
    for emitter in ordered {
        let first = vertices.len() as u32;
        let blend = emitter.source.asset.blend();
        let mut samples = emitter
            .source
            .sample(camera, time)
            .map_err(|error| format!("Particle source entity {}: {error}", emitter.source.seed))?;
        if blend == Blend::Alpha {
            let distance = |quad: &particle_preview::Quad| eye.distance_squared(quad.center.into());
            samples.sort_by(|a, b| distance(b).total_cmp(&distance(a)));
        }
        for quad in samples {
            if eye.distance_squared(quad.center.into()) < (quad.radius * 0.5).powi(2) {
                continue;
            }
            if vertices.len() == MAX_VERTICES {
                return Err("Particle frame exceeds its vertex budget".into());
            }
            let [red, green, blue] = quad.color;
            vertices.extend([2, 1, 0, 2, 0, 3].map(|index| IconVertex {
                center: quad.corners[index],
                offset: [0.0; 2],
                uv: QUAD_UVS[index],
                opacity: quad.opacity,
                color: [red, green, blue, 255],
            }));
        }
        if first != vertices.len() as u32 {
            draws.push(Draw {
                binding: &emitter.binding,
                vertices: first..vertices.len() as u32,
                blend,
            });
        }
    }
    if !vertices.is_empty() {
        let buffer = buffer.get_or_insert_with(|| {
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Particle preview vertices"),
                size: (MAX_VERTICES * size_of::<IconVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        ctx.queue
            .write_buffer(buffer, 0, bytemuck::cast_slice(&vertices));
    }
    Ok(draws)
}
