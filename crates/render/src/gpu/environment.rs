use source_formats::vtf::Vtf;

use super::GpuContext;

const CUBE_FACES: u32 = 6;

pub(super) fn empty(ctx: &GpuContext) -> wgpu::TextureView {
    let texture = cube_texture(ctx, 1, 1);
    write_faces(
        ctx,
        &texture,
        0,
        0,
        1,
        CUBE_FACES,
        &[0; 4 * CUBE_FACES as usize],
    );
    cube_view(&texture)
}

pub(super) fn upload(ctx: &GpuContext, source: &Vtf) -> Result<wgpu::TextureView, String> {
    let texture = cube_texture(ctx, source.width, u32::from(source.mip_count));
    for mip in 0..source.mip_count {
        for face in 0..CUBE_FACES {
            let image = source
                .decode_rgba8(mip, 0, face as u8)
                .map_err(|error| format!("Environment cube mip {mip}, face {face}: {error}"))?;
            write_faces(
                ctx,
                &texture,
                u32::from(mip),
                face,
                image.width,
                1,
                &image.pixels,
            );
        }
    }
    Ok(cube_view(&texture))
}

fn cube_texture(ctx: &GpuContext, edge: u32, mips: u32) -> wgpu::Texture {
    ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("environment cube"),
        size: wgpu::Extent3d {
            width: edge,
            height: edge,
            depth_or_array_layers: CUBE_FACES,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn cube_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    })
}

fn write_faces(
    ctx: &GpuContext,
    texture: &wgpu::Texture,
    mip: u32,
    first_face: u32,
    edge: u32,
    faces: u32,
    bytes: &[u8],
) {
    ctx.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: mip,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: first_face,
            },
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(edge * 4),
            rows_per_image: Some(edge),
        },
        wgpu::Extent3d {
            width: edge,
            height: edge,
            depth_or_array_layers: faces,
        },
    );
}
