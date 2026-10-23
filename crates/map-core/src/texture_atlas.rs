use glam::DVec3;

use crate::EditError;
use crate::surface::{Projection, placed_projection};

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

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec2;
    use source_formats::vmf::UvAxis;

    #[test]
    fn atlas_fit_preserves_rotated_skewed_and_mirrored_bases_on_triangle_bounds() {
        let points = [
            DVec3::new(10.0, 20.0, 4.0),
            DVec3::new(138.0, 20.0, 4.0),
            DVec3::new(42.0, 100.0, 4.0),
        ];
        let rectangle = Rectangle {
            x: 16,
            y: 32,
            width: 48,
            height: 64,
        };
        for sign in [-1.0, 1.0] {
            let original = Projection {
                u: UvAxis {
                    axis: DVec3::new(0.8, 0.6, 0.0),
                    offset: 3.0,
                    scale: sign * 0.25,
                },
                v: UvAxis {
                    axis: DVec3::new(-0.2, 1.0, 0.0),
                    offset: -7.0,
                    scale: 0.5,
                },
                rotation: 37.0,
            };
            let fitted = fit_projection(&original, &points, rectangle, [128, 256]).unwrap();
            assert_eq!(fitted.u.axis, original.u.axis);
            assert_eq!(fitted.v.axis, original.v.axis);
            assert_eq!(fitted.rotation, original.rotation);
            assert_eq!(fitted.u.scale.signum(), sign);
            let (lo, hi) = points
                .iter()
                .map(|point| {
                    DVec2::new(
                        point.dot(fitted.u.axis) / fitted.u.scale + fitted.u.offset,
                        point.dot(fitted.v.axis) / fitted.v.scale + fitted.v.offset,
                    )
                })
                .fold(
                    (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
                    |(lo, hi), uv| (lo.min(uv), hi.max(uv)),
                );
            assert!(lo.abs_diff_eq(DVec2::new(16.0, 32.0), 1e-8));
            assert!(hi.abs_diff_eq(DVec2::new(64.0, 96.0), 1e-8));
        }
        assert_eq!(
            rectangle.uv_bounds([128, 256]).unwrap(),
            [[0.125, 0.125], [0.5, 0.375]]
        );
    }

    #[test]
    fn atlas_fit_rejects_empty_outside_and_overflowing_regions() {
        for rectangle in [
            Rectangle {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            },
            Rectangle {
                x: 0,
                y: 255,
                width: 1,
                height: 2,
            },
            Rectangle {
                x: u32::MAX,
                y: 0,
                width: 1,
                height: 1,
            },
        ] {
            assert!(rectangle.validate([128, 256]).is_err());
        }
    }

    #[test]
    fn padded_atlas_fit_has_literal_skewed_mirrored_corners_and_zero_padding_parity() {
        let points = [
            DVec3::new(0.0, 0.0, 4.0),
            DVec3::new(4.0, 0.0, 4.0),
            DVec3::new(4.0, 2.0, 4.0),
            DVec3::new(0.0, 2.0, 4.0),
        ];
        let original = Projection {
            u: UvAxis {
                axis: DVec3::new(1.0, 1.0, 0.0),
                offset: 3.0,
                scale: -0.25,
            },
            v: UvAxis {
                axis: DVec3::new(-1.0, 2.0, 0.0),
                offset: -7.0,
                scale: 0.5,
            },
            rotation: 37.0,
        };
        let rectangle = Rectangle {
            x: 16,
            y: 32,
            width: 48,
            height: 64,
        };
        let fitted = fit_region_projection(
            &original,
            &points,
            FitRegion {
                rectangle,
                padding: 4,
            },
            [128, 256],
        )
        .unwrap();
        assert_eq!(fitted.u.axis, original.u.axis);
        assert_eq!(fitted.v.axis, original.v.axis);
        assert_eq!(fitted.rotation, original.rotation);
        assert!(fitted.u.scale.is_sign_negative());
        assert!(fitted.v.scale.is_sign_positive());
        for (point, expected) in points.into_iter().zip([
            DVec2::new(60.0, 64.0),
            DVec2::new(100.0 / 3.0, 36.0),
            DVec2::new(20.0, 64.0),
            DVec2::new(140.0 / 3.0, 92.0),
        ]) {
            let actual = DVec2::new(
                point.dot(fitted.u.axis) / fitted.u.scale + fitted.u.offset,
                point.dot(fitted.v.axis) / fitted.v.scale + fitted.v.offset,
            );
            assert!(
                actual.abs_diff_eq(expected, 1e-10),
                "{actual} != {expected}"
            );
        }
        assert_eq!(
            fit_region_projection(
                &original,
                &points,
                FitRegion {
                    rectangle,
                    padding: 0,
                },
                [128, 256],
            )
            .unwrap(),
            fit_projection(&original, &points, rectangle, [128, 256]).unwrap()
        );
    }

    #[test]
    fn atlas_padding_checks_outer_image_bounds_and_positive_usable_area() {
        let rectangle = Rectangle {
            x: 16,
            y: 32,
            width: 7,
            height: 5,
        };
        assert_eq!(
            FitRegion {
                rectangle,
                padding: 2,
            }
            .usable_rectangle([128, 256])
            .unwrap(),
            Rectangle {
                x: 18,
                y: 34,
                width: 3,
                height: 1,
            }
        );
        for region in [
            FitRegion {
                rectangle,
                padding: 3,
            },
            FitRegion {
                rectangle,
                padding: u32::MAX,
            },
            FitRegion {
                rectangle: Rectangle {
                    height: 4,
                    ..rectangle
                },
                padding: 2,
            },
            FitRegion {
                rectangle: Rectangle {
                    width: 0,
                    ..rectangle
                },
                padding: 0,
            },
            FitRegion {
                rectangle: Rectangle {
                    x: 122,
                    y: 0,
                    width: 8,
                    height: 8,
                },
                padding: 2,
            },
        ] {
            assert!(region.usable_rectangle([128, 256]).is_err(), "{region:?}");
        }
    }
}
