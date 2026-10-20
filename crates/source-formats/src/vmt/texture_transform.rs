use std::{
    ops::{Add, Mul, Sub},
    str::FromStr,
};

#[derive(Clone, Copy)]
pub enum DuplicateOperations {
    LastWins,
    Reject,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TransformError {
    Syntax,
    Duplicate,
    Incomplete,
    Invalid,
    Bounds,
    Rotation,
}

pub fn texture_transform<T>(
    value: Option<&str>,
    duplicates: DuplicateOperations,
) -> Result<[T; 4], TransformError>
where
    T: Copy
        + From<f32>
        + Into<f64>
        + FromStr
        + PartialEq
        + Add<Output = T>
        + Sub<Output = T>
        + Mul<Output = T>,
{
    let components = parse(value, duplicates, false)?;
    Ok(components.diagonal())
}

pub fn texture_transform_matrix(
    value: Option<&str>,
    duplicates: DuplicateOperations,
) -> Result<[[f64; 3]; 2], TransformError> {
    let components = parse::<f64>(value, duplicates, true)?;
    if components.rotate == 0.0 {
        let [sx, sy, tx, ty] = components.diagonal();
        return Ok([[sx, 0.0, tx], [0.0, sy, ty]]);
    }
    let [cx, cy] = components.center;
    let [sx, sy] = components.scale;
    let [tx, ty] = components.translate;
    let (sin, cos) = components.rotate.to_radians().sin_cos();
    let mut rows = [[cos * sx, -sin * sy, 0.0], [sin * sx, cos * sy, 0.0]];
    rows[0][2] = cx + tx - (rows[0][0] * cx + rows[0][1] * cy);
    rows[1][2] = cy + ty - (rows[1][0] * cx + rows[1][1] * cy);
    Ok(rows)
}

struct Components<T> {
    center: [T; 2],
    scale: [T; 2],
    translate: [T; 2],
    rotate: T,
}

impl<T> Components<T>
where
    T: Copy + From<f32> + Add<Output = T> + Sub<Output = T> + Mul<Output = T>,
{
    fn diagonal(&self) -> [T; 4] {
        let one = T::from(1.0);
        [
            self.scale[0],
            self.scale[1],
            self.center[0] * (one - self.scale[0]) + self.translate[0],
            self.center[1] * (one - self.scale[1]) + self.translate[1],
        ]
    }
}

fn parse<T>(
    value: Option<&str>,
    duplicates: DuplicateOperations,
    allow_rotation: bool,
) -> Result<Components<T>, TransformError>
where
    T: Copy + From<f32> + Into<f64> + FromStr + PartialEq,
{
    let zero = T::from(0.0);
    let one = T::from(1.0);
    let mut components = Components {
        center: [T::from(0.5); 2],
        scale: [one; 2],
        translate: [zero; 2],
        rotate: zero,
    };
    let mut seen = [false; 4];
    let mut tokens = value.unwrap_or_default().split_whitespace();
    while let Some(key) = tokens.next() {
        let (index, count) = match key {
            "center" => (0, 2),
            "scale" => (1, 2),
            "translate" => (2, 2),
            "rotate" => (3, 1),
            _ => return Err(TransformError::Syntax),
        };
        if std::mem::replace(&mut seen[index], true)
            && matches!(duplicates, DuplicateOperations::Reject)
        {
            return Err(TransformError::Duplicate);
        }
        let mut values = [zero; 2];
        for value in values.iter_mut().take(count) {
            *value = tokens
                .next()
                .ok_or(TransformError::Incomplete)?
                .parse::<T>()
                .map_err(|_| TransformError::Invalid)?;
            let scalar: f64 = (*value).into();
            if !scalar.is_finite() || scalar.abs() > 1024.0 {
                return Err(TransformError::Bounds);
            }
        }
        match index {
            0 => components.center = values,
            1 => components.scale = values,
            2 => components.translate = values,
            _ if values[0] != zero && !allow_rotation => return Err(TransformError::Rotation),
            _ => components.rotate = values[0],
        }
    }
    Ok(components)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affine_rotation_tracks_native_scale_center_translation_and_sign() {
        for (value, expected) in [
            ("rotate 90", [[0.0, -1.0, 1.0], [1.0, 0.0, 0.0]]),
            ("rotate -90", [[0.0, 1.0, 0.0], [-1.0, 0.0, 1.0]]),
            (
                "center 0.25 0.75 scale 2 3 rotate 90 translate 0.5 -0.25",
                [[0.0, -3.0, 3.0], [2.0, 0.0, 0.0]],
            ),
            (
                "center 0 0 scale -2 3 rotate 90 translate 0.25 0.5",
                [[0.0, -3.0, 0.25], [-2.0, 0.0, 0.5]],
            ),
            (
                "center 0 0 scale 2 3 rotate 30 translate 0.25 0.5",
                [
                    [3.0_f64.sqrt(), -1.5, 0.25],
                    [1.0, 1.5 * 3.0_f64.sqrt(), 0.5],
                ],
            ),
        ] {
            let actual =
                texture_transform_matrix(Some(value), DuplicateOperations::Reject).unwrap();
            for (actual, expected) in actual
                .into_iter()
                .flatten()
                .zip(expected.into_iter().flatten())
            {
                assert!(
                    (actual - expected).abs() < 1e-12,
                    "{value}: {actual} != {expected}"
                );
            }
        }
    }

    #[test]
    fn legacy_transform_keeps_exact_diagonal_results_and_rotation_rejections() {
        for value in [
            None,
            Some("rotate -0 scale 2 3 center 0.25 0.75 translate -0.5 0.125"),
            Some("scale -1 1024 center 0.125 -0.75 translate 0.25 -0.5 rotate 0"),
            Some("scale 2 3 scale 4 5"),
        ] {
            let legacy = texture_transform::<f64>(value, DuplicateOperations::LastWins).unwrap();
            let matrix = texture_transform_matrix(value, DuplicateOperations::LastWins).unwrap();
            assert_eq!(
                matrix,
                [[legacy[0], 0.0, legacy[2]], [0.0, legacy[1], legacy[3]]]
            );
        }
        for value in [
            "rotate 90",
            "rotate 360",
            "rotate 10 rotate 0",
            "rotate 10 unknown",
        ] {
            assert_eq!(
                texture_transform::<f64>(Some(value), DuplicateOperations::LastWins),
                Err(TransformError::Rotation)
            );
        }
        assert_eq!(
            texture_transform_matrix(Some("rotate 10 rotate 0"), DuplicateOperations::LastWins)
                .unwrap(),
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
        );
    }

    #[test]
    fn affine_admission_retains_parser_bounds_and_duplicate_policy() {
        for (value, expected) in [
            ("rotate", TransformError::Incomplete),
            ("rotate invalid", TransformError::Invalid),
            ("rotate NaN", TransformError::Bounds),
            ("rotate inf", TransformError::Bounds),
            ("rotate 1024.00001", TransformError::Bounds),
            ("rotate 30 rotate -30", TransformError::Duplicate),
            ("rotate 30 scale 1", TransformError::Incomplete),
            ("rotate 30 scale 1 inf", TransformError::Bounds),
            ("rotate 30 matrix 1", TransformError::Syntax),
        ] {
            assert_eq!(
                texture_transform_matrix(Some(value), DuplicateOperations::Reject),
                Err(expected),
                "{value}"
            );
        }
        assert_eq!(
            texture_transform::<f32>(Some("scale 1024.00001 1"), DuplicateOperations::Reject)
                .unwrap()[0],
            1024.0
        );
    }
}
