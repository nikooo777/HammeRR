use std::collections::HashMap;

use glam::DVec3;

use crate::map::Map;
use crate::{Control, Diagnostic, Report};

const FADE_FIELDS: [(&str, f32); 3] = [
    ("FadeStartDist", 128.0),
    ("FadeDist", 512.0),
    ("TranslucencyLimit", 0.2),
];
const BOUNDS_TOLERANCE: f64 = 0.02;

fn value<'a>(properties: &'a [(String, String)], name: &str) -> Option<&'a str> {
    properties
        .iter()
        .rev()
        .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value.as_str()))
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
    let mut names: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, entity) in map.entities.iter().enumerate().skip(1) {
        control.checkpoint()?;
        if let Some(name) = value(&entity.properties, "targetname").filter(|name| !name.is_empty())
        {
            names.entry(name).or_default().push(index);
        }
    }
    for portal in &map.area_portals {
        control.checkpoint()?;
        let entity = &map.entities[portal.entity];
        if entity.classname != "func_areaportalwindow" {
            continue;
        }
        let mut targets = [None; 2];
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
            let matching = names.get(name).map(Vec::as_slice).unwrap_or_default();
            let index = match matching {
                &[index]
                    if !map.entities[index].brushes.is_empty()
                        && map.entities[index].brushes.iter().any(|&brush| {
                            map.brushes[brush]
                                .sides
                                .iter()
                                .any(|side| side.visible && !side.bevel)
                        }) =>
                {
                    Some(index)
                }
                _ => None,
            };
            if let Some(index) = index {
                targets[slot] = Some(index);
            } else {
                report.diagnostics.push(Diagnostic::error(format!("Window portal {label} must name exactly one visible rendered brush model; {name:?} does not"), Some(format!("entity:{}", entity.id)), None));
            }
        }
        if let Some(target) = targets[0] {
            let (low, high) = map.model_bounds(target);
            let origin = map.entities[target].origin.unwrap_or(DVec3::ZERO);
            let brush = &map.brushes[portal.brush];
            if !(low + origin - DVec3::splat(BOUNDS_TOLERANCE))
                .cmple(brush.mins)
                .all()
                || !(high + origin + DVec3::splat(BOUNDS_TOLERANCE))
                    .cmpge(brush.maxs)
                    .all()
            {
                report.diagnostics.push(Diagnostic::error(
                    "Window portal Rendered Window bounds must enclose the portal volume",
                    Some(format!("entity:{}", entity.id)),
                    None,
                ));
            }
            if let Some(background) = targets[1] {
                let (back_low, back_high) = map.model_bounds(background);
                if map.entities[background].origin.unwrap_or(DVec3::ZERO) != origin
                    || !back_low.cmple(low).all()
                    || !back_high.cmpge(high).all()
                {
                    report.diagnostics.push(Diagnostic::error("Window portal Foreground bmodel must share the Rendered Window pivot and enclose its bounds", Some(format!("entity:{}", entity.id)), None));
                }
            }
        }
    }
    Ok(())
}
