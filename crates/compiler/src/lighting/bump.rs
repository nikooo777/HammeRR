use glam::DVec3;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directional_basis_is_orthonormal_and_tracks_uv_handedness_and_smoothing() {
        for normal in [DVec3::Z, DVec3::new(0.5, 0.25, 1.0).normalize()] {
            let info = TexInfo {
                flags: surf::BUMPLIGHT,
                texture_vecs: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]],
                ..Default::default()
            };
            let basis = Basis::new(&info, DVec3::Z).unwrap().unwrap();
            let vectors = basis.normals(normal).unwrap();
            for (i, vector) in vectors.iter().enumerate().skip(1) {
                assert!((vector.length() - 1.0).abs() < 1e-12);
                assert!((vector.dot(normal) - (1.0_f64 / 3.0).sqrt()).abs() < 1e-12);
                for other in &vectors[i + 1..] {
                    assert!(vector.dot(*other).abs() < 1e-12);
                }
            }
            let mirrored = Basis::new(
                &TexInfo {
                    texture_vecs: [info.texture_vecs[0], [0.0, -1.0, 0.0, 0.0]],
                    ..info
                },
                DVec3::Z,
            )
            .unwrap()
            .unwrap()
            .normals(normal)
            .unwrap();
            assert_eq!(vectors[1], mirrored[1]);
            assert_eq!(vectors[2], mirrored[3]);
            assert_eq!(vectors[3], mirrored[2]);
        }
    }

    #[test]
    fn rejects_singular_frames_and_preserves_ordinary_materials() {
        let info = TexInfo::default();
        assert!(Basis::new(&info, DVec3::Z).unwrap().is_none());
        assert!(
            Basis::new(
                &TexInfo {
                    flags: surf::BUMPLIGHT,
                    ..info
                },
                DVec3::Z
            )
            .is_err()
        );
        let basis = Basis {
            s: DVec3::X,
            left_handed: false,
        };
        assert!(basis.normals(DVec3::ZERO).is_err());
        assert!(basis.normals(DVec3::X).is_err());
    }
}
