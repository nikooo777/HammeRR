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

    #[test]
    fn rotated_texture_axes_preserve_directional_order_handedness_and_smooth_normals() {
        for angles in [
            [0.0, 90.0, 0.0],
            [90.0, 0.0, 0.0],
            [0.0, 0.0, 90.0],
            [20.0, 35.0, 10.0],
        ] {
            let [pitch, yaw, roll] = angles.map(f64::to_radians);
            let rotation = DMat3::from_quat(glam::DQuat::from_euler(
                glam::EulerRot::ZYX,
                yaw,
                pitch,
                roll,
            ));
            let (sp, cp) = pitch.sin_cos();
            let (sy, cy) = yaw.sin_cos();
            let (sr, cr) = roll.sin_cos();
            let expected_rotation = |point: DVec3| {
                DVec3::new(
                    cy * cp * point.x
                        + (cy * sp * sr - sy * cr) * point.y
                        + (cy * sp * cr + sy * sr) * point.z,
                    sy * cp * point.x
                        + (sy * sp * sr + cy * cr) * point.y
                        + (sy * sp * cr - cy * sr) * point.z,
                    -sp * point.x + cp * sr * point.y + cp * cr * point.z,
                )
            };
            for mirrored in [false, true] {
                let info = TexInfo {
                    flags: surf::BUMPLIGHT,
                    texture_vecs: [
                        [2.0, 0.0, 0.0, 17.0],
                        [0.0, if mirrored { -3.0 } else { 3.0 }, 0.0, -9.0],
                    ],
                    ..Default::default()
                };
                for (normal, tangent) in [
                    (DVec3::Z, DVec3::X),
                    (DVec3::new(0.6, 0.0, 0.8), DVec3::new(0.8, 0.0, -0.6)),
                ] {
                    let mut basis = Basis::new(&info, DVec3::Z).unwrap().unwrap();
                    basis.rotate(rotation);
                    let actual = basis.normals(rotation * normal).unwrap();
                    let bitangent = if mirrored { DVec3::NEG_Y } else { DVec3::Y };
                    let local = [
                        normal,
                        tangent * (2.0_f64 / 3.0).sqrt() + normal / 3.0_f64.sqrt(),
                        -tangent / 6.0_f64.sqrt()
                            + bitangent / 2.0_f64.sqrt()
                            + normal / 3.0_f64.sqrt(),
                        -tangent / 6.0_f64.sqrt() - bitangent / 2.0_f64.sqrt()
                            + normal / 3.0_f64.sqrt(),
                    ];
                    for (actual, local) in actual.into_iter().zip(local) {
                        assert!(actual.distance(expected_rotation(local)) < 1e-12);
                    }
                }
            }
        }
    }
}
