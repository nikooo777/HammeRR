use std::collections::HashMap;

use glam::{DMat3, DVec3};

use crate::brush_model_pose::{self, Pose};
use crate::map::{Map, MapEntity};
use crate::{Control, Diagnostic, Report};

const FADE_FIELDS: [(&str, f32); 3] = [
    ("FadeStartDist", 128.0),
    ("FadeDist", 512.0),
    ("TranslucencyLimit", 0.2),
];
const BOUNDS_TOLERANCE: f64 = 0.02;
const MAX_WORK: usize = 4_000_000;
const ASCII_CASE_OFFSET: u8 = b'a' - b'A';
// Across Source coordinates, matrix roundoff contributes less than 1e-7 units.
const ROTATION_TOLERANCE: f64 = 1e-12;

fn value<'a>(properties: &'a [(String, String)], name: &str) -> Option<&'a str> {
    properties
        .iter()
        .rev()
        .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value.as_str()))
}

fn consume(work: &mut usize, amount: usize, control: &Control) -> Result<(), String> {
    control.checkpoint()?;
    *work = work
        .checked_add(amount)
        .filter(|&work| work <= MAX_WORK)
        .ok_or("Window portal validation exceeds its four-million work-unit budget")?;
    Ok(())
}

fn literal_name_matches(candidate: &str, query: &str) -> bool {
    // Native literal matching omits lower bounds on both ASCII offset branches.
    candidate.len() == query.len()
        && candidate
            .bytes()
            .zip(query.bytes())
            .all(|(candidate, query)| {
                candidate == query
                    || (candidate <= b'Z' && candidate + ASCII_CASE_OFFSET == query)
                    || (candidate <= b'z'
                        && candidate.checked_sub(ASCII_CASE_OFFSET) == Some(query))
            })
}

fn consume_pose_properties(
    entity: &MapEntity,
    work: &mut usize,
    control: &Control,
) -> Result<(), String> {
    for (key, value) in &entity.properties {
        consume(
            work,
            key.len()
                .saturating_add(value.len().min(brush_model_pose::MAX_INITIAL_POSE_BYTES))
                .saturating_add(1),
            control,
        )?;
    }
    Ok(())
}

fn initial_target_pose(
    entity: &MapEntity,
    work: &mut usize,
    control: &Control,
) -> Result<Pose, String> {
    consume_pose_properties(entity, work, control)?;
    brush_model_pose::require_unparented(&entity.properties)?;
    let rotation = brush_model_pose::initial_rotation(&entity.classname, &entity.properties)?;
    Ok(Pose {
        offset: if rotation.is_some() {
            brush_model_pose::initial_offset(&entity.properties)?
        } else {
            entity.origin.unwrap_or(DVec3::ZERO)
        },
        rotation,
    })
}

fn matching_rotation(first: Option<DMat3>, second: Option<DMat3>) -> bool {
    first
        .unwrap_or(DMat3::IDENTITY)
        .to_cols_array()
        .into_iter()
        .zip(second.unwrap_or(DMat3::IDENTITY).to_cols_array())
        .all(|(first, second)| (first - second).abs() <= ROTATION_TOLERANCE)
}

pub(crate) fn normalize(properties: &mut Vec<(String, String)>, id: u32, report: &mut Report) {
    let mut numbers = [0.0; 3];
    for (index, (field, default)) in FADE_FIELDS.iter().enumerate() {
        let parsed = value(properties, field).map_or(Ok(*default), str::parse::<f32>);
        let Some(number) = parsed.ok().filter(|number| number.is_finite()) else {
            report.diagnostics.push(Diagnostic::error(
                format!("Window portal {field} must be a finite number"),
                Some(format!("entity:{id}")),
                None,
            ));
            return;
        };
        numbers[index] = number;
    }
    if numbers[0] < 0.0 || numbers[1] < numbers[0] || !(0.0..=1.0).contains(&numbers[2]) {
        report.diagnostics.push(Diagnostic::error("Window portal distances must satisfy 0 ≤ Fade Start Distance ≤ Fade End Distance, and Translucency limit must be between 0 and 1", Some(format!("entity:{id}")), None));
        return;
    }
    properties.retain(|(key, _)| {
        !FADE_FIELDS
            .iter()
            .any(|(field, _)| key.eq_ignore_ascii_case(field))
    });
    for ((field, _), number) in FADE_FIELDS.iter().zip(numbers) {
        properties.push(((*field).into(), number.to_string()));
    }
}

