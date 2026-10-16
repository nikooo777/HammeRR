use glam::{DQuat, DVec3, EulerRot};
use source_formats::{
    bsp::{DetailProp, DetailSprite},
    keyvalues::Node,
};

use super::scatter::{number, value};
use super::shapes::Shape;
use crate::map::MAX_COMPILE_COORD;

pub(super) const KIND: u8 = 1;
const VERTICAL: u8 = 2;
pub(super) const MIN_NATIVE_SCALE: f32 = f32::from_bits(103 << 23);
pub(super) const MAX_SPRITES: usize = 4096;

pub(super) struct Definition {
    pub record: DetailSprite,
    pub random_scale: f64,
    pub orientation: u8,
    pub shape: Option<Shape>,
}

fn vector<const N: usize>(raw: &str, name: &str) -> Result<[f32; N], String> {
    let fields = raw.split_ascii_whitespace().take(N + 1).collect::<Vec<_>>();
    if fields.len() != N {
        return Err(format!(
            "Detail dictionary {name} requires {N} finite numbers"
        ));
    }
    let mut result = [0.0; N];
    for (target, field) in result.iter_mut().zip(fields) {
        *target = field
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or_else(|| format!("Detail dictionary {name} requires {N} finite numbers"))?;
    }
    Ok(result)
}

impl Definition {
    pub fn parse(nodes: &[Node]) -> Result<Self, String> {
        let orientation = number(nodes, "detailOrientation", 0.0, 0.0, 2.0)?;
        if orientation.fract() != 0.0 {
            return Err("Detail dictionary detailOrientation must be an integer within 0–2".into());
        }
        let orientation = orientation as u8;
        let random_scale = number(nodes, "spriterandomscale", 0.0, 0.0, 1.0)?;
        if number(nodes, "sway", 0.0, 0.0, 1.0)? != 0.0 {
            return Err("Detail sprite sway is unsupported".into());
        }
        let shape = Shape::parse(nodes)?;
        if shape.is_some() && orientation != 0 {
            return Err("Detail sprite shapes require detailOrientation=0".into());
        }
        let raw = value(nodes, "sprite")?.ok_or("Detail sprite requires atlas coordinates")?;
        let [x, y, width, height, total] = vector::<5>(raw, "sprite")?;
        if x < 0.0
            || y < 0.0
            || width < 1.0
            || height < 1.0
            || total <= 0.0
            || x + width > total
            || y + height > total
        {
            return Err("Detail sprite rectangle must fit its positive logical atlas size with width and height at least one texel".into());
        }
        let mut upper_left = [-10.0, 20.0];
        let mut lower_right = [10.0, 0.0];
        if let Some(raw) = value(nodes, "spritesize")? {
            let [anchor_x, anchor_y, width, height] = vector::<4>(raw, "spritesize")?;
            if !(0.0..=1.0).contains(&anchor_x)
                || !(0.0..=1.0).contains(&anchor_y)
                || width <= 0.0
                || height <= 0.0
            {
                return Err(
                    "Detail spritesize requires anchors within 0–1 and positive width and height"
                        .into(),
                );
            }
            upper_left = [-width * anchor_x, height - height * anchor_y];
            lower_right = [width - width * anchor_x, -height * anchor_y];
        }
        let record = DetailSprite {
            upper_left,
            lower_right,
            texture_upper_left: [(x + 0.5) / total, (y + 0.5) / total],
            texture_lower_right: [(x + width - 0.5) / total, (y + height - 0.5) / total],
        };
        if let Some(shape) = shape {
            shape.check_bounds(record, DVec3::ZERO, DVec3::ZERO, 1.0)?;
        } else {
            check_bounds(record, DVec3::ZERO, DVec3::ZERO, orientation, 1.0)?;
        }
        Ok(Self {
            record,
            random_scale,
            orientation,
            shape,
        })
    }
}

pub(super) fn key(sprite: DetailSprite) -> [u32; 8] {
    let values = [
        sprite.upper_left,
        sprite.lower_right,
        sprite.texture_upper_left,
        sprite.texture_lower_right,
    ];
    std::array::from_fn(|index| {
        let value = values[index / 2][index % 2];
        if value == 0.0 { 0 } else { value.to_bits() }
    })
}

pub(super) fn check_bounds(
    sprite: DetailSprite,
    origin: DVec3,
    angles: DVec3,
    orientation: u8,
    scale: f32,
) -> Result<(), String> {
    check_geometry(sprite, scale)?;
    if !angles.is_finite() || orientation > VERTICAL {
        return Err("Detail sprite orientation and angles are invalid".into());
    }
    if orientation != VERTICAL {
        check_native_scale(scale)?;
        check_origin(origin)?;
    }
    check_orientation_bounds(sprite, origin, angles, orientation, scale)
}

pub(super) fn check_prop_bounds(sprite: DetailSprite, prop: &DetailProp) -> Result<(), String> {
    let origin = DVec3::from_array(prop.origin.map(f64::from));
    let angles = DVec3::from_array(prop.angles.map(f64::from));
    match prop.kind {
        KIND => check_bounds(sprite, origin, angles, prop.orientation, prop.scale),
        2 | 3 if prop.orientation == 0 && prop.sway == 0 => Shape {
            kind: prop.kind,
            angle: prop.shape_angle,
            size: prop.shape_size,
        }
        .check_bounds(sprite, origin, angles, prop.scale),
        _ => Err("Detail sprite shape requires fixed orientation without sway".into()),
    }
}

pub(super) fn check_native_scale(scale: f32) -> Result<(), String> {
    if !scale.is_finite() || scale < MIN_NATIVE_SCALE {
        return Err("Detail sprite scale collapses in native half precision below 2^-24".into());
    }
    Ok(())
}

