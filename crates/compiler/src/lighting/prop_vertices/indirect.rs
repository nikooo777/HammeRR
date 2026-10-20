use glam::DVec3;

use super::super::ReceiverAngularMode;

const IGNORE_NORMALS_NORMALIZATION: f64 = 0.35355_f32 as f64;

pub(super) fn cone(mode: ReceiverAngularMode, count: usize) -> f64 {
    if count == 0 {
        return 0.0;
    }
    let coverage = match mode {
        ReceiverAngularMode::Normal => 1.0,
        ReceiverAngularMode::IgnoreNormals => 2.0,
    };
    (1.0 - coverage / count as f64).acos().tan()
}

pub(super) fn gather(
    mode: ReceiverAngularMode,
    normal: DVec3,
    hemisphere: &[DVec3],
    sphere: &[DVec3],
    sample: &mut impl FnMut(DVec3) -> Result<DVec3, String>,
) -> Result<DVec3, String> {
    match mode {
        ReceiverAngularMode::Normal => super::super::bounce::gather(normal, hemisphere, sample),
        ReceiverAngularMode::IgnoreNormals => {
            let _normal = normal
                .try_normalize()
                .ok_or("Bounce receiver needs a finite nonzero normal")?;
            if sphere.is_empty() {
                return Err("Ignored-normal indirect lighting needs sphere directions".into());
            }
            let mut total = DVec3::ZERO;
            for &direction in sphere {
                total += sample(direction)?;
            }
            let color = total / (sphere.len() as f64 * IGNORE_NORMALS_NORMALIZATION);
            if !color.is_finite() || color.min_element() < 0.0 {
                return Err(
                    "Ignored-normal indirect lighting must be finite and nonnegative".into(),
                );
            }
            Ok(color)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SkyQuality;

    #[test]
    fn both_prop_reception_kernels_gather_actual_transmitted_source_hits() {
        let control = crate::Control::default();
        let mut budget = super::super::Budget::new(&control);
        let (surfaces, shadows, props) =
            super::super::super::radiance::receiver_tests::transmitted_source(&mut budget);
        let hemisphere = super::super::super::bounce::directions(32);
        let sphere = super::super::super::sky::ambient_directions(SkyQuality::Fast);
        for (mode, expected) in [
            (
                ReceiverAngularMode::Normal,
                DVec3::new(0.7490196078431373, 0.3759784698193003, 0.18798923490965015),
            ),
            (
                ReceiverAngularMode::IgnoreNormals,
                DVec3::new(1.0592838851928694, 0.5317189698223032, 0.2658594849111516),
            ),
        ] {
            let color = gather(mode, -DVec3::Z, &hemisphere, &sphere, &mut |direction| {
                Ok(surfaces
                    .trace(
                        DVec3::Z * 16.0,
                        direction,
                        0.0,
                        &shadows,
                        &props,
                        &mut budget,
                    )?
                    .map_or(DVec3::ZERO, |hit| hit.color))
            })
            .unwrap();
            assert!(color.abs_diff_eq(expected, 1e-12), "{color:?}");
        }
    }

    #[test]
    fn sdk_sphere_normalization_keeps_all_rays_and_ignores_valid_normal_orientation() {
        assert_eq!((IGNORE_NORMALS_NORMALIZATION as f32).to_bits(), 0x3eb5_0481);
        for quality in [SkyQuality::Fast, SkyQuality::Balanced, SkyQuality::Detailed] {
            let sphere = super::super::super::sky::ambient_directions(quality);
            for normal in [DVec3::Z, -DVec3::Z, DVec3::new(1.0, 2.0, -3.0)] {
                for (half, expected) in [
                    (
                        false,
                        DVec3::new(181.02107860049873, 90.51053930024936, 45.25526965012468),
                    ),
                    (
                        true,
                        DVec3::new(90.51053930024936, 45.25526965012468, 22.62763482506234),
                    ),
                ] {
                    let mut calls = 0;
                    let value = gather(
                        ReceiverAngularMode::IgnoreNormals,
                        normal,
                        &[],
                        &sphere,
                        &mut |direction| {
                            calls += 1;
                            Ok(if !half || direction.z > 0.0 {
                                DVec3::new(64.0, 32.0, 16.0)
                            } else {
                                DVec3::ZERO
                            })
                        },
                    )
                    .unwrap();
                    assert!(value.abs_diff_eq(expected, 1e-12), "{value:?}");
                    assert_eq!(calls, quality.directions());
                }
            }
            let zero = gather(
                ReceiverAngularMode::IgnoreNormals,
                DVec3::Z,
                &[],
                &sphere,
                &mut |_| Ok(DVec3::ZERO),
            )
            .unwrap();
            assert_eq!(zero, DVec3::ZERO);
        }
        for normal in [DVec3::ZERO, DVec3::splat(f64::NAN)] {
            assert!(
                gather(
                    ReceiverAngularMode::IgnoreNormals,
                    normal,
                    &[],
                    &[DVec3::Z],
                    &mut |_| Ok(DVec3::ONE)
                )
                .is_err()
            );
        }
        assert!(
            gather(
                ReceiverAngularMode::IgnoreNormals,
                DVec3::Z,
                &[],
                &[],
                &mut |_| Ok(DVec3::ONE)
            )
            .is_err()
        );
    }

    #[test]
    fn sphere_and_hemisphere_keep_their_distinct_solid_angle_filter_cones() {
        assert!((cone(ReceiverAngularMode::IgnoreNormals, 32) - 0.3711842908553348).abs() < 1e-15);
        assert!((cone(ReceiverAngularMode::Normal, 32) - 0.2560404494578636).abs() < 1e-15);
        assert_eq!(cone(ReceiverAngularMode::Normal, 0), 0.0);
    }

    #[test]
    fn sphere_gather_propagates_mid_sample_cancellation_and_shared_work_failure() {
        let sphere = super::super::super::sky::ambient_directions(SkyQuality::Fast);
        for canceled in [false, true] {
            let control = crate::Control::default();
            let mut budget = super::super::Budget::new(&control);
            if !canceled {
                budget.consume(100_000_000 - 1).unwrap();
            }
            let mut calls = 0;
            let result = gather(
                ReceiverAngularMode::IgnoreNormals,
                DVec3::Z,
                &[],
                &sphere,
                &mut |_| {
                    calls += 1;
                    if canceled && calls == 2 {
                        control.cancel().unwrap();
                    }
                    budget.consume(1)?;
                    Ok(DVec3::splat(64.0))
                },
            );
            assert!(result.is_err());
            assert_eq!(calls, 2);
        }
    }
}