pub(crate) fn validate(
    map: &Map<'_>,
    report: &mut Report,
    control: &Control,
) -> Result<(), String> {
    if !map
        .area_portals
        .iter()
        .any(|portal| map.entities[portal.entity].classname == "func_areaportalwindow")
    {
        return Ok(());
    }
    let mut names: HashMap<usize, Vec<(&str, usize)>> = HashMap::new();
    let mut work = 0;
    for (index, entity) in map.entities.iter().enumerate().skip(1) {
        consume(
            &mut work,
            entity.properties.len().saturating_add(1),
            control,
        )?;
        if let Some(name) = value(&entity.properties, "targetname").filter(|name| !name.is_empty())
        {
            names.entry(name.len()).or_default().push((name, index));
        }
    }
    let mut poses = HashMap::new();
    for portal in &map.area_portals {
        control.checkpoint()?;
        let entity = &map.entities[portal.entity];
        if entity.classname != "func_areaportalwindow" {
            continue;
        }
        let mut targets = [None; 2];
        let mut target_poses = [None; 2];
        let mut unsupported_pose = false;
        let controller_rotation = match consume_pose_properties(entity, &mut work, control)
            .and_then(|()| brush_model_pose::require_unparented(&entity.properties))
            .and_then(|()| brush_model_pose::initial_window_rotation(&entity.properties))
        {
            Ok(rotation) => Some(rotation),
            Err(error) => {
                control.checkpoint()?;
                report.diagnostics.push(Diagnostic::error(
                    format!("Unsupported initial pose for window portal entity:{} controller angle/angles fields: {error}", entity.id),
                    Some(format!("entity:{}", entity.id)),
                    entity.origin,
                ));
                unsupported_pose = true;
                None
            }
        };
        for (slot, (field, label)) in [
            ("target", "Rendered Window"),
            ("BackgroundBModel", "Foreground bmodel"),
        ]
        .into_iter()
        .enumerate()
        {
            let name = value(&entity.properties, field).unwrap_or("");
            if slot == 1 && name.is_empty() {
                continue;
            }
            if let Err(error) = consume(&mut work, name.len().saturating_add(1), control) {
                control.checkpoint()?;
                report.diagnostics.push(Diagnostic::error(
                    error,
                    Some(format!("entity:{}", entity.id)),
                    entity.origin,
                ));
                return Ok(());
            }
            if name.starts_with('!') || name.contains('*') {
                report.diagnostics.push(Diagnostic::error(
                    format!("Window portal {label} requires a literal brush-model name; wildcard and procedural references are unsupported"),
                    Some(format!("entity:{}", entity.id)),
                    entity.origin,
                ));
                continue;
            }
            let candidates = names
                .get(&name.len())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let mut matching = None;
            for &(candidate, index) in candidates {
                if let Err(error) = consume(&mut work, candidate.len().saturating_add(1), control) {
                    control.checkpoint()?;
                    report.diagnostics.push(Diagnostic::error(
                        error,
                        Some(format!("entity:{}", entity.id)),
                        entity.origin,
                    ));
                    return Ok(());
                }
                if literal_name_matches(candidate, name) {
                    if matching.is_some() {
                        matching = None;
                        break;
                    }
                    matching = Some(index);
                }
            }
            let index = matching.filter(|&index| {
                !map.entities[index].brushes.is_empty()
                    && map.entities[index].brushes.iter().any(|&brush| {
                        map.brushes[brush]
                            .sides
                            .iter()
                            .any(|side| side.visible && !side.bevel)
                    })
            });
            if let Some(index) = index {
                targets[slot] = Some(index);
                let target = &map.entities[index];
                let pose = poses
                    .entry(index)
                    .or_insert_with(|| initial_target_pose(target, &mut work, control));
                match pose {
                    Ok(pose) => {
                        target_poses[slot] = Some(*pose);
                        if controller_rotation
                            .is_some_and(|rotation| !matching_rotation(rotation, pose.rotation))
                        {
                            report.diagnostics.push(Diagnostic::error(
                                format!("Window portal {label} ({field}) target entity:{} must have the same initial rotation as its controller entity:{}", target.id, entity.id),
                                Some(format!("entity:{}", target.id)),
                                target.origin,
                            ));
                            unsupported_pose = true;
                        }
                    }
                    Err(error) => {
                        control.checkpoint()?;
                        report.diagnostics.push(Diagnostic::error(
                            format!("Unsupported initial pose for window portal entity:{} {label} ({field}) target entity:{} angle/angles fields: {error}", entity.id, target.id),
                            Some(format!("entity:{}", target.id)),
                            target.origin,
                        ));
                        unsupported_pose = true;
                    }
                }
            } else {
                report.diagnostics.push(Diagnostic::error(format!("Window portal {label} must name exactly one visible rendered brush model; {name:?} does not"), Some(format!("entity:{}", entity.id)), None));
            }
        }
        if unsupported_pose {
            continue;
        }
        if let (Some(target), Some(target_pose), Some(rotation)) =
            (targets[0], target_poses[0], controller_rotation)
        {
            consume(
                &mut work,
                map.entities[target].brushes.len().saturating_add(16),
                control,
            )?;
            let (low, high) = map.model_bounds(target);
            let origin = map.entities[target].origin.unwrap_or(DVec3::ZERO);
            let rotated = rotation.is_some() || target_pose.rotation.is_some();
            let draw_pose = Pose {
                offset: target_pose.offset.as_vec3().as_dvec3(),
                rotation,
            };
            for &brush in &portal.brushes {
                consume(&mut work, 1, control)?;
                let brush = &map.brushes[brush];
                let enclosure = if rotated {
                    draw_pose
                    .raw_and_stored_bounds(
                        low.min(low.as_vec3().as_dvec3()),
                        high.max(high.as_vec3().as_dvec3()),
                    )
                    .and_then(|_| {
                        let local_low = low.max(low.as_vec3().as_dvec3())
                            - DVec3::splat(BOUNDS_TOLERANCE);
                        let local_high = high.min(high.as_vec3().as_dvec3())
                            + DVec3::splat(BOUNDS_TOLERANCE);
                        for point in brush.points() {
                            consume(&mut work, 2, control)?;
                            for point in [point, crate::emit::stored_vertex(point)] {
                                let local = draw_pose.local_point(point);
                                if !crate::map::within_compile_bounds(local)
                                    || !local.cmple(local_high).all()
                                    || !local.cmpge(local_low).all()
                                {
                                    return Err("Rendered Window local bounds must enclose every source and native portal vertex in the matching initial pose".into());
                                }
                            }
                        }
                        Ok(())
                    })
                } else if !(low + origin - DVec3::splat(BOUNDS_TOLERANCE))
                    .cmple(brush.mins)
                    .all()
                    || !(high + origin + DVec3::splat(BOUNDS_TOLERANCE))
                        .cmpge(brush.maxs)
                        .all()
                {
                    Err("Rendered Window bounds must enclose the portal volume".into())
                } else {
                    Ok(())
                };
                if let Err(error) = enclosure {
                    control.checkpoint()?;
                    report.diagnostics.push(Diagnostic::error(
                        format!("Window portal {error}"),
                        Some(format!("entity:{}", entity.id)),
                        None,
                    ));
                }
            }
            if let Some(background) = targets[1] {
                consume(
                    &mut work,
                    map.entities[background].brushes.len().saturating_add(16),
                    control,
                )?;
                let (back_low, back_high) = map.model_bounds(background);
                if map.entities[background].origin.unwrap_or(DVec3::ZERO) != origin
                    || !back_low.cmple(low).all()
                    || !back_high.cmpge(high).all()
                {
                    report.diagnostics.push(Diagnostic::error("Window portal Foreground bmodel must share the Rendered Window pivot and enclose its bounds", Some(format!("entity:{}", entity.id)), None));
                }
                if rotated
                    && let Err(error) = draw_pose.raw_and_stored_bounds(
                        back_low.min(back_low.as_vec3().as_dvec3()),
                        back_high.max(back_high.as_vec3().as_dvec3()),
                    )
                {
                    report.diagnostics.push(Diagnostic::error(
                        format!("Window portal Foreground bmodel initial pose: {error}"),
                        Some(format!("entity:{}", map.entities[background].id)),
                        map.entities[background].origin,
                    ));
                }
            }
        }
    }
    Ok(())
}
