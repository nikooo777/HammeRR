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
