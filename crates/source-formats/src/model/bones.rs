use std::io;

use glam::{DAffine3, DQuat, DVec3};

use super::{invalid, reader::Reader};

pub(super) const MAX_BONES: usize = 256;
const WEIGHT_EPSILON: f64 = 4.0 * f32::EPSILON as f64;

fn resolve(
    index: usize,
    parents: &[Option<usize>],
    local: &[DAffine3],
    global: &mut [DAffine3],
    state: &mut [u8],
) -> io::Result<DAffine3> {
    match state[index] {
        2 => return Ok(global[index]),
        1 => return Err(invalid("MDL bone parents contain a cycle")),
        _ => {}
    }
    state[index] = 1;
    global[index] = match parents[index] {
        Some(parent) => resolve(parent, parents, local, global, state)? * local[index],
        None => local[index],
    };
    state[index] = 2;
    Ok(global[index])
}

pub struct Reference {
    pub skinning: Vec<DAffine3>,
    pub poses: Vec<DAffine3>,
}

pub fn reference(reader: &Reader<'_>) -> io::Result<Reference> {
    let bones = reader.table(0, 156, 160, 216, MAX_BONES)?;
    if bones.len == 0 {
        return Err(invalid(
            "Reference-pose models need a bounded bone hierarchy",
        ));
    }
    let mut parents = Vec::with_capacity(bones.len);
    let mut local = Vec::with_capacity(bones.len);
    let mut inverse_bind = Vec::with_capacity(bones.len);
    for index in 0..bones.len {
        let base = bones.at(index)?;
        parents.push(match reader.int(base + 4)? {
            -1 => None,
            parent if parent >= 0 && (parent as usize) < bones.len => Some(parent as usize),
            _ => return Err(invalid("MDL bone parent is outside its hierarchy")),
        });
        let position = DVec3::from_array(reader.vector(base + 32)?.map(f64::from));
        let rotation = DQuat::from_xyzw(
            f64::from(reader.float(base + 44)?),
            f64::from(reader.float(base + 48)?),
            f64::from(reader.float(base + 52)?),
            f64::from(reader.float(base + 56)?),
        );
        if (rotation.length_squared() - 1.0).abs() > 0.01 {
            return Err(invalid("MDL reference bone quaternion must be unit length"));
        }
        local.push(DAffine3::from_rotation_translation(
            rotation.normalize(),
            position,
        ));
        inverse_bind.push(reader.affine(base + 96)?);
    }
    let mut global = vec![DAffine3::IDENTITY; bones.len];
    let mut state = vec![0; bones.len];
    for index in 0..bones.len {
        resolve(index, &parents, &local, &mut global, &mut state)?;
    }
    let skinning = global
        .iter()
        .zip(inverse_bind)
        .map(|(pose, bind)| *pose * bind)
        .collect();
    Ok(Reference {
        skinning,
        poses: global,
    })
}

pub fn vertex(
    reader: &Reader<'_>,
    offset: usize,
    transforms: &[DAffine3],
    position: [f32; 3],
    normal: [f32; 3],
) -> io::Result<([f32; 3], [f32; 3])> {
    let count = usize::from(reader.byte(offset + 15)?);
    if !(1..=3).contains(&count) {
        return Err(invalid(
            "VVD reference vertices need one to three bone weights",
        ));
    }
    let position = DVec3::from_array(position.map(f64::from));
    let normal = DVec3::from_array(normal.map(f64::from));
    let mut point = DVec3::ZERO;
    let mut direction = DVec3::ZERO;
    let mut total = 0.0;
    for index in 0..count {
        let weight = f64::from(reader.float(offset + index * 4)?);
        if !(-WEIGHT_EPSILON..=1.0 + WEIGHT_EPSILON).contains(&weight) {
            return Err(invalid(
                "VVD bone weights exceed unit-range precision tolerance",
            ));
        }
        let bone = usize::from(reader.byte(offset + 12 + index)?);
        let transform = transforms
            .get(bone)
            .ok_or_else(|| invalid("VVD bone index is outside the model hierarchy"))?;
        point += transform.transform_point3(position) * weight;
        direction += transform.transform_vector3(normal) * weight;
        total += weight;
    }
    if (total - 1.0).abs() > 0.001 || !point.is_finite() || point.abs().max_element() > 65536.0 {
        return Err(invalid(
            "VVD reference weights or transformed position exceed bounds",
        ));
    }
    let direction = direction
        .try_normalize()
        .ok_or_else(|| invalid("VVD reference normal is degenerate"))?;
    Ok((point.as_vec3().to_array(), direction.as_vec3().to_array()))
}
