use std::sync::Arc;

use super::*;
use crate::color_correction::{Lut, RAW_BYTES};

fn pixel(ctx: &GpuContext, state: &mut State, preview: &Preview, input: [u8; 4]) -> [u8; 4] {
    state.prepare(ctx, preview, (1, 1)).unwrap();
    let texture = &state.resources.as_ref().unwrap().target.texture;
    let display = texture.create_view(&wgpu::TextureViewDescriptor {
        format: Some(wgpu::TextureFormat::Rgba8Unorm),
        ..Default::default()
    });
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &display,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: f64::from(input[0]) / 255.0,
                        g: f64::from(input[1]) / 255.0,
                        b: f64::from(input[2]) / 255.0,
                        a: f64::from(input[3]) / 255.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }
    ctx.queue.submit([encoder.finish()]);
    let output = RenderTarget::new(ctx, 1, 1);
    state.draw(ctx, &output.view).unwrap();
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("color-correction literal pixel readback"),
        size: u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        output.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT),
                rows_per_image: Some(1),
            },
        },
        output.texture.size(),
    );
    ctx.queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
    ctx.device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(5)),
        })
        .unwrap();
    let mapped = buffer.get_mapped_range(..).unwrap();
    mapped[..4].try_into().unwrap()
}

fn close(actual: [u8; 4], expected: [u8; 4]) {
    for channel in 0..3 {
        assert!(
            actual[channel].abs_diff(expected[channel]) <= 1,
            "{actual:?} != {expected:?}"
        );
    }
    assert_eq!(actual[3], expected[3]);
}

#[test]
#[ignore = "requires a real GPU; run the color-correction acceptance gate explicitly"]
fn gpu_literal_axes_trilinear_display_weight_identity_and_alpha() {
    let ctx = GpuContext::new().unwrap();
    let mut state = State::default();
    let mut raw = vec![0; RAW_BYTES];
    raw[93..96].copy_from_slice(&[17, 43, 91]);
    raw[2976..2979].copy_from_slice(&[61, 127, 193]);
    raw[95232..95235].copy_from_slice(&[211, 83, 29]);
    let preview = Preview {
        lut: Arc::new(Lut::from_raw(raw).unwrap()),
        weight: 1.0,
    };
    close(
        pixel(&ctx, &mut state, &preview, [255, 0, 0, 73]),
        [17, 43, 91, 73],
    );
    close(
        pixel(&ctx, &mut state, &preview, [0, 255, 0, 73]),
        [61, 127, 193, 73],
    );
    close(
        pixel(&ctx, &mut state, &preview, [0, 0, 255, 73]),
        [211, 83, 29, 73],
    );

    let mut raw = vec![0; RAW_BYTES];
    raw[98301..98304].copy_from_slice(&[240, 120, 60]);
    let preview = Preview {
        lut: Arc::new(Lut::from_raw(raw).unwrap()),
        weight: 1.0,
    };
    close(
        pixel(&ctx, &mut state, &preview, [251, 251, 251, 73]),
        [33, 16, 8, 73],
    );

    let raw = [240, 120, 60].repeat(32768);
    let mut preview = Preview {
        lut: Arc::new(Lut::from_raw(raw).unwrap()),
        weight: 0.0,
    };
    assert_eq!(
        pixel(&ctx, &mut state, &preview, [64, 128, 192, 73]),
        [64, 128, 192, 73]
    );
    preview.weight = 0.5;
    close(
        pixel(&ctx, &mut state, &preview, [64, 128, 192, 73]),
        [152, 124, 126, 73],
    );
    preview.weight = 1.0;
    close(
        pixel(&ctx, &mut state, &preview, [64, 128, 192, 73]),
        [240, 120, 60, 73],
    );

    let mut identity = Vec::with_capacity(RAW_BYTES);
    for blue in 0..32 {
        for green in 0..32 {
            for red in 0..32 {
                identity.extend_from_slice(&[
                    ((red * 255 + 15) / 31) as u8,
                    ((green * 255 + 15) / 31) as u8,
                    ((blue * 255 + 15) / 31) as u8,
                ]);
            }
        }
    }
    let preview = Preview {
        lut: Arc::new(Lut::from_raw(identity).unwrap()),
        weight: 1.0,
    };
    close(
        pixel(&ctx, &mut state, &preview, [64, 128, 192, 73]),
        [64, 128, 192, 73],
    );
    assert!(state.prepare(&ctx, &preview, (4097, 1)).is_err());
    state.disable(&ctx).unwrap();
    assert!(state.resources.is_none());
}
