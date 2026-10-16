use glam::{DQuat, DVec3, EulerRot};
use source_formats::{bsp::DetailSprite, keyvalues::Node};

use super::scatter::{number, value};
use crate::map::MAX_COMPILE_COORD;

const CROSS: u8 = 2;
const TRI: u8 = 3;

#[derive(Clone, Copy)]
pub(super) struct Shape {
    pub kind: u8,
    pub angle: u8,
    pub size: u8,
}

impl Shape {
    pub fn parse(nodes: &[Node]) -> Result<Option<Self>, String> {
        let Some(name) = value(nodes, "sprite_shape")? else {
            if value(nodes, "shape_angle")?.is_some() || value(nodes, "shape_size")?.is_some() {
                return Err("Detail shape parameters require sprite_shape cross or tri".into());
            }
            return Ok(None);
        };
        let kind = if name.eq_ignore_ascii_case("cross") {
            CROSS
        } else if name.eq_ignore_ascii_case("tri") {
            TRI
        } else {
            return Err("Detail sprite_shape must be cross or tri".into());
        };
        let angle = number(nodes, "shape_angle", 0.0, 0.0, 255.0)?;
        if angle.fract() != 0.0 {
            return Err("Detail shape_angle must be an integer within 0–255".into());
        }
        let size = number(nodes, "shape_size", 0.0, 0.0, 1.0)? as f32;
        if kind == CROSS && (angle != 0.0 || size != 0.0) {
            return Err("Cross detail shapes require shape_angle=0 and shape_size=0; the native cross geometry ignores these parameters".into());
        }
        Ok(Some(Self {
            kind,
            angle: angle as u8,
            size: (255.0 * f64::from(size)) as u8,
        }))
    }

    fn quads(self, sprite: DetailSprite, origin: DVec3, angles: DVec3) -> Vec<[DVec3; 4]> {
        let radians = angles.map(f64::to_radians);
        let rotation = DQuat::from_euler(EulerRot::ZYX, radians.y, radians.x, radians.z);
        let width = f64::from(sprite.lower_right[0]) - f64::from(sprite.upper_left[0]);
        let height = f64::from(sprite.lower_right[1]) - f64::from(sprite.upper_left[1]);
        let mut result = Vec::with_capacity(4);
        if self.kind == CROSS {
            let up = rotation * DVec3::Z;
            let base = origin + up * f64::from(sprite.upper_left[1]);
            let vertical = up * height;
            for axis in [DVec3::NEG_X, DVec3::Y, DVec3::X, DVec3::NEG_Y] {
                let horizontal = rotation * axis * (width * 0.5);
                result.push([
                    base,
                    base + vertical,
                    base + vertical + horizontal,
                    base + horizontal,
                ]);
            }
        } else {
            let offset = f64::from(f32::from(self.size) * (1.0_f32 / 255.0)) * width;
            for branch in 0..3 {
                let local = DQuat::from_euler(
                    EulerRot::ZYX,
                    f64::from(branch) * std::f64::consts::TAU / 3.0,
                    f64::from(self.angle).to_radians(),
                    0.0,
                );
                let basis = rotation * local;
                let forward = basis * DVec3::X;
                let right = basis * DVec3::NEG_Y;
                let up = basis * DVec3::Z;
                let base = origin
                    + right * f64::from(sprite.upper_left[0])
                    + up * f64::from(sprite.upper_left[1])
                    + forward * offset;
                let horizontal = right * width;
                let vertical = up * height;
                result.push([
                    base,
                    base + vertical,
                    base + vertical + horizontal,
                    base + horizontal,
                ]);
            }
        }
        result
    }

    pub fn check_bounds(
        self,
        sprite: DetailSprite,
        origin: DVec3,
        angles: DVec3,
    ) -> Result<(), String> {
        super::sprites::check_geometry(sprite, 1.0)?;
        if !origin.is_finite()
            || !angles.is_finite()
            || !matches!(self.kind, CROSS | TRI)
            || self.kind == CROSS && (self.angle != 0 || self.size != 0)
        {
            return Err("Detail sprite shape geometry is invalid".into());
        }
        if self
            .quads(sprite, origin, angles)
            .into_iter()
            .flatten()
            .any(|point| !point.is_finite() || point.abs().max_element() > MAX_COMPILE_COORD)
        {
            return Err(format!(
                "Detail sprite shape bounds must fit ±{MAX_COMPILE_COORD} units"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sprite() -> DetailSprite {
        DetailSprite {
            upper_left: [-4.0, 16.0],
            lower_right: [12.0, -16.0],
            ..Default::default()
        }
    }

    #[test]
    fn cross_uses_centered_half_cards_and_tri_uses_encoded_offsets() {
        let cross = Shape {
            kind: CROSS,
            angle: 0,
            size: 0,
        };
        let quads = cross.quads(sprite(), DVec3::ZERO, DVec3::ZERO);
        assert_eq!(quads.len(), 4);
        assert_eq!(
            quads[0],
            [
                DVec3::new(0.0, 0.0, 16.0),
                DVec3::new(0.0, 0.0, -16.0),
                DVec3::new(-8.0, 0.0, -16.0),
                DVec3::new(-8.0, 0.0, 16.0)
            ]
        );
        let mut centered = sprite();
        centered.upper_left[0] = -8.0;
        centered.lower_right[0] = 8.0;
        assert_eq!(quads, cross.quads(centered, DVec3::ZERO, DVec3::ZERO));
        let tri = Shape {
            kind: TRI,
            angle: 0,
            size: 127,
        };
        let quads = tri.quads(sprite(), DVec3::ZERO, DVec3::ZERO);
        assert_eq!(quads.len(), 3);
        let offset = f64::from(127.0_f32 * (1.0_f32 / 255.0)) * 16.0;
        assert_eq!(
            quads[0],
            [
                DVec3::new(offset, 4.0, 16.0),
                DVec3::new(offset, 4.0, -16.0),
                DVec3::new(offset, -12.0, -16.0),
                DVec3::new(offset, -12.0, 16.0)
            ]
        );
        for branch in 0..3 {
            let expected =
                DQuat::from_rotation_z(branch as f64 * std::f64::consts::TAU / 3.0) * quads[0][0];
            assert!((quads[branch][0] - expected).length() < 1e-12);
        }
    }

    #[test]
    fn shape_bounds_follow_fanned_and_rotated_quads() {
        for shape in [
            Shape {
                kind: CROSS,
                angle: 0,
                size: 0,
            },
            Shape {
                kind: TRI,
                angle: 75,
                size: 255,
            },
        ] {
            assert!(
                shape
                    .check_bounds(sprite(), DVec3::ZERO, DVec3::new(35.0, 70.0, 20.0))
                    .is_ok()
            );
            assert!(
                shape
                    .check_bounds(
                        sprite(),
                        DVec3::X * MAX_COMPILE_COORD,
                        DVec3::new(35.0, 70.0, 20.0)
                    )
                    .is_err()
            );
            assert!(
                shape
                    .check_bounds(
                        sprite(),
                        DVec3::Z * MAX_COMPILE_COORD,
                        DVec3::new(0.0, 0.0, 180.0)
                    )
                    .is_err()
            );
        }
    }
}
