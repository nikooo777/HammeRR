use glam::{DVec2, DVec3};

use crate::surface::{self, FaceId, Projection, SurfaceEdit, placed_projection};
use crate::{Document, EditError, surface_group};

mod packing;
pub mod regions;

pub use packing::{PackOptions, PackedIsland};

pub const MAX_FACES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rectangle {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FitRegion {
    pub rectangle: Rectangle,
    pub padding: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FitMode {
    PerFace,
    Together { reference: FaceId },
    Pack(PackOptions),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitOptions {
    pub region: FitRegion,
    pub mode: FitMode,
}

#[derive(Debug)]
pub struct FitPlan {
    pub edits: Vec<(FaceId, SurfaceEdit)>,
    pub islands: Vec<PackedIsland>,
}

impl FitRegion {
    pub fn usable_rectangle(self, size: [u32; 2]) -> Result<Rectangle, EditError> {
        self.rectangle.validate(size)?;
        let inset = self
            .padding
            .checked_mul(2)
            .filter(|inset| *inset < self.rectangle.width && *inset < self.rectangle.height)
            .ok_or_else(|| {
                EditError::Invalid(
                    "Atlas padding must leave a positive usable width and height inside the rectangle"
                        .into(),
                )
            })?;
        Ok(Rectangle {
            x: self.rectangle.x + self.padding,
            y: self.rectangle.y + self.padding,
            width: self.rectangle.width - inset,
            height: self.rectangle.height - inset,
        })
    }
}

impl Rectangle {
    pub fn validate(self, size: [u32; 2]) -> Result<(), EditError> {
        if self.width == 0
            || self.height == 0
            || size.contains(&0)
            || self
                .x
                .checked_add(self.width)
                .is_none_or(|edge| edge > size[0])
            || self
                .y
                .checked_add(self.height)
                .is_none_or(|edge| edge > size[1])
        {
            return Err(EditError::Invalid(
                "Atlas rectangle must have positive area within the texture dimensions".into(),
            ));
        }
        Ok(())
    }

    pub fn uv_bounds(self, size: [u32; 2]) -> Result<[[f64; 2]; 2], EditError> {
        self.validate(size)?;
        Ok([
            [
                f64::from(self.x) / f64::from(size[0]),
                f64::from(self.y) / f64::from(size[1]),
            ],
            [
                f64::from(self.x + self.width) / f64::from(size[0]),
                f64::from(self.y + self.height) / f64::from(size[1]),
            ],
        ])
    }
}

pub fn fit_projection(
    projection: &Projection,
    points: &[DVec3],
    rectangle: Rectangle,
    size: [u32; 2],
) -> Result<Projection, EditError> {
    fit_region_projection(
        projection,
        points,
        FitRegion {
            rectangle,
            padding: 0,
        },
        size,
    )
}

pub fn fit_region_projection(
    projection: &Projection,
    points: &[DVec3],
    region: FitRegion,
    size: [u32; 2],
) -> Result<Projection, EditError> {
    let rectangle = region.usable_rectangle(size)?;
    let mut fitted = placed_projection(
        projection,
        points,
        Some([rectangle.width, rectangle.height]),
        [None, None],
    )?;
    fitted.u.offset += f64::from(rectangle.x);
    fitted.v.offset += f64::from(rectangle.y);
    Ok(fitted)
}

pub fn plan_fit(
    document: &Document,
    faces: &[FaceId],
    options: FitOptions,
    size: [u32; 2],
) -> Result<Vec<(FaceId, SurfaceEdit)>, EditError> {
    Ok(plan_layout(document, faces, options, size)?.edits)
}

pub fn plan_layout(
    document: &Document,
    faces: &[FaceId],
    options: FitOptions,
    size: [u32; 2],
) -> Result<FitPlan, EditError> {
    if matches!(options.mode, FitMode::Pack(_)) && (faces.is_empty() || faces.len() > MAX_FACES) {
        return Err(EditError::Invalid(format!(
            "Atlas packing needs between 1 and {MAX_FACES} flat faces"
        )));
    }
    let usable = options.region.usable_rectangle(size)?;
    let samples = faces
        .iter()
        .map(|face| surface::sample(document, *face))
        .collect::<Result<Vec<_>, _>>()?;
    if samples.iter().any(|sample| sample.displaced) {
        return Err(EditError::Invalid(
            "Atlas fit supports flat brush polygons; terrain faces are unsupported".into(),
        ));
    }
    let edits = match options.mode {
        FitMode::PerFace => samples
            .iter()
            .map(|sample| {
                let projection = fit_region_projection(
                    &sample.projection,
                    &sample.vertices,
                    options.region,
                    size,
                )?;
                Ok((
                    sample.face,
                    SurfaceEdit {
                        projection: Some(projection),
                        ..SurfaceEdit::default()
                    },
                ))
            })
            .collect::<Result<Vec<_>, EditError>>(),
        FitMode::Together { reference } => {
            let mut edits = surface_group::project(
                document,
                faces,
                surface_group::Basis::Face(reference),
                [usable.width, usable.height],
                DVec2::ONE,
            )?;
            for (_, edit) in &mut edits {
                let projection = edit.projection.as_mut().expect("planned group projection");
                projection.u.offset += f64::from(usable.x);
                projection.v.offset += f64::from(usable.y);
            }
            Ok(edits)
        }
        FitMode::Pack(options) => return packing::plan(&samples, usable, options),
    }?;
    Ok(FitPlan {
        edits,
        islands: Vec::new(),
    })
}
