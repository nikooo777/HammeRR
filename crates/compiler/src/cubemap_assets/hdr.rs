use std::io;

use source_formats::vtf::{ImageFormat, Vtf};

use super::manifest::{cube, invalid};
use super::{CubemapAssetPhase, CubemapSampleAsset, MAX_CUBE_EDGE, MAX_VTF_BYTES, hash};

const HEADER_BYTES: usize = 88;
const HALF_ONE: [u8; 2] = 0x3c00u16.to_le_bytes();
const SUPPORTED_FLAGS: u32 = 0x600c;

fn header(edge: u16, flags: u32, reflectivity: [f32; 3], bump_scale: [u8; 4]) -> Vec<u8> {
    let mut bytes = vec![0; HEADER_BYTES];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [
        (4, 7u32),
        (8, 4),
        (12, HEADER_BYTES as u32),
        (20, flags),
        (52, 24),
        (57, u32::MAX),
        (68, 1),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [(16, edge), (18, edge), (24, 1), (63, 1)] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    for (index, value) in reflectivity.iter().enumerate() {
        bytes[32 + index * 4..36 + index * 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[48..52].copy_from_slice(&bump_scale);
    bytes[56] = edge.ilog2() as u8 + 1;
    bytes[80..88].copy_from_slice(&[0x30, 0, 0, 0, HEADER_BYTES as u8, 0, 0, 0]);
    bytes
}

pub(super) fn bootstrap_cube(edge: u16) -> Result<Vec<u8>, String> {
    if !edge.is_power_of_two() || edge > MAX_CUBE_EDGE {
        return Err("Unsupported HDR reflection cube edge".into());
    }
    let mut bytes = header(edge, 0x400c, [0.0; 3], 1f32.to_le_bytes());
    for mip in (0..bytes[56]).rev() {
        let size = usize::from((edge >> mip).max(1));
        bytes.resize(bytes.len() + size * size * 7 * 8, 0);
    }
    Ok(bytes)
}

fn captured_cube(bytes: &[u8], edge: u16) -> io::Result<Vtf> {
    let vtf = cube(bytes, edge)?;
    if vtf.version != (7, 4)
        || vtf.format != ImageFormat::Rgba16161616F
        || vtf.flags & !SUPPORTED_FLAGS != 0
    {
        return Err(invalid(
            "HDR reflection capture requires native VTF7.4 RGBA16F with unused alpha",
        ));
    }
    let bump = f32::from_le_bytes(bytes[48..52].try_into().unwrap());
    if !bump.is_finite() || bump < 0.0 || vtf.reflectivity.iter().any(|value| *value < 0.0) {
        return Err(invalid(
            "HDR reflection reflectivity or bump metadata is invalid",
        ));
    }
    Ok(vtf)
}

pub(super) fn canonicalize(bytes: &[u8], edge: u16) -> io::Result<Vec<u8>> {
    let vtf = captured_cube(bytes, edge)?;
    let mut result = header(
        edge,
        vtf.flags,
        vtf.reflectivity,
        bytes[48..52].try_into().unwrap(),
    );
    for mip in (0..vtf.mip_count).rev() {
        for face in 0..7 {
            let data = vtf
                .image_data(mip, 0, face)
                .map_err(|error| invalid(error.to_string()))?;
            if face == 6 {
                result.resize(result.len() + data.len(), 0);
                continue;
            }
            for pixel in data.as_chunks::<8>().0 {
                for value in pixel[..6].as_chunks::<2>().0 {
                    let bits = u16::from_le_bytes(*value);
                    if bits & 0x7c00 == 0x7c00 || (bits & 0x8000 != 0 && bits & 0x7fff != 0) {
                        return Err(invalid(format!(
                            "HDR reflection RGB contains a negative or nonfinite value at mip {mip}, face {face}"
                        )));
                    }
                }
                result.extend_from_slice(&pixel[..6]);
                result.extend_from_slice(&HALF_ONE);
            }
        }
    }
    if result.len() > MAX_VTF_BYTES {
        return Err(invalid("Canonical HDR reflection VTF exceeds 4 MiB"));
    }
    Ok(result)
}

pub(super) fn validate_sample(
    bytes: &[u8],
    sample: &CubemapSampleAsset,
    phase: CubemapAssetPhase,
) -> io::Result<()> {
    let bootstrap = bootstrap_cube(sample.edge).map_err(invalid)?;
    if hash(&bootstrap) != sample.bootstrap_sha256 {
        return Err(invalid(
            "HDR reflection bootstrap provenance does not match its sample",
        ));
    }
    if phase == CubemapAssetPhase::Pending {
        if bytes != bootstrap {
            return Err(invalid(
                "Pending HDR reflection sample differs from its registered bootstrap",
            ));
        }
    } else if bytes == bootstrap || canonicalize(bytes, sample.edge)? != bytes {
        return Err(invalid(
            "Verified HDR reflection sample contains a bootstrap or noncanonical unused data",
        ));
    }
    Ok(())
}

pub(super) fn validate_cube(bytes: &[u8], edge: u16) -> io::Result<()> {
    if canonicalize(bytes, edge)? != bytes {
        return Err(invalid(
            "HDR reflection default contains noncanonical unused data",
        ));
    }
    Ok(())
}
