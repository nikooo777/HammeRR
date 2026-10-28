use glam::DVec3;
use source_formats::vmf::DispInfo;

use super::{
    Operation,
    brush::{self, Axis, Blend, Brush, Sculpt},
};
use crate::{EditError, brush::Face};

pub const MAX_DABS: usize = 512;
pub const MAX_UPDATE_POINTS: usize = 64;
pub const MAX_INPUT_POINTS: usize = 4096;
pub const MAX_WORK: usize = 2_000_000;

#[derive(Debug, Clone, Copy)]
pub enum Mode {
    Sculpt(Sculpt),
    Paint { value: f64, strength: f64 },
}

#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub brush: Brush,
    pub mode: Mode,
}

impl Spec {
    pub fn from_operation(operation: &Operation) -> Result<Self, EditError> {
        match operation {
            Operation::Sculpt { brush, operation } => Ok(Self {
                brush: *brush,
                mode: Mode::Sculpt(*operation),
            }),
            Operation::Blend {
                brush,
                operation: Blend::Paint { value, strength },
            } => Ok(Self {
                brush: *brush,
                mode: Mode::Paint {
                    value: *value,
                    strength: *strength,
                },
            }),
            _ => Err(EditError::Invalid(
                "Live strokes support sculpting and blend paint".into(),
            )),
        }
    }

    pub fn spacing(self) -> f64 {
        (self.brush.radius * 0.25).clamp(1.0, 64.0)
    }

    pub fn name(self) -> &'static str {
        match self.mode {
            Mode::Sculpt(Sculpt::Raise(_)) => "raise",
            Mode::Sculpt(Sculpt::Flatten(_)) => "flatten",
            Mode::Sculpt(Sculpt::Smooth(_)) => "smooth",
            Mode::Sculpt(Sculpt::Noise { .. }) => "noise",
            Mode::Paint { .. } => "paint",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Path {
    pub points: Vec<DVec3>,
    pub cursor: Option<DVec3>,
    pub input_points: usize,
    distance: f64,
}

impl Path {
    pub fn append(&self, points: &[DVec3], spacing: f64, axis: DVec3) -> Result<Self, EditError> {
        if points.is_empty()
            || points.len() > MAX_UPDATE_POINTS
            || !spacing.is_finite()
            || !(1.0..=64.0).contains(&spacing)
            || self.input_points + points.len() > MAX_INPUT_POINTS
            || !axis.is_finite()
            || axis.length_squared() < 1e-12
            || points
                .iter()
                .any(|p| !p.is_finite() || p.abs().max_element() > 65536.0)
        {
            return Err(EditError::Invalid(
                "Stroke needs 1–64 finite points within ±65536; at most 4096 input points".into(),
            ));
        }
        let mut result = self.clone();
        let axis = axis.normalize();
        result.input_points += points.len();
        for &point in points {
            let Some(mut start) = result.cursor else {
                if result.points.len() >= MAX_DABS {
                    return Err(EditError::Invalid(
                        "Terrain stroke exceeds 512 brush samples".into(),
                    ));
                }
                result.points.push(point);
                result.cursor = Some(point);
                continue;
            };
            let delta = point - start;
            let mut length = (delta - axis * delta.dot(axis)).length();
            if length > 1e-9 {
                let direction = (point - start) / length;
                let mut step = spacing - result.distance;
                while length + 1e-9 >= step {
                    if result.points.len() >= MAX_DABS {
                        return Err(EditError::Invalid(
                            "Terrain stroke exceeds 512 brush samples".into(),
                        ));
                    }
                    start += direction * step.min(length);
                    result.points.push(start);
                    length = (length - step).max(0.0);
                    result.distance = 0.0;
                    step = spacing;
                }
                result.distance += length;
            }
            result.cursor = Some(point);
        }
        Ok(result)
    }

    pub fn interrupt(&mut self) {
        self.cursor = None;
        self.distance = 0.0;
    }
}

pub fn apply(
    face: &Face,
    info: &DispInfo,
    spec: Spec,
    points: &[DVec3],
    vertices: Option<&[usize]>,
) -> Result<DispInfo, EditError> {
    if points.is_empty() || points.len() > MAX_DABS {
        return Err(EditError::Invalid(
            "Terrain stroke needs 1–512 brush samples".into(),
        ));
    }
    let mut result = info.clone();
    for &point in points {
        let mut brush = Brush {
            center: point,
            ..spec.brush
        };
        if matches!(spec.mode, Mode::Sculpt(Sculpt::Flatten(_))) {
            let axis = match brush.axis {
                Axis::WorldZ => DVec3::Z,
                Axis::FaceNormal => face.plane.normal,
            };
            brush.center += axis * (spec.brush.center - point).dot(axis);
        }
        let operation = match spec.mode {
            Mode::Sculpt(operation) => Operation::Sculpt { brush, operation },
            Mode::Paint { value, strength } => Operation::Blend {
                brush,
                operation: Blend::Paint { value, strength },
            },
        };
        result = if let Some(vertices) = vertices {
            brush::masked(face, &result, &operation, vertices)?
        } else {
            match operation {
                Operation::Sculpt { brush, operation } => {
                    brush::sculpt(face, &result, brush, operation)?
                }
                Operation::Blend { brush, operation } => {
                    brush::blend(face, &result, brush, operation)?
                }
                _ => unreachable!(),
            }
        };
    }
    Ok(result)
}
