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

#[cfg(test)]
mod tests {
    use super::*;

    fn properties(entries: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut properties = vec![("classname".into(), "func_brush".into())];
        properties.extend(
            entries
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into())),
        );
        properties
    }

    fn rotation(angles: &str) -> DMat3 {
        initial_rotation("func_brush", &properties(&[("angles", angles)]))
            .unwrap()
            .unwrap()
    }

    fn assert_vector(actual: DVec3, expected: DVec3) {
        assert!(
            actual.distance(expected) < 2e-12,
            "{actual:?} != {expected:?}"
        );
    }

    fn mixed_matrix() -> DMat3 {
        let half_root = std::f64::consts::FRAC_1_SQRT_2;
        DMat3::from_cols(
            DVec3::new(0.5, 0.5, -half_root),
            DVec3::new(half_root / 2.0 - 0.5, half_root / 2.0 + 0.5, 0.5),
            DVec3::new(half_root / 2.0 + 0.5, half_root / 2.0 - 0.5, 0.5),
        )
    }

    fn compiled_models(entries: &[(&str, &str)]) -> Bsp {
        let mut bsp = Bsp::new();
        bsp.set_records(&[
            Model::default(),
            Model {
                mins: [-1.0, -2.0, -3.0],
                maxs: [4.0, 5.0, 6.0],
                origin: [999.0, 998.0, 997.0],
                ..Model::default()
            },
        ]);
        let mut text = "{\n\"classname\" \"worldspawn\"\n}\n{\n".to_owned();
        for (key, value) in properties(entries) {
            text.push_str(&format!("\"{key}\" \"{value}\"\n"));
        }
        text.push_str("}\n");
        bsp.set_entities(&text);
        bsp
    }

    #[test]
    fn source_axes_and_mixed_rotation_match_independent_literal_matrices() {
        let cases = [
            ("0 90 0", DMat3::from_cols(DVec3::Y, -DVec3::X, DVec3::Z)),
            ("90 0 0", DMat3::from_cols(-DVec3::Z, DVec3::Y, DVec3::X)),
            ("-90 0 0", DMat3::from_cols(DVec3::Z, DVec3::Y, -DVec3::X)),
            ("0 0 90", DMat3::from_cols(DVec3::X, DVec3::Z, -DVec3::Y)),
            ("45 45 45", mixed_matrix()),
        ];
        let point = DVec3::new(2.25, -3.5, 7.75);
        let normal = DVec3::new(2.0, -1.0, 3.0).normalize();
        let offset = DVec3::new(101.5, -67.25, 32.125);
        for (angles, expected) in cases {
            let actual = rotation(angles);
            for (actual, expected) in actual
                .to_cols_array()
                .into_iter()
                .zip(expected.to_cols_array())
            {
                assert!(
                    (actual - expected).abs() < 2e-15,
                    "{angles}: {actual} != {expected}"
                );
            }
            let pose = Pose {
                offset,
                rotation: Some(actual),
            };
            assert_vector(pose.point(point), expected * point + offset);
            assert_vector(pose.local_point(expected * point + offset), point);
            assert_vector(pose.normal(normal), expected * normal);
            assert_vector(
                pose.point(point) + pose.normal(normal) * 0.25,
                expected * (point + normal * 0.25) + offset,
            );
            assert!((pose.normal(normal).length() - 1.0).abs() < 2e-15);
            let bsp = compiled_models(&[
                ("model", "*1"),
                ("origin", "101.5 -67.25 32.125"),
                ("angles", angles),
            ]);
            let transforms = initial_brush_model_transforms(&bsp).unwrap();
            assert_eq!(transforms[0], DMat4::IDENTITY);
            assert_vector(
                transforms[1].transform_point3(point),
                expected * point + offset,
            );
            assert_vector(transforms[1].transform_vector3(normal), expected * normal);
        }
    }

    #[test]
    fn inverse_translation_retains_f64_subtraction_without_native_projection_rounding() {
        let pose = Pose {
            offset: DVec3::new(101.500001, -67.250001, 32.125001),
            rotation: None,
        };
        let world = DVec3::new(102.0, -66.0, 33.0);
        let local = pose.local_point(world);
        assert_eq!(
            local.to_array().map(f64::to_bits),
            (world - pose.offset).to_array().map(f64::to_bits)
        );
        assert_ne!(local, (world.as_vec3() - pose.offset.as_vec3()).as_dvec3());
    }

    #[test]
    fn absent_and_exact_zero_preserve_original_point_and_normal_bits() {
        for (classname, entries) in [
            ("func_brush", vec![]),
            ("func_brush", vec![("AnGlEs", "\t-0 +0.0 0e-300\n")]),
            ("func_brush", vec![("angle", "-0.000")]),
            (
                "func_door",
                vec![("angles", "0 0 0"), ("parentname", "some_parent")],
            ),
            (
                "func_wall",
                vec![("angle", "0"), ("classname#1", "other_class")],
            ),
        ] {
            let rotation = initial_rotation(classname, &properties(&entries)).unwrap();
            assert!(rotation.is_none());
            let offset = DVec3::new(-0.0, 11.5, -0.0);
            let local = DVec3::new(-0.0, -7.25, 5.5);
            let pose = Pose { offset, rotation };
            assert_eq!(
                pose.point(local).to_array().map(f64::to_bits),
                (local + offset).to_array().map(f64::to_bits)
            );
            assert_eq!(
                pose.normal(local).to_array().map(f64::to_bits),
                local.to_array().map(f64::to_bits)
            );
        }
    }

    #[test]
    fn native_float_angles_are_used_without_modulo_or_source_range_rounding() {
        assert_eq!(rotation("20.1250001 35 -10"), rotation("20.125 35 -10"));
        for angles in ["-360 0 0", "0 360 0", "0 0 -360", "1e-40 0 0"] {
            assert!(
                initial_rotation("func_brush", &properties(&[("angles", angles)]))
                    .unwrap()
                    .is_some()
            );
        }
        for angles in ["360.00000001 0 0", "0 -360.00000001 0"] {
            assert!(
                initial_rotation("func_brush", &properties(&[("angles", angles)]))
                    .unwrap_err()
                    .contains("[-360,360]")
            );
        }
        let mut fields = properties(&[("ANGLES", "1 2 3"), ("ParentName", "")]);
        fields[0].1 = "FUNC_BRUSH".into();
        assert!(initial_rotation("FUNC_BRUSH", &fields).unwrap().is_some());
        for angles in [None, Some("0 0 0")] {
            let mut entries = vec![
                ("model", "*1"),
                ("origin", "101.500001 -67.250001 32.125001"),
            ];
            if let Some(angles) = angles {
                entries.push(("angles", angles));
            }
            let transforms = initial_brush_model_transforms(&compiled_models(&entries)).unwrap();
            let local = DVec3::new(2.5, -7.5, 1.25);
            assert_vector(
                transforms[1].transform_point3(local),
                local + DVec3::new(101.5, -67.25, 32.125),
            );
            assert_vector(transforms[1].transform_vector3(local), local);
        }
    }

    #[test]
    fn malformed_ambiguous_nonfinite_and_underflowed_fields_are_not_zero_fallbacks() {
        let cases: &[&[(&str, &str)]] = &[
            &[("angles", "")],
            &[("angles", "0 0")],
            &[("angles", "0 0 0 trailing")],
            &[("angles", "[0 0 0]")],
            &[("angles", "0 0 ０")],
            &[("angles", "NaN 0 0")],
            &[("angles", "0 inf 0")],
            &[("angles", "0 0 1e400")],
            &[("angles", "3.5e38 0 0")],
            &[("angles", "1e-4000 0 0")],
            &[("angles", "1e-50 0 0")],
            &[("angle", "0 0")],
            &[("angle", "-1")],
            &[("angle", "-2")],
            &[("angles", "0 0 0"), ("angles", "0 0 0")],
            &[("AnGlEs", "0 0 0"), ("ANGLES", "0 90 0")],
            &[("angles", "0 0 0"), ("angle", "0")],
            &[("angle", "0"), ("ANGLE", "0")],
            &[("angles#1", "0 0 0")],
            &[("ANGLE#later", "90")],
            &[("angles ", "0 90 0")],
            &[("angles ", "0 0 0")],
            &[("angle ", "0")],
            &[("angles", "0 0 0"), ("angles ", "0 90 0")],
            &[("angles#later ", "0 0 0")],
        ];
        for entries in cases {
            assert!(
                initial_rotation("func_brush", &properties(entries)).is_err(),
                "{entries:?}"
            );
        }
        let boundary = " ".repeat(MAX_INITIAL_POSE_BYTES - 5) + "0 0 0";
        assert!(
            initial_rotation("func_brush", &properties(&[("angles", &boundary)]))
                .unwrap()
                .is_none()
        );
        let oversized = " ".repeat(MAX_INITIAL_POSE_BYTES) + "0 0 0";
        assert!(
            initial_rotation("func_brush", &properties(&[("angles", &oversized)]))
                .unwrap_err()
                .contains("128 ASCII")
        );
        let source =
            initial_rotation("func_brush", &properties(&[("angles", "1e-4000 0 0")])).unwrap_err();
        let native =
            initial_rotation("func_brush", &properties(&[("angles", "1e-50 0 0")])).unwrap_err();
        assert!(source.contains("source number") && native.contains("native float"));
        assert!(
            initial_rotation(
                "func_brush",
                &properties(&[("unrelated_angles", "45 45 45")])
            )
            .unwrap()
            .is_none()
        );
        for entries in [
            vec![],
            vec![("model", "*0")],
            vec![("model", "*2")],
            vec![("model", "*x")],
            vec![("model", "*-1")],
            vec![("model", "*1 trailing")],
            vec![("model#1", "*1")],
            vec![("model ", "*1")],
            vec![("model", "*1"), ("MODEL", "*1")],
            vec![("model", "*1"), ("origin", "NaN 0 0")],
            vec![("model", "*1"), ("angles", "0 0")],
            vec![("model", "*1"), ("classname", "func_brush")],
        ] {
            assert!(
                initial_brush_model_transforms(&compiled_models(&entries)).is_err(),
                "{entries:?}"
            );
        }
        let mut duplicate = compiled_models(&[("model", "*1")]);
        let text = duplicate.entities().unwrap().to_owned()
            + "{\n\"classname\" \"func_brush\"\n\"model\" \"*1\"\n}\n";
        duplicate.set_entities(&text);
        assert!(initial_brush_model_transforms(&duplicate).is_err());
        let mut cancelled = || {
            Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ))
        };
        assert_eq!(
            initial_brush_model_transforms_controlled(&duplicate, &mut cancelled).unwrap_err(),
            "cancelled"
        );
    }

    #[test]
    fn other_classes_and_hidden_parent_class_or_origin_aliases_cannot_admit_nonzero_poses() {
        for classname in [
            "func_wall",
            "func_door",
            "func_rotating",
            "trigger_multiple",
            "",
        ] {
            assert!(initial_rotation(classname, &properties(&[("angles", "0 90 0")])).is_err());
        }
        let cases: &[&[(&str, &str)]] = &[
            &[("parentname", "parent")],
            &[("PaReNtNaMe", " ")],
            &[("parentname#1", "")],
            &[("parentname", ""), ("PARENTNAME", "")],
            &[("parentname", ""), ("parentname", "parent")],
            &[("parentname ", "")],
            &[("parentname", ""), ("PaReNtNaMe ", "parent")],
            &[("classname", "func_wall")],
            &[("classname#1", "func_brush")],
            &[("classname", "func_brush"), ("ClassName", "func_brush")],
            &[("classname ", "func_brush")],
            &[("classname", "func_brush"), ("ClassName ", "func_wall")],
            &[("origin", "1 2 3"), ("origin", "1 2 3")],
            &[("origin", "1 2 3"), ("ORIGIN", "4 5 6")],
            &[("origin#1", "1 2 3")],
            &[("OrIgIn#later", "")],
            &[("origin ", "1 2 3")],
            &[("origin", "1 2 3"), ("OrIgIn ", "4 5 6")],
            &[("origin", "invalid")],
            &[("origin", "(1 2 3)")],
            &[("origin", "1 2")],
            &[("origin", "1 2 3 trailing")],
            &[("origin", "NaN 0 0")],
            &[("origin", "1e400 0 0")],
        ];
        for entries in cases {
            let mut fields = properties(entries);
            fields.push(("angles".into(), "0 90 0".into()));
            assert!(
                initial_rotation("func_brush", &fields).is_err(),
                "{entries:?}"
            );
            fields.last_mut().unwrap().1 = "0 0 0".into();
            assert!(initial_rotation("func_brush", &fields).unwrap().is_none());
        }
    }

    #[test]
    fn native_key_recognition_trims_only_trailing_ascii_spaces_before_the_suffix() {
        for key in [
            "angles #later",
            "angles #later ",
            "angles\t",
            "angles\u{a0}",
        ] {
            assert!(
                initial_rotation("func_brush", &properties(&[(key, "0 90 0")]))
                    .unwrap()
                    .is_none(),
                "{key:?}"
            );
        }
        for key in ["angles ", "angles#later ", "ANGLE "] {
            assert!(
                initial_rotation("func_brush", &properties(&[(key, "0 0 0")])).is_err(),
                "{key:?}"
            );
        }
    }

    #[test]
    fn overlong_keys_cannot_hide_native_truncated_angles_with_absent_or_zero_fields() {
        let long = format!("angles{}x", " ".repeat(MAX_INITIAL_KEY_BYTES - 6));
        assert_eq!(long.len(), 2048);
        let fields = vec![(long.clone(), "0 90 0".into())];
        assert!(
            initial_rotation("func_brush", &fields)
                .unwrap_err()
                .contains("2047 bytes")
        );
        assert!(initial_offset(&fields).unwrap_err().contains("2047 bytes"));
        let after_zero = vec![("angles".into(), "0 0 0".into()), (long, "0 90 0".into())];
        assert!(
            initial_rotation("func_brush", &after_zero)
                .unwrap_err()
                .contains("2047 bytes")
        );
        let boundary = format!("angles{}x", " ".repeat(MAX_INITIAL_KEY_BYTES - 7));
        assert_eq!(boundary.len(), 2047);
        let admitted = vec![(boundary, "0 90 0".into())];
        assert!(initial_rotation("func_brush", &admitted).unwrap().is_none());
        assert_eq!(initial_offset(&admitted).unwrap(), DVec3::ZERO);
    }

    #[test]
    fn rotated_class_factory_requires_one_canonical_classname_key() {
        for fields in [
            vec![("angles".into(), "0 90 0".into())],
            vec![
                ("ClassName".into(), "func_brush".into()),
                ("angles".into(), "0 90 0".into()),
            ],
        ] {
            assert!(initial_rotation("func_brush", &fields).is_err());
        }
        assert!(
            initial_rotation("func_brush", &properties(&[("angles", "0 90 0")]))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn rotated_translation_uses_the_unique_emitted_origin_native_components() {
        assert_eq!(initial_offset(&[]).unwrap(), DVec3::ZERO);
        let raw = "101.500001 -67.250001 32.125001";
        let offset = initial_offset(&properties(&[("OrIgIn", raw)])).unwrap();
        assert_eq!(offset, DVec3::new(101.5, -67.25, 32.125));
        assert_ne!(offset, source_formats::vmf::parse_vec3(raw).unwrap());
        let different_text =
            initial_offset(&properties(&[("origin", "101.75 -67.5 32.25")])).unwrap();
        assert_eq!(different_text, DVec3::new(101.75, -67.5, 32.25));
        assert!(
            initial_rotation(
                "func_brush",
                &properties(&[("angles", "0 90 0"), ("origin", raw)])
            )
            .unwrap()
            .is_some()
        );
    }

    #[test]
    fn rotated_fields_must_fit_the_native_128byte_buffer_without_truncation() {
        let fits = " ".repeat(MAX_NATIVE_POSE_BYTES - 6) + "0 0 45";
        let truncates = " ".to_owned() + &fits;
        assert_eq!(fits.len(), 127);
        assert_eq!(truncates.len(), 128);
        assert!(
            initial_rotation("func_brush", &properties(&[("angles", &fits)]))
                .unwrap()
                .is_some()
        );
        assert!(
            initial_rotation("func_brush", &properties(&[("angles", &truncates)]))
                .unwrap_err()
                .contains("127 ASCII")
        );
        assert_eq!(
            initial_offset(&properties(&[("origin", &fits)])).unwrap(),
            DVec3::new(0.0, 0.0, 45.0)
        );
        assert!(
            initial_offset(&properties(&[("origin", &truncates)]))
                .unwrap_err()
                .contains("127 ASCII")
        );
        assert!(
            initial_rotation(
                "func_brush",
                &properties(&[("angles", "0 0 0"), ("origin", &truncates)])
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn bounds_use_all_eight_corners_and_preserve_unrotated_arithmetic() {
        let mins = DVec3::new(-1.0, -2.0, -3.0);
        let maxs = DVec3::new(4.0, 5.0, 6.0);
        let offset = DVec3::new(101.5, -67.25, 32.125);
        let pose = Pose {
            offset,
            rotation: Some(mixed_matrix()),
        };
        let (low, high) = pose.bounds(mins, maxs).unwrap();
        assert_vector(
            low,
            offset + DVec3::new(-3.7928932188134525, -3.085786437626905, -5.32842712474619),
        );
        assert_vector(
            high,
            offset + DVec3::new(7.414213562373095, 6.707106781186548, 6.207106781186548),
        );
        let identity = Pose {
            offset,
            rotation: None,
        };
        assert_eq!(
            identity.bounds(mins, maxs).unwrap(),
            (mins + offset, maxs + offset)
        );
    }

    #[test]
    fn bounds_reject_invalid_edges_and_stored_vertex_rounding_can_cross_the_limit() {
        let pose = Pose {
            offset: DVec3::ZERO,
            rotation: Some(DMat3::IDENTITY),
        };
        let edge = DVec3::splat(crate::map::MAX_COMPILE_COORD);
        assert!(pose.bounds(-edge, edge).is_ok());
        assert!(pose.bounds(-edge, edge + DVec3::X * 0.25).is_err());
        assert!(pose.bounds(DVec3::ONE, DVec3::ZERO).is_err());
        assert!(pose.bounds(DVec3::splat(f64::NAN), DVec3::ONE).is_err());
        assert!(
            pose.bounds(DVec3::ZERO, DVec3::splat(f64::INFINITY))
                .is_err()
        );
        for rotation in [None, Some(DMat3::IDENTITY)] {
            assert!(
                Pose {
                    offset: DVec3::splat(f64::NAN),
                    rotation
                }
                .bounds(DVec3::ZERO, DVec3::ONE)
                .is_err()
            );
        }
        let rounding = Pose {
            offset: DVec3::new(16382.99994, 0.0, 0.0),
            rotation: Some(DMat3::IDENTITY),
        };
        let local = DVec3::new(1.00006, 0.0, 0.0);
        assert!(rounding.bounds(local, local).is_ok());
        let stored = crate::emit::stored_vertex(local);
        assert!(stored.x > local.x);
        assert!(rounding.bounds(stored, stored).is_err());
    }
}
