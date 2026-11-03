use glam::DVec3;

const MARKER_HALF_SIZE: f64 = 4.0;
const CYLINDER_SEGMENTS: usize = 32;
const MIN_CYLINDER_LENGTH: f64 = 0.001;

pub(super) fn marker(segments: &mut Vec<[[f64; 3]; 2]>, point: DVec3) {
    for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
        segments.push([
            (point - axis * MARKER_HALF_SIZE).to_array(),
            (point + axis * MARKER_HALF_SIZE).to_array(),
        ]);
    }
}

pub(super) fn wirebox(mins: DVec3, maxs: DVec3) -> Vec<[[f64; 3]; 2]> {
    let points: [DVec3; 8] = std::array::from_fn(|index| {
        DVec3::new(
            if index & 1 == 0 { mins.x } else { maxs.x },
            if index & 2 == 0 { mins.y } else { maxs.y },
            if index & 4 == 0 { mins.z } else { maxs.z },
        )
    });
    let mut segments = Vec::with_capacity(12);
    for index in 0..8 {
        for axis in [1, 2, 4] {
            if index & axis == 0 {
                segments.push([points[index].to_array(), points[index | axis].to_array()]);
            }
        }
    }
    segments
}

pub(super) fn cylinder(points: [DVec3; 2], radii: [f64; 2]) -> Result<Vec<[[f64; 3]; 2]>, String> {
    let axis = points[1] - points[0];
    let length = axis.length();
    if !length.is_finite()
        || length < MIN_CYLINDER_LENGTH
        || points[0].as_vec3() == points[1].as_vec3()
    {
        return Err(
            "Cylinder endpoints need distinct finite native positions at least 0.001 units apart"
                .into(),
        );
    }
    if radii == [0.0; 2] {
        return Ok(vec![points.map(|point| point.to_array())]);
    }
    let axis = axis / length;
    let reference = if axis.z.abs() < 0.9 {
        DVec3::Z
    } else {
        DVec3::Y
    };
    let u = axis.cross(reference).normalize();
    let v = axis.cross(u).normalize();
    let ring = |end: usize, sample: usize| {
        let angle = sample as f64 * std::f64::consts::TAU / CYLINDER_SEGMENTS as f64;
        (points[end] + (u * angle.cos() + v * angle.sin()) * radii[end]).to_array()
    };
    let mut segments = Vec::with_capacity(super::MAX_SEGMENTS_PER_GUIDE);
    for sample in 0..CYLINDER_SEGMENTS {
        for (end, radius) in radii.iter().enumerate() {
            if *radius > 0.0 {
                segments.push([
                    ring(end, sample),
                    ring(end, (sample + 1) % CYLINDER_SEGMENTS),
                ]);
            }
        }
        segments.push([ring(0, sample), ring(1, sample)]);
    }
    Ok(segments)
}
