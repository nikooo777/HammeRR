use glam::{DMat3, DMat4, DVec3};
use map_core::Plane;
use source_formats::bsp::{Bsp, MAX_LIGHTING_FACES, Model, lump};
use source_formats::keyvalues::{FlatBlockLimits, parse_flat_blocks_controlled};

pub(crate) const MAX_INITIAL_POSE_BYTES: usize = 128;
const MAX_NATIVE_POSE_BYTES: usize = MAX_INITIAL_POSE_BYTES - 1;
const MAX_INITIAL_KEY_BYTES: usize = 2047;
const MAX_INITIAL_ANGLE: f64 = 360.0;
const MAX_INITIAL_ENTITIES_BYTES: usize = 16 * 1024 * 1024;
const MAX_INITIAL_ENTITY_PAIRS: usize = 262_144;

pub fn initial_brush_model_transforms(bsp: &Bsp) -> Result<Vec<DMat4>, String> {
    initial_brush_model_transforms_controlled(bsp, &mut || Ok(()))
}

pub fn initial_brush_model_transforms_controlled(
    bsp: &Bsp,
    check: &mut dyn FnMut() -> std::io::Result<()>,
) -> Result<Vec<DMat4>, String> {
    check().map_err(|error| error.to_string())?;
    let model_lump = bsp.lump(lump::MODELS);
    if model_lump.version != 0
        || model_lump.fourcc != [0; 4]
        || model_lump.data.len() / size_of::<Model>() > MAX_LIGHTING_FACES
    {
        return Err("Unsupported compiled model metadata or count".into());
    }
    let models = bsp.records::<Model>().map_err(|error| error.to_string())?;
    let world = models.first().ok_or("Compiled Model0 is unavailable")?;
    if world.origin != [0.0; 3] {
        return Err("Compiled Model0 has a nonzero or invalid origin".into());
    }
    let entity_lump = bsp.lump(lump::ENTITIES);
    if entity_lump.version != 0
        || entity_lump.fourcc != [0; 4]
        || entity_lump.data.len() > MAX_INITIAL_ENTITIES_BYTES
    {
        return Err("Unsupported compiled entity metadata or size".into());
    }
    let entities = parse_flat_blocks_controlled(
        bsp.entities().map_err(|error| error.to_string())?,
        FlatBlockLimits {
            max_bytes: MAX_INITIAL_ENTITIES_BYTES,
            max_blocks: MAX_LIGHTING_FACES,
            max_pairs: MAX_INITIAL_ENTITY_PAIRS,
        },
        check,
    )
    .map_err(|error| error.to_string())?;
    let mut transforms = vec![None; models.len()];
    transforms[0] = Some(DMat4::IDENTITY);
    for properties in entities {
        check().map_err(|error| error.to_string())?;
        let fields = properties
            .iter()
            .filter(|(key, _)| pose_key(key).is_ok_and(|base| base.eq_ignore_ascii_case("model")))
            .collect::<Vec<_>>();
        if !fields.iter().any(|(_, value)| value.starts_with('*')) {
            continue;
        }
        if fields.len() != 1 {
            return Err("Compiled brush model has duplicate or ambiguous model fields".into());
        }
        let (key, value) = fields[0];
        if pose_key(key)?.len() != key.len() {
            return Err("Compiled brush model requires an unaliased model field".into());
        }
        let ordinal = value
            .strip_prefix('*')
            .filter(|ordinal| {
                !ordinal.is_empty() && ordinal.bytes().all(|byte| byte.is_ascii_digit())
            })
            .ok_or("Compiled brush model binding is malformed")?
            .parse::<usize>()
            .map_err(|_| "Compiled brush model binding is out of range")?;
        if ordinal == 0 || ordinal >= models.len() {
            return Err("Compiled brush model binding is out of range".into());
        }
        if transforms[ordinal].is_some() {
            return Err("Compiled brush model is bound to more than one entity".into());
        }
        let mut classname = None;
        for (key, value) in &properties {
            let base = pose_key(key)?;
            if base.eq_ignore_ascii_case("classname") {
                if base.len() != key.len() || classname.is_some() {
                    return Err("Compiled brush model has ambiguous classname fields".into());
                }
                classname = Some(value.as_str());
            }
        }
        let classname = classname.ok_or("Compiled brush model classname is missing")?;
        let pose = Pose {
            offset: initial_offset(&properties)?,
            rotation: initial_rotation(classname, &properties)?,
        };
        let model = &models[ordinal];
        pose.bounds(
            DVec3::from_array(model.mins.map(f64::from)),
            DVec3::from_array(model.maxs.map(f64::from)),
        )?;
        let mut transform = pose.rotation.map_or(DMat4::IDENTITY, DMat4::from_mat3);
        transform.w_axis = pose.offset.extend(1.0);
        transforms[ordinal] = Some(transform);
    }
    transforms
        .into_iter()
        .enumerate()
        .map(|(ordinal, transform)| {
            transform.ok_or_else(|| {
                format!("Compiled brush model *{ordinal} has no emitted entity binding")
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Pose {
    pub offset: DVec3,
    pub rotation: Option<DMat3>,
}

impl Pose {
    pub(crate) fn point(self, local: DVec3) -> DVec3 {
        match self.rotation {
            Some(rotation) => rotation * local + self.offset,
            None => local + self.offset,
        }
    }

    pub(crate) fn local_point(self, world: DVec3) -> DVec3 {
        let translated = world - self.offset;
        match self.rotation {
            Some(rotation) => rotation.transpose() * translated,
            None => translated,
        }
    }

    pub(crate) fn normal(self, local: DVec3) -> DVec3 {
        match self.rotation {
            Some(rotation) => rotation * local,
            None => local,
        }
    }

    pub(crate) fn plane(self, local: Plane) -> Plane {
        let normal = self.normal(local.normal);
        Plane {
            normal,
            dist: local.dist + normal.dot(self.offset),
        }
    }

    pub(crate) fn bounds(self, mins: DVec3, maxs: DVec3) -> Result<(DVec3, DVec3), String> {
        if !mins.is_finite() || !maxs.is_finite() || mins.cmpgt(maxs).any() {
            return Err("initial pose requires finite ordered model bounds".into());
        }
        let bounds = if self.rotation.is_none() {
            (mins + self.offset, maxs + self.offset)
        } else {
            let mut bounds = (DVec3::MAX, DVec3::MIN);
            for x in [mins.x, maxs.x] {
                for y in [mins.y, maxs.y] {
                    for z in [mins.z, maxs.z] {
                        let point = self.point(DVec3::new(x, y, z));
                        if !point.is_finite() {
                            return Err("initial pose produces nonfinite model bounds".into());
                        }
                        bounds = (bounds.0.min(point), bounds.1.max(point));
                    }
                }
            }
            bounds
        };
        if !bounds.0.is_finite()
            || !bounds.1.is_finite()
            || bounds.0.abs().max_element() > crate::map::MAX_COMPILE_COORD
            || bounds.1.abs().max_element() > crate::map::MAX_COMPILE_COORD
        {
            return Err(format!(
                "initial model pose bounds exceed the finite Source coordinate limit ±{}",
                crate::map::MAX_COMPILE_COORD,
            ));
        }
        Ok(bounds)
    }

    pub(crate) fn raw_and_stored_bounds(
        self,
        mins: DVec3,
        maxs: DVec3,
    ) -> Result<(DVec3, DVec3), String> {
        let raw = self.bounds(mins, maxs)?;
        let stored = self.bounds(
            crate::emit::stored_vertex(mins),
            crate::emit::stored_vertex(maxs),
        )?;
        Ok((raw.0.min(stored.0), raw.1.max(stored.1)))
    }
}

pub(crate) fn component(token: &str) -> Result<f64, String> {
    let value = token
        .parse::<f64>()
        .map_err(|_| "angle field is not numeric")?;
    let native = value as f32;
    if !value.is_finite() || !native.is_finite() {
        return Err("angle components must fit finite native floats".into());
    }
    let nonzero_mantissa = token
        .split(['e', 'E'])
        .next()
        .unwrap_or(token)
        .bytes()
        .any(|byte| matches!(byte, b'1'..=b'9'));
    if value == 0.0 && nonzero_mantissa {
        return Err("a nonzero angle underflows its source number".into());
    }
    if value != 0.0 && native == 0.0 {
        return Err("a nonzero angle underflows its native float".into());
    }
    if value.abs() > MAX_INITIAL_ANGLE {
        return Err("initial angle components must be within [-360,360] degrees".into());
    }
    Ok(f64::from(native))
}

fn pose_key(key: &str) -> Result<&str, String> {
    if key.len() > MAX_INITIAL_KEY_BYTES {
        return Err("initial brush-model pose keys require at most 2047 bytes".into());
    }
    let trimmed = key.trim_end_matches(' ');
    Ok(trimmed.split_once('#').map_or(trimmed, |(base, _)| base))
}

pub(crate) fn initial_offset(properties: &[(String, String)]) -> Result<DVec3, String> {
    let mut field = None;
    for (key, value) in properties {
        let base = pose_key(key)?;
        if !base.eq_ignore_ascii_case("origin") {
            continue;
        }
        if base.len() != key.len() || field.is_some() {
            return Err(
                "nonzero initial lighting angles require one unaliased unambiguous origin field"
                    .into(),
            );
        }
        field = Some(value);
    }
    let Some(raw) = field else {
        return Ok(DVec3::ZERO);
    };
    if raw.len() > MAX_NATIVE_POSE_BYTES || !raw.is_ascii() {
        return Err("rotated origin fields require at most 127 ASCII payload bytes".into());
    }
    let mut numbers = raw.split_ascii_whitespace();
    let mut offset = DVec3::ZERO;
    for axis in 0..3 {
        let value = numbers
            .next()
            .ok_or("origin field requires three numeric components")?
            .parse::<f64>()
            .map_err(|_| "origin field is not numeric")?;
        let native = value as f32;
        if !value.is_finite() || !native.is_finite() {
            return Err("origin components must fit finite native floats".into());
        }
        offset[axis] = f64::from(native);
    }
    if numbers.next().is_some() {
        return Err("origin field requires three numeric components".into());
    }
    Ok(offset)
}

pub(crate) fn initial_rotation(
    classname: &str,
    properties: &[(String, String)],
) -> Result<Option<DMat3>, String> {
    let mut field = None;
    let mut parent_seen = false;
    let mut parented = false;
    let mut parent_ambiguous = false;
    let mut class_seen = false;
    let mut class_ambiguous = false;
    for (key, value) in properties {
        let base = pose_key(key)?;
        if base.eq_ignore_ascii_case("parentname") {
            parent_ambiguous |= parent_seen || base.len() != key.len();
            parent_seen = true;
            parented |= !value.is_empty();
        }
        if base.eq_ignore_ascii_case("classname") {
            class_ambiguous |=
                class_seen || key != "classname" || !value.eq_ignore_ascii_case(classname);
            class_seen = true;
        }
        let count = if base.eq_ignore_ascii_case("angles") {
            3
        } else if base.eq_ignore_ascii_case("angle") {
            1
        } else {
            continue;
        };
        if base.len() != key.len() {
            return Err("suffixed or space-padded angle keys are unsupported".into());
        }
        if field.is_some() {
            return Err("duplicate or coexisting angle/angles fields are unsupported".into());
        }
        field = Some((count, value));
    }
    let Some((count, raw)) = field else {
        return Ok(None);
    };
    if raw.len() > MAX_INITIAL_POSE_BYTES || !raw.is_ascii() {
        return Err("angle fields require at most 128 ASCII bytes".into());
    }
    let mut values = [0.0; 3];
    let mut numbers = raw.split_ascii_whitespace();
    for value in &mut values[..count] {
        *value = component(
            numbers
                .next()
                .ok_or("angle field has the wrong component count")?,
        )?;
    }
    if numbers.next().is_some() {
        return Err("angle field has the wrong component count".into());
    }
    if values == [0.0; 3] {
        return Ok(None);
    }
    if raw.len() > MAX_NATIVE_POSE_BYTES {
        return Err("nonzero angle fields require at most 127 ASCII payload bytes".into());
    }
    if count != 3 {
        return Err(
            "nonzero legacy scalar angle is unsupported for initial brush-model lighting".into(),
        );
    }
    if !classname.eq_ignore_ascii_case("func_brush") || !class_seen || class_ambiguous {
        return Err(
            "nonzero initial lighting angles require one unambiguous func_brush class".into(),
        );
    }
    if parented || parent_ambiguous {
        return Err("nonzero initial lighting angles require an unparented model with an unambiguous parentname".into());
    }
    let _ = initial_offset(properties)?;
    let [pitch, yaw, roll] = values.map(f64::to_radians);
    let (sp, cp) = pitch.sin_cos();
    let (sy, cy) = yaw.sin_cos();
    let (sr, cr) = roll.sin_cos();
    Ok(Some(DMat3::from_cols(
        DVec3::new(cp * cy, cp * sy, -sp),
        DVec3::new(sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, sr * cp),
        DVec3::new(cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp),
    )))
}
