use std::{
    ops::{Add, Mul, Sub},
    str::FromStr,
};

#[derive(Clone, Copy)]
pub enum DuplicateOperations {
    LastWins,
    Reject,
}

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
    let zero = T::from(0.0);
    let one = T::from(1.0);
    let mut center = [T::from(0.5); 2];
    let mut scale = [one; 2];
    let mut translate = [zero; 2];
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
            0 => center = values,
            1 => scale = values,
            2 => translate = values,
            _ if values[0] != zero => return Err(TransformError::Rotation),
            _ => {}
        }
    }
    Ok([
        scale[0],
        scale[1],
        center[0] * (one - scale[0]) + translate[0],
        center[1] * (one - scale[1]) + translate[1],
    ])
}
