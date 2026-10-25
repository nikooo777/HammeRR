use super::{MIN_LIGHT_DISTANCE, REFERENCE_DISTANCE, number, unresolved_value};
use crate::map::MapEntity;

const NEAR_ZERO_DIVISOR: f64 = 256.0;
const MONOTONIC_STEPS: usize = 20;

#[derive(Debug, Clone, Copy)]
pub(super) struct Falloff {
    pub coefficients: [f64; 3],
    pub gain: f64,
    pub radius: f64,
    pub(super) cap: Option<f64>,
    pub(super) fade: Option<[f64; 2]>,
}

impl Falloff {
    pub(super) fn read(entity: &MapEntity, defer_parameters: bool) -> Result<Option<Self>, String> {
        let scalar = |name| {
            if unresolved_value(entity, name, defer_parameters) {
                Ok(None)
            } else {
                number(entity, name).map(Some)
            }
        };
        let half = scalar("_fifty_percent_distance")?;
        let end = scalar("_zero_percent_distance")?;
        let hard = scalar("_hardfalloff")?;
        if hard.is_some_and(|hard| hard != 0.0 && hard != 1.0) {
            return Err("Light hard falloff must be zero or one".into());
        }
        if half
            .into_iter()
            .chain(end)
            .any(|value| !native_value(value))
        {
            return Err("Light falloff exceeds finite native float limits".into());
        }
        if half == Some(0.0)
            && (end.is_some_and(|end| end != 0.0) || hard.is_some_and(|hard| hard != 0.0))
        {
            return Err("Near-zero distance and hard falloff need a positive 50% distance".into());
        }
        let Some(half) = half else {
            return Ok(None);
        };
        let result = if half > 0.0 {
            let Some(end) = end else {
                return Ok(None);
            };
            let end = if end == 0.0 { 2.0 * half } else { end };
            if end <= half {
                return Err("Near-zero light distance must exceed the 50% distance; use zero for an automatic distance".into());
            }
            let coefficients = percentage_curve(half, end)?;
            if !native_value(end)
                || coefficients
                    .iter()
                    .any(|&value| !native_preserves_nonzero(value))
            {
                return Err("Light falloff exceeds finite native float limits".into());
            }
            let Some(hard) = hard else {
                return Ok(None);
            };
            let [_, linear, quadratic] = coefficients;
            let cap = if quadratic < 0.0 {
                Some(linear / (-2.0 * quadratic))
            } else {
                None
            };
            let fade = if hard == 1.0 {
                Some([0.75 * end + 0.25 * half, end])
            } else {
                cap.map(|cap| [cap, 10.0 * cap])
            };
            Self {
                coefficients,
                gain: 1.0,
                radius: 0.0,
                cap: if hard == 1.0 { None } else { cap },
                fade,
            }
        } else {
            let coefficients = [
                scalar("_constant_attn")?,
                scalar("_linear_attn")?,
                scalar("_quadratic_attn")?,
            ];
            let radius = scalar("_distance")?;
            if radius.is_some_and(|radius| !native_preserves_nonzero(radius))
                || coefficients
                    .into_iter()
                    .flatten()
                    .any(|value| !native_preserves_nonzero(value))
            {
                return Err("Light falloff exceeds finite native float limits".into());
            }
            let (Some(radius), [Some(constant), Some(linear), Some(quadratic)], Some(_), Some(_)) =
                (radius, coefficients, end, hard)
            else {
                return Ok(None);
            };
            let mut coefficients = [constant, linear, quadratic];
            if coefficients == [0.0; 3] {
                coefficients[0] = 1.0;
            }
            Self {
                coefficients,
                gain: coefficients[0]
                    + REFERENCE_DISTANCE * coefficients[1]
                    + REFERENCE_DISTANCE.powi(2) * coefficients[2],
                radius,
                cap: None,
                fade: None,
            }
        };
        if [half, result.radius]
            .into_iter()
            .chain(end)
            .chain(result.coefficients)
            .chain(result.cap)
            .chain(result.fade.into_iter().flatten())
            .any(|value| !native_value(value))
            || result
                .coefficients
                .iter()
                .any(|&value| !native_preserves_nonzero(value))
            || !result.gain.is_finite()
        {
            return Err("Light falloff exceeds finite native float limits".into());
        }
        Ok(Some(result))
    }

    pub(super) fn outside_range(self, distance: f64) -> bool {
        (self.radius > 0.0 && distance > self.radius)
            || self.fade.is_some_and(|[_, end]| distance >= end)
    }

    pub(super) fn factor(self, distance: f64) -> Result<f64, String> {
        if self.outside_range(distance) {
            return Ok(0.0);
        }
        let distance = distance.max(MIN_LIGHT_DISTANCE);
        let evaluated = self.cap.map_or(distance, |cap| distance.min(cap));
        let [constant, linear, quadratic] = self.coefficients;
        let divisor = constant + evaluated * linear + evaluated * evaluated * quadratic;
        if !divisor.is_finite() || divisor <= 0.0 {
            return Err("Light falloff divisor must be finite and positive".into());
        }
        let fade = self.fade.map_or(1.0, |[start, end]| {
            let t = (1.0 - (distance - start) / (end - start)).clamp(0.0, 1.0);
            t.powi(3) * (t * (6.0 * t - 15.0) + 10.0)
        });
        Ok(fade / divisor)
    }
}

fn native_value(value: f64) -> bool {
    value.is_finite() && value.abs() <= f64::from(f32::MAX)
}

fn native_preserves_nonzero(value: f64) -> bool {
    native_value(value) && (value == 0.0 || value as f32 != 0.0)
}

fn percentage_curve(half: f64, end: f64) -> Result<[f64; 3], String> {
    let linear_midpoint = 1.0 + (NEAR_ZERO_DIVISOR - 1.0) * (half / end);
    let mut coefficients = [0.0; 3];
    for step in 0..=MONOTONIC_STEPS {
        let blend = step as f64 / MONOTONIC_STEPS as f64;
        let midpoint = 2.0 + blend * (linear_midpoint - 2.0);
        let quadratic = ((NEAR_ZERO_DIVISOR - 1.0) / end - (midpoint - 1.0) / half) / (end - half);
        let linear = (midpoint - 1.0) / half - quadratic * half;
        coefficients = [1.0, linear, quadratic];
        if 2.0 * quadratic + linear >= 0.0 {
            break;
        }
    }
    let scale = 2.0 / (coefficients[0] + half * (coefficients[1] + half * coefficients[2]));
    let coefficients = coefficients.map(|value| value * scale);
    if coefficients.iter().any(|value| !value.is_finite()) {
        return Err("Percentage light falloff cannot be resolved to a finite curve".into());
    }
    Ok(coefficients)
}
