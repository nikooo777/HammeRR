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
        let angle = number(nodes, "shape_angle", 0.0, 0.0, 255.0)?;
        let size = number(nodes, "shape_size", 0.0, 0.0, 1.0)?;
        Self::parameters(name, angle, size).map(Some)
    }

    pub fn parameters(name: &str, angle: f64, size: f64) -> Result<Self, String> {
        let kind = if name.eq_ignore_ascii_case("cross") {
            CROSS
        } else if name.eq_ignore_ascii_case("tri") {
            TRI
        } else {
            return Err("Detail sprite_shape must be cross or tri".into());
        };
        if !angle.is_finite() || !(0.0..=255.0).contains(&angle) || angle.fract() != 0.0 {
            return Err("Detail shape_angle must be an integer within 0–255".into());
        }
        if !size.is_finite() || !(0.0..=1.0).contains(&size) {
            return Err("Detail shape_size must be finite and within 0–1".into());
        }
        let size = size as f32;
        if kind == CROSS && (angle != 0.0 || size != 0.0) {
            return Err("Cross detail shapes require shape_angle=0 and shape_size=0; the native cross geometry ignores these parameters".into());
        }
        Ok(Self {
            kind,
            angle: angle as u8,
            size: (255.0 * f64::from(size)) as u8,
        })
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
        scale: f32,
    ) -> Result<(), String> {
        super::sprites::check_geometry(sprite, scale)?;
        super::sprites::check_native_transform(scale, origin)?;
        if !origin.is_finite()
            || !angles.is_finite()
            || !matches!(self.kind, CROSS | TRI)
            || self.kind == CROSS && (self.angle != 0 || self.size != 0)
        {
            return Err("Detail sprite shape geometry is invalid".into());
        }
        let quads = if scale == 1.0 {
            self.quads(sprite, origin, angles)
        } else {
            self.quads(sprite, DVec3::ZERO, angles)
                .into_iter()
                .map(|quad| quad.map(|point| origin + point * f64::from(scale)))
                .collect()
        };
        if quads
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
