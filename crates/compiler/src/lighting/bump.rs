use glam::{DMat3, DVec3};
use source_formats::bsp::{TexInfo, surf};

pub(super) const MAPS: usize = 4;

pub(super) struct Basis {
    s: DVec3,
    left_handed: bool,
}

impl Basis {
    pub fn new(info: &TexInfo, normal: DVec3) -> Result<Option<Self>, String> {
        if info.flags & surf::BUMPLIGHT == 0 {
            return Ok(None);
        }
        let [s, t] = info
            .texture_vecs
            .map(|row| DVec3::new(f64::from(row[0]), f64::from(row[1]), f64::from(row[2])));
        let orientation = normal.dot(s.cross(t));
        if !orientation.is_finite() || orientation == 0.0 {
            return Err("Normal-map lighting needs a nonsingular texture projection".into());
        }
        Ok(Some(Self {
            s,
            left_handed: orientation < 0.0,
        }))
    }

    pub fn normals(&self, normal: DVec3) -> Result<[DVec3; MAPS], String> {
        let (tangent, bitangent, normal) =
            map_core::surface::texture_tangents(normal, self.s, self.left_handed)?;
        let z = (1.0_f64 / 3.0).sqrt();
        let x = (1.0_f64 / 6.0).sqrt();
        let y = (1.0_f64 / 2.0).sqrt();
        Ok([
            normal,
            tangent * (2.0_f64 / 3.0).sqrt() + normal * z,
            tangent * -x + bitangent * y + normal * z,
            tangent * -x - bitangent * y + normal * z,
        ])
    }

    pub(super) fn rotate(&mut self, rotation: DMat3) {
        self.s = rotation * self.s;
    }
}
