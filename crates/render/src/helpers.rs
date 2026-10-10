use std::io::Cursor;
use std::sync::OnceLock;

use glam::Vec3;
use source_formats::vtf::RgbaImage;

use crate::scene::Vertex;

pub(crate) struct HelperModel {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub mins: Vec3,
    pub maxs: Vec3,
}

#[derive(Debug)]
pub(crate) struct HelperMaterial {
    pub diffuse: Vec<RgbaImage>,
    pub normal: Vec<RgbaImage>,
    pub surface: Vec<RgbaImage>,
}

pub(crate) fn bulb_model() -> &'static HelperModel {
    static MODEL: OnceLock<HelperModel> = OnceLock::new();
    MODEL.get_or_init(|| {
        let bytes = include_bytes!("../assets/lightbulb/bulb.mesh");
        assert_eq!(&bytes[..4], b"HRM1");
        let count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let index_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let end = 12 + count * size_of::<Vertex>();
        assert_eq!(bytes.len(), end + index_count * size_of::<u32>());
        let vertices: Vec<Vertex> = bytes[12..end]
            .as_chunks::<{ size_of::<Vertex>() }>()
            .0
            .iter()
            .map(|chunk| {
                let floats: [f32; 8] = std::array::from_fn(|i| {
                    f32::from_le_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap())
                });
                Vertex {
                    position: floats[..3].try_into().unwrap(),
                    normal: floats[3..6].try_into().unwrap(),
                    uv: floats[6..8].try_into().unwrap(),
                    color: chunk[32..36].try_into().unwrap(),
                }
            })
            .collect();
        let indices = bytes[end..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| u32::from_le_bytes(*chunk))
            .collect();
        let (mins, maxs) = vertices.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), v| {
            let p = Vec3::from_array(v.position);
            (lo.min(p), hi.max(p))
        });
        HelperModel {
            vertices,
            indices,
            mins,
            maxs,
        }
    })
}

pub(crate) fn bulb_material() -> &'static HelperMaterial {
    static MATERIAL: OnceLock<HelperMaterial> = OnceLock::new();
    MATERIAL.get_or_init(|| HelperMaterial {
        diffuse: texture_mips(include_bytes!("../assets/lightbulb/diffuse.png")),
        normal: texture_mips(include_bytes!("../assets/lightbulb/normal.png")),
        surface: texture_mips(include_bytes!("../assets/lightbulb/arm.png")),
    })
}

fn texture_mips(bytes: &[u8]) -> Vec<RgbaImage> {
    let mut reader = png::Decoder::new(Cursor::new(bytes))
        .read_info()
        .expect("bundled helper texture");
    let mut pixels = vec![0; reader.output_buffer_size().expect("texture size")];
    let info = reader
        .next_frame(&mut pixels)
        .expect("helper texture pixels");
    assert_eq!(info.color_type, png::ColorType::Rgba);
    pixels.truncate(info.buffer_size());
    let mut mips = vec![RgbaImage {
        width: info.width,
        height: info.height,
        pixels,
    }];
    while mips.last().is_some_and(|m| m.width > 1 || m.height > 1) {
        let previous = mips.last().unwrap();
        let width = (previous.width / 2).max(1);
        let height = (previous.height / 2).max(1);
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                for channel in 0..4 {
                    let mut sum = 0u16;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let px = (x * 2 + dx).min(previous.width - 1);
                            let py = (y * 2 + dy).min(previous.height - 1);
                            sum += u16::from(
                                previous.pixels
                                    [((py * previous.width + px) * 4 + channel) as usize],
                            );
                        }
                    }
                    pixels.push(((sum + 2) / 4) as u8);
                }
            }
        }
        mips.push(RgbaImage {
            width,
            height,
            pixels,
        });
    }
    mips
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_bulb_has_valid_bounds_normals_and_clockwise_faces() {
        let model = bulb_model();
        assert!(model.vertices.len() > 1000);
        assert_eq!(model.indices.len() % 3, 0);
        assert!(model.mins.cmpge(Vec3::splat(-18.0)).all());
        assert!(model.maxs.cmple(Vec3::splat(18.0)).all());
        assert!((model.maxs.z - model.mins.z - 32.0).abs() < 0.001);
        for vertex in &model.vertices {
            assert!(
                vertex
                    .position
                    .iter()
                    .chain(&vertex.uv)
                    .all(|v| v.is_finite())
            );
            assert!((Vec3::from_array(vertex.normal).length() - 1.0).abs() < 0.001);
        }
        for triangle in model.indices.as_chunks::<3>().0 {
            let [a, b, c] = std::array::from_fn(|i| model.vertices[triangle[i] as usize]);
            let normal = (Vec3::from_array(a.position) - Vec3::from_array(b.position))
                .cross(Vec3::from_array(c.position) - Vec3::from_array(b.position));
            let smooth = Vec3::from_array(a.normal)
                + Vec3::from_array(b.normal)
                + Vec3::from_array(c.normal);
            assert!(normal.dot(smooth) >= -0.0001);
        }
        assert!(model.vertices.iter().any(|v| v.color[3] == 255));
        assert!(model.vertices.iter().any(|v| v.color[3] == 0));
    }

    #[test]
    fn bundled_bulb_textures_have_complete_mip_chains() {
        let material = bulb_material();
        for mips in [&material.diffuse, &material.normal, &material.surface] {
            assert_eq!((mips[0].width, mips[0].height), (512, 512));
            assert_eq!(mips.len(), 10);
            assert_eq!((mips[9].width, mips[9].height), (1, 1));
            for mip in mips {
                assert_eq!(mip.pixels.len(), (mip.width * mip.height * 4) as usize);
            }
        }
    }
}
