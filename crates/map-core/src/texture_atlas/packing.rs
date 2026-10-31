use std::collections::BTreeSet;

use crate::EditError;
use crate::surface::{Justify, Projection, SurfaceEdit, SurfaceSample, placed_projection};

use super::{FitPlan, FitRegion, Rectangle};

const MAX_FREE_RECTANGLES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PackOptions {
    pub scale: f64,
    pub padding: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackedIsland {
    pub face: crate::surface::FaceId,
    pub region: FitRegion,
    pub texel_size: [f64; 2],
}

struct Island {
    layout: PackedIsland,
    projection: Projection,
}

fn invalid(message: &str) -> EditError {
    EditError::Invalid(message.into())
}

fn island(sample: &SurfaceSample, options: PackOptions) -> Result<Island, EditError> {
    let justify = [Some((Justify::Start, 1)); 2];
    let mut projection = placed_projection(&sample.projection, &sample.vertices, None, justify)?;
    for axis in [&mut projection.u, &mut projection.v] {
        axis.scale /= options.scale;
        axis.offset *= options.scale;
    }
    let projection = placed_projection(&projection, &sample.vertices, None, justify)?;
    let texel_size = [&projection.u, &projection.v].map(|axis| {
        let (lo, hi) = sample
            .vertices
            .iter()
            .map(|point| point.dot(axis.axis) / axis.scale + axis.offset)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), value| {
                (lo.min(value), hi.max(value))
            });
        hi - lo
    });
    let inset = options
        .padding
        .checked_mul(2)
        .ok_or_else(|| invalid("Island padding exceeds the supported pixel range"))?;
    let mut size = [0; 2];
    for (axis, span) in texel_size.iter().copied().enumerate() {
        if !span.is_finite() || span <= 0.0 || span.ceil() > f64::from(u32::MAX - inset) {
            return Err(invalid(
                "A packed island has invalid or oversized projected bounds",
            ));
        }
        size[axis] = (span.ceil() as u32)
            .checked_add(inset)
            .ok_or_else(|| invalid("A packed island exceeds the supported pixel range"))?;
    }
    Ok(Island {
        layout: PackedIsland {
            face: sample.face,
            region: FitRegion {
                rectangle: Rectangle {
                    x: 0,
                    y: 0,
                    width: size[0],
                    height: size[1],
                },
                padding: options.padding,
            },
            texel_size,
        },
        projection,
    })
}

fn area(rectangle: Rectangle) -> u64 {
    u64::from(rectangle.width) * u64::from(rectangle.height)
}

fn contains(outer: Rectangle, inner: Rectangle) -> bool {
    outer.x <= inner.x
        && outer.y <= inner.y
        && outer.x + outer.width >= inner.x + inner.width
        && outer.y + outer.height >= inner.y + inner.height
}

fn split_free(free: &mut Vec<Rectangle>, placed: Rectangle) -> Result<(), EditError> {
    let mut split = Vec::new();
    let right = placed.x + placed.width;
    let bottom = placed.y + placed.height;
    for rectangle in free.drain(..) {
        let free_right = rectangle.x + rectangle.width;
        let free_bottom = rectangle.y + rectangle.height;
        if placed.x >= free_right
            || right <= rectangle.x
            || placed.y >= free_bottom
            || bottom <= rectangle.y
        {
            split.push(rectangle);
            continue;
        }
        if placed.x > rectangle.x {
            split.push(Rectangle {
                width: placed.x - rectangle.x,
                ..rectangle
            });
        }
        if right < free_right {
            split.push(Rectangle {
                x: right,
                width: free_right - right,
                ..rectangle
            });
        }
        if placed.y > rectangle.y {
            split.push(Rectangle {
                height: placed.y - rectangle.y,
                ..rectangle
            });
        }
        if bottom < free_bottom {
            split.push(Rectangle {
                y: bottom,
                height: free_bottom - bottom,
                ..rectangle
            });
        }
    }
    split.sort_by_key(|rectangle| (rectangle.y, rectangle.x, rectangle.width, rectangle.height));
    split.dedup();
    *free = split
        .iter()
        .enumerate()
        .filter(|(index, rectangle)| {
            !split
                .iter()
                .enumerate()
                .any(|(other, container)| *index != other && contains(*container, **rectangle))
        })
        .map(|(_, rectangle)| *rectangle)
        .collect();
    if free.len() > MAX_FREE_RECTANGLES {
        return Err(invalid(
            "Atlas packing exceeded its free-rectangle budget; pack fewer faces",
        ));
    }
    Ok(())
}

pub(super) fn plan(
    samples: &[SurfaceSample],
    usable: Rectangle,
    options: PackOptions,
) -> Result<FitPlan, EditError> {
    if !options.scale.is_finite() || options.scale <= 0.0 {
        return Err(invalid(
            "Island UV size must be finite and greater than zero",
        ));
    }
    let unique: BTreeSet<_> = samples.iter().map(|sample| sample.face).collect();
    if unique.len() != samples.len() {
        return Err(invalid("Atlas packing targets must not repeat a face"));
    }
    let mut pending = samples
        .iter()
        .map(|sample| island(sample, options))
        .collect::<Result<Vec<_>, _>>()?;
    pending.sort_by(|a, b| {
        let a_rectangle = a.layout.region.rectangle;
        let b_rectangle = b.layout.region.rectangle;
        area(b_rectangle)
            .cmp(&area(a_rectangle))
            .then_with(|| {
                b_rectangle
                    .width
                    .max(b_rectangle.height)
                    .cmp(&a_rectangle.width.max(a_rectangle.height))
            })
            .then_with(|| a.layout.face.cmp(&b.layout.face))
    });
    let total_area = pending.iter().try_fold(0u64, |total, island| {
        total.checked_add(area(island.layout.region.rectangle))
    });
    let total_area = total_area
        .ok_or_else(|| invalid("Packed islands exceed the supported total pixel area"))?;
    if total_area > area(usable) {
        return Err(invalid(
            "Packed islands and their padding exceed the usable atlas area; reduce Island UV size or island padding, or choose a larger region",
        ));
    }
    let mut free = vec![usable];
    let mut edits = Vec::with_capacity(pending.len());
    let mut islands = Vec::with_capacity(pending.len());
    for mut island in pending {
        let size = island.layout.region.rectangle;
        let location = free
            .iter()
            .filter(|rectangle| rectangle.width >= size.width && rectangle.height >= size.height)
            .min_by_key(|rectangle| {
                let width = rectangle.width - size.width;
                let height = rectangle.height - size.height;
                (width.min(height), width.max(height), rectangle.y, rectangle.x, rectangle.width, rectangle.height)
            })
            .ok_or_else(|| invalid("The nonrotating rectangular packer found no layout at the requested UV size and padding; reduce Island UV size or island padding, or choose a larger region"))?;
        let placed = Rectangle {
            x: location.x,
            y: location.y,
            ..size
        };
        split_free(&mut free, placed)?;
        island.layout.region.rectangle = placed;
        island.projection.u.offset += f64::from(placed.x + options.padding);
        island.projection.v.offset += f64::from(placed.y + options.padding);
        edits.push((
            island.layout.face,
            SurfaceEdit {
                projection: Some(island.projection),
                ..SurfaceEdit::default()
            },
        ));
        islands.push(island.layout);
    }
    edits.sort_by_key(|(face, _)| *face);
    islands.sort_by_key(|island| island.face);
    Ok(FitPlan { edits, islands })
}