pub(super) fn check_origin(origin: DVec3) -> Result<(), String> {
    if !origin.is_finite() || origin.abs().max_element() > MAX_COMPILE_COORD {
        return Err(format!(
            "Detail sprite origin must fit ±{MAX_COMPILE_COORD} units"
        ));
    }
    Ok(())
}

pub(super) fn check_geometry(sprite: DetailSprite, scale: f32) -> Result<(), String> {
    let values = [
        sprite.upper_left,
        sprite.lower_right,
        sprite.texture_upper_left,
        sprite.texture_lower_right,
    ];
    if values.into_iter().flatten().any(|value| !value.is_finite())
        || !scale.is_finite()
        || scale <= 0.0
        || sprite.upper_left[0] >= sprite.lower_right[0]
        || sprite.upper_left[1] <= sprite.lower_right[1]
    {
        return Err(
            "Detail sprite geometry and scale must remain finite and positive in native floats"
                .into(),
        );
    }
    Ok(())
}

fn check_orientation_bounds(
    sprite: DetailSprite,
    origin: DVec3,
    angles: DVec3,
    orientation: u8,
    scale: f32,
) -> Result<(), String> {
    if orientation == 0 {
        let radians = angles.map(f64::to_radians);
        let rotation = DQuat::from_euler(EulerRot::ZYX, radians.y, radians.x, radians.z);
        let right = rotation * DVec3::NEG_Y;
        let up = rotation * DVec3::Z;
        for x in [sprite.upper_left[0], sprite.lower_right[0]] {
            for y in [sprite.upper_left[1], sprite.lower_right[1]] {
                let point = origin + (right * f64::from(x) + up * f64::from(y)) * f64::from(scale);
                if !point.is_finite() || point.abs().max_element() > MAX_COMPILE_COORD {
                    return Err(format!(
                        "Detail sprite fixed bounds must fit ±{MAX_COMPILE_COORD} units"
                    ));
                }
            }
        }
        return Ok(());
    }
    if orientation == 1 {
        let x = f64::from(sprite.upper_left[0].abs().max(sprite.lower_right[0].abs()));
        let y = f64::from(sprite.upper_left[1].abs().max(sprite.lower_right[1].abs()));
        let radius = x.hypot(y) * f64::from(scale);
        let extent = origin.abs() + DVec3::splat(radius);
        if !extent.is_finite() || extent.max_element() > MAX_COMPILE_COORD {
            return Err(format!(
                "Detail sprite full billboard bounds must fit ±{MAX_COMPILE_COORD} units"
            ));
        }
        return Ok(());
    }
    let radius =
        f64::from(sprite.upper_left[0].abs().max(sprite.lower_right[0].abs())) * f64::from(scale);
    let low = origin
        + DVec3::new(
            -radius,
            -radius,
            f64::from(sprite.lower_right[1]) * f64::from(scale),
        );
    let high = origin
        + DVec3::new(
            radius,
            radius,
            f64::from(sprite.upper_left[1]) * f64::from(scale),
        );
    if !origin.is_finite()
        || !low.is_finite()
        || !high.is_finite()
        || low.abs().max_element() > MAX_COMPILE_COORD
        || high.abs().max_element() > MAX_COMPILE_COORD
    {
        return Err(format!(
            "Detail sprite camera-facing bounds must fit ±{MAX_COMPILE_COORD} units"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_source_right_up_and_full_camera_bounds_reject_real_extents() {
        let sprite = DetailSprite {
            upper_left: [-4.0, 16.0],
            lower_right: [12.0, -16.0],
            ..Default::default()
        };
        let origin = DVec3::X * (MAX_COMPILE_COORD - 8.0);
        assert!(check_bounds(sprite, origin, DVec3::ZERO, 0, 1.0).is_ok());
        assert!(check_bounds(sprite, origin, DVec3::new(0.0, 90.0, 0.0), 0, 1.0).is_err());
        assert!(check_bounds(sprite, origin, DVec3::new(90.0, 0.0, 0.0), 0, 1.0).is_err());
        assert!(check_bounds(sprite, origin, DVec3::ZERO, 1, 1.0).is_err());
        assert!(check_bounds(sprite, DVec3::ZERO, DVec3::ZERO, 1, 1.0).is_ok());
        for scale in [f32::MIN_POSITIVE, MIN_NATIVE_SCALE * 0.5] {
            assert!(check_bounds(sprite, DVec3::ZERO, DVec3::ZERO, 0, scale).is_err());
            assert!(check_bounds(sprite, DVec3::ZERO, DVec3::ZERO, 1, scale).is_err());
        }
        for orientation in [0, 1] {
            let near_top = DVec3::Z * (MAX_COMPILE_COORD - 20.0);
            assert!(check_bounds(sprite, near_top, DVec3::ZERO, orientation, 1.0).is_ok());
            assert!(check_bounds(sprite, near_top, DVec3::ZERO, orientation, 2.0).is_err());
        }
        for scale in [MIN_NATIVE_SCALE, 0.5, 1.001, 6.0] {
            assert!(check_bounds(sprite, DVec3::ZERO, DVec3::ZERO, 0, scale).is_ok());
            assert!(check_bounds(sprite, DVec3::ZERO, DVec3::ZERO, 1, scale).is_ok());
        }
        assert!(check_bounds(sprite, DVec3::ZERO, DVec3::ZERO, 2, f32::MIN_POSITIVE).is_ok());
        assert!(check_bounds(sprite, origin, DVec3::new(0.0, 90.0, 0.0), 0, 0.5).is_ok());
        assert!(
            check_bounds(
                sprite,
                DVec3::X * (MAX_COMPILE_COORD + 1.0),
                DVec3::ZERO,
                0,
                0.5
            )
            .is_err()
        );
    }
}
