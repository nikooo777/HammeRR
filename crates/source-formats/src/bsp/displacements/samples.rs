use std::io;

use glam::DVec3;

use super::{
    DisplacementInfo, MAX_AUXILIARY_BYTES, MAX_DISPLACEMENTS, auxiliary, checked_records, faces,
};
use crate::bsp::{Bsp, Face, invalid, lump};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplacementLightmapSample {
    pub triangle: u16,
    pub weights: [u8; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplacementSurfaceSample {
    pub position: [f64; 3],
    pub normal: [f64; 3],
}

impl DisplacementLightmapSample {
    pub fn surface(
        self,
        triangles: &[[usize; 3]],
        positions: &[[f32; 3]],
    ) -> io::Result<Option<DisplacementSurfaceSample>> {
        if self.triangle == 0 && self.weights == [0; 3] {
            return Ok(None);
        }
        let indices = triangles
            .get(usize::from(self.triangle))
            .ok_or_else(|| invalid("displacement sample triangle is missing"))?;
        let mut points = [DVec3::ZERO; 3];
        for (point, &index) in points.iter_mut().zip(indices) {
            *point = DVec3::from_array(
                positions
                    .get(index)
                    .ok_or_else(|| invalid("displacement sample vertex is missing"))?
                    .map(f64::from),
            );
            if !point.is_finite() {
                return Err(invalid("displacement sample vertex must be finite"));
            }
        }
        let total: f64 = self.weights.iter().map(|&weight| f64::from(weight)).sum();
        if total == 0.0 {
            return Err(invalid("displacement sample weights cannot all be zero"));
        }
        let position = points
            .iter()
            .zip(self.weights)
            .map(|(&point, weight)| point * (f64::from(weight) / total))
            .sum::<DVec3>();
        let normal = (points[2] - points[0])
            .cross(points[1] - points[0])
            .try_normalize()
            .ok_or_else(|| invalid("displacement sample triangle is collapsed"))?;
        if !position.is_finite() || !normal.is_finite() {
            return Err(invalid("displacement surface sample must be finite"));
        }
        Ok(Some(DisplacementSurfaceSample {
            position: position.to_array(),
            normal: normal.to_array(),
        }))
    }
}

pub(super) struct Samples<'a> {
    bytes: &'a [u8],
    count: usize,
    remaining: usize,
    triangles: usize,
}

impl Samples<'_> {
    pub(super) fn count(&self) -> usize {
        self.count
    }
    pub(super) fn remaining(&self) -> usize {
        self.remaining
    }

    pub(super) fn read(&mut self) -> io::Result<DisplacementLightmapSample> {
        if self.remaining == 0 {
            return Err(invalid("no displacement lightmap samples remain"));
        }
        let (&first, mut bytes) = self
            .bytes
            .split_first()
            .ok_or_else(|| invalid("truncated displacement lightmap sample"))?;
        let triangle = if first == 255 {
            let (&extra, rest) = bytes
                .split_first()
                .ok_or_else(|| invalid("truncated displacement triangle escape"))?;
            bytes = rest;
            255 + u16::from(extra)
        } else {
            u16::from(first)
        };
        if usize::from(triangle) >= self.triangles {
            return Err(invalid(
                "displacement lightmap sample triangle is out of range",
            ));
        }
        let weights = bytes
            .get(..3)
            .ok_or_else(|| invalid("truncated displacement lightmap weights"))?
            .try_into()
            .unwrap();
        self.bytes = &bytes[3..];
        self.remaining -= 1;
        Ok(DisplacementLightmapSample { triangle, weights })
    }
}

pub(super) fn stream<'a>(
    bsp: &'a Bsp,
    info: DisplacementInfo,
    face: &Face,
) -> io::Result<Option<Samples<'a>>> {
    auxiliary(
        bsp,
        lump::DISP_LIGHTMAP_SAMPLE_POSITIONS,
        info.lightmap_sample_start,
    )?;
    let bytes = &bsp.lumps[lump::DISP_LIGHTMAP_SAMPLE_POSITIONS].data;
    if bytes.is_empty() {
        return Ok(None);
    }
    let mut count = 1usize;
    for size in face.lightmap_size {
        let dimension = usize::try_from(size)
            .ok()
            .and_then(|size| size.checked_add(1))
            .ok_or_else(|| invalid("displacement lightmap size is invalid"))?;
        count = count
            .checked_mul(dimension)
            .filter(|&count| count <= MAX_AUXILIARY_BYTES / 4)
            .ok_or_else(|| invalid("displacement lightmap sample count exceeds bounds"))?;
    }
    let bytes = &bytes[info.lightmap_sample_start as usize..];
    if count > bytes.len() / 4 {
        return Err(invalid("truncated displacement lightmap sample stream"));
    }
    Ok(Some(Samples {
        bytes,
        count,
        remaining: count,
        triangles: info.triangle_count()?,
    }))
}

impl DisplacementInfo {
    pub fn encode_lightmap_samples(
        self,
        samples: &[DisplacementLightmapSample],
    ) -> io::Result<Vec<u8>> {
        let triangles = self.triangle_count()?;
        if samples.len() > MAX_AUXILIARY_BYTES / 4 {
            return Err(invalid("displacement lightmap sample count exceeds bounds"));
        }
        let mut size = 0usize;
        for sample in samples {
            if usize::from(sample.triangle) >= triangles || sample.triangle > 510 {
                return Err(invalid(
                    "displacement lightmap triangle is not representable",
                ));
            }
            size += if sample.triangle < 255 { 4 } else { 5 };
            if size > MAX_AUXILIARY_BYTES {
                return Err(invalid("displacement lightmap sample bytes exceed bounds"));
            }
        }
        let mut bytes = Vec::with_capacity(size);
        for sample in samples {
            if sample.triangle < 255 {
                bytes.push(sample.triangle as u8);
            } else {
                bytes.extend([255, (sample.triangle - 255) as u8]);
            }
            bytes.extend(sample.weights);
        }
        Ok(bytes)
    }
}

impl Bsp {
    pub fn displacement_lightmap_samples(
        &self,
        index: usize,
    ) -> io::Result<Option<Vec<DisplacementLightmapSample>>> {
        let infos = checked_records::<DisplacementInfo>(self, MAX_DISPLACEMENTS)?;
        let info = *infos
            .get(index)
            .ok_or_else(|| invalid("displacement index is invalid"))?;
        let faces = faces(self)?;
        let face = faces
            .get(usize::from(info.face))
            .ok_or_else(|| invalid("displacement base face is missing"))?;
        if face.dispinfo != index as i16 || face.edge_count != 4 {
            return Err(invalid(
                "displacement base face reference or quad topology is invalid",
            ));
        }
        let Some(mut stream) = stream(self, info, face)? else {
            return Ok(None);
        };
        let mut samples = Vec::with_capacity(stream.count());
        while stream.remaining() != 0 {
            samples.push(stream.read()?);
        }
        Ok(Some(samples))
    }
}
