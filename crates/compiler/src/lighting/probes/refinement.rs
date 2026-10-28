use source_formats::bsp::{Leaf, LeafAmbientSample};

use super::super::bake::Budget;

pub(super) const MAX_POOL: usize = 16;
const COMPONENTS: usize = 18;
const NATIVE_BYTE_SCALE: f32 = f32::from_bits(0x3c008081);
const DECODE_WORK: usize = 256;
const WEIGHT_WORK: usize = 32;
const CONTRIBUTION_WORK: usize = COMPONENTS * 2 + 1;
const ERROR_WORK: usize = COMPONENTS * 4 + 1;

struct Scratch {
    colors: [[f32; COMPONENTS]; MAX_POOL],
    weights: [[f32; MAX_POOL]; MAX_POOL],
}

pub(super) const SCRATCH_BYTES: usize = size_of::<Scratch>()
    + size_of::<[f32; COMPONENTS]>()
    + 3 * size_of::<[f32; 3]>()
    + 3 * size_of::<Vec<()>>();

fn position(sample: &LeafAmbientSample, center: [f32; 3], half: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|axis| {
        (f32::from(sample.position[axis]) * (NATIVE_BYTE_SCALE * half[axis]) - half[axis])
            + center[axis]
    })
}

fn weight(sample: &LeafAmbientSample, query: [f32; 3], center: [f32; 3], half: [f32; 3]) -> f32 {
    let delta: [f32; 3] = std::array::from_fn(|axis| {
        (f32::from(sample.position[axis]) * (NATIVE_BYTE_SCALE * half[axis]) - half[axis])
            + (center[axis] - query[axis])
    });
    let denominator = (delta[2] * delta[2] + 1.0) + (delta[1] * delta[1] + delta[0] * delta[0]);
    1.0 / denominator
}

fn score(
    scratch: &Scratch,
    pool: usize,
    count: usize,
    last: usize,
    budget: &mut Budget<'_>,
) -> Result<f32, String> {
    let mut error = 0.0_f32;
    for (weights, reference) in scratch.weights.iter().zip(&scratch.colors).take(pool) {
        let mut predicted = [0.0_f32; COMPONENTS];
        let mut total = 0.0_f32;
        for slot in 0..count {
            budget.consume(CONTRIBUTION_WORK)?;
            let sample = if slot + 1 == count { last } else { slot };
            let weight = weights[sample];
            total += weight;
            for (value, color) in predicted.iter_mut().zip(scratch.colors[sample]) {
                *value += color * weight;
            }
        }
        budget.consume(ERROR_WORK)?;
        let reciprocal = 1.0 / total;
        for (value, reference) in predicted.into_iter().zip(*reference) {
            let difference = (value * reciprocal - reference).abs();
            if !difference.is_finite() {
                return Err(
                    "Ambient refinement requires finite native reconstruction errors".into(),
                );
            }
            error = error.max(difference);
        }
    }
    Ok(error)
}

pub(super) fn choose(
    leaf: &Leaf,
    samples: &[LeafAmbientSample],
    count: usize,
    budget: &mut Budget<'_>,
) -> Result<usize, String> {
    if !matches!(count, 4 | 8) || samples.len() <= count || samples.len() > count * 2 {
        return Err("Ambient refinement requires a bounded 4/8-probe candidate prefix".into());
    }
    budget.consume(samples.len() * 6)?;
    if samples.iter().all(|sample| sample.cube == samples[0].cube) {
        return Ok(count - 1);
    }
    budget.consume(9)?;
    let center =
        std::array::from_fn(|axis| (f32::from(leaf.mins[axis]) + f32::from(leaf.maxs[axis])) * 0.5);
    let half = std::array::from_fn(|axis| f32::from(leaf.maxs[axis]) - center[axis]);
    budget.consume(MAX_POOL * (COMPONENTS + MAX_POOL))?;
    let mut scratch = Scratch {
        colors: [[0.0; COMPONENTS]; MAX_POOL],
        weights: [[0.0; MAX_POOL]; MAX_POOL],
    };
    for (index, sample) in samples.iter().enumerate() {
        budget.consume(DECODE_WORK)?;
        for (face, color) in sample.cube.iter().enumerate() {
            let exponent = (2.0_f64.powi(i32::from(color.exponent)) / 255.0) as f32;
            for (channel, byte) in color.rgb.iter().enumerate() {
                let value = (f32::from(*byte) * 255.0) * exponent;
                if !value.is_finite() {
                    return Err("Ambient refinement requires finite native RGBExp colors".into());
                }
                scratch.colors[index][face * 3 + channel] = value;
            }
        }
        let query = position(sample, center, half);
        for (other, source) in samples.iter().enumerate() {
            budget.consume(WEIGHT_WORK)?;
            scratch.weights[index][other] = weight(source, query, center, half);
        }
    }
    let mut best = count - 1;
    let mut error = score(&scratch, samples.len(), count, best, budget)?;
    for candidate in count..samples.len() {
        budget.consume(1)?;
        let candidate_error = score(&scratch, samples.len(), count, candidate, budget)?;
        if candidate_error < error {
            best = candidate;
            error = candidate_error;
        }
    }
    Ok(best)
}
