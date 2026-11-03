use glam::DVec3;
use source_formats::{
    fgd::{Catalog, Class, Helper, Property},
    vmf::Entity,
};

use crate::Document;

use super::{MAX_HELPER_EXTENT, lines, values};

mod geometry;

pub const MAX_GUIDES_PER_ENTITY: usize = 32;
pub const MAX_SEGMENTS_PER_GUIDE: usize = 96;
pub const MAX_KEY_BYTES: usize = 128;
pub const MAX_VALUE_BYTES: usize = 256;
pub const MAX_ANCHOR_SOLIDS: usize = values::MAX_ANCHOR_SOLIDS;
pub const MAX_ANCHOR_SIDES: usize = values::MAX_ANCHOR_SIDES;
pub const MAX_ANCHOR_BRUSH_SIDES: usize = values::MAX_ANCHOR_BRUSH_SIDES;
pub const MAX_ANCHOR_WORK: usize = values::MAX_ANCHOR_WORK;
pub const MARKER_POLICY: &str = "World-axis wire crosses with 4-unit half-length; camera-independent editor guides, not native draggable sphere handles";

#[derive(Debug, Clone)]
pub struct Input {
    pub entity: u32,
    pub property: String,
    pub property_type: Option<String>,
    pub raw: Option<String>,
    pub raw_truncated: bool,
    pub value_source: &'static str,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Guide {
    pub kind: &'static str,
    pub declaration_index: Option<usize>,
    pub property: Option<String>,
    pub property_truncated: bool,
    pub arguments: Vec<String>,
    pub arguments_truncated: bool,
    pub inputs: Vec<Input>,
    pub endpoints: Vec<lines::Endpoint>,
    pub anchor: Option<[f64; 3]>,
    pub anchor_source: Option<&'static str>,
    pub radii: Option<[f64; 2]>,
    pub color: Option<[u8; 3]>,
    pub marker_policy: Option<&'static str>,
    pub segments: Vec<[[f64; 3]; 2]>,
    pub visual_bounds: Option<[[f64; 3]; 2]>,
    pub state: &'static str,
    pub error: Option<String>,
    pub omitted: usize,
}

impl Guide {
    fn new(kind: &'static str) -> Self {
        Self {
            kind,
            declaration_index: None,
            property: None,
            property_truncated: false,
            arguments: Vec::new(),
            arguments_truncated: false,
            inputs: Vec::new(),
            endpoints: Vec::new(),
            anchor: None,
            anchor_source: None,
            radii: None,
            color: None,
            marker_policy: None,
            segments: Vec::new(),
            visual_bounds: None,
            state: "invalid",
            error: None,
            omitted: 0,
        }
    }

    fn anchor(
        &mut self,
        class: &Class,
        entity: &Entity,
        targets: &lines::Targets<'_>,
    ) -> Result<DVec3, String> {
        self.anchor_source = Some(if entity.solids.is_empty() {
            "authored_origin"
        } else {
            "brush_bounds_center"
        });
        if entity.solids.is_empty() {
            let input = input(Some(class), entity, "origin", false);
            let error = input.error.clone();
            self.inputs.push(input);
            if let Some(error) = error {
                return Err(error);
            }
        }
        let (point, source) = targets.anchor(entity)?;
        self.anchor = Some(point.to_array());
        self.anchor_source = Some(source);
        Ok(point)
    }

    fn finish(&mut self, segments: Vec<[[f64; 3]; 2]>) -> Result<(), String> {
        if segments.is_empty() || segments.len() > MAX_SEGMENTS_PER_GUIDE {
            return Err("Static helper exceeds its segment bound or has no geometry".into());
        }
        let mut mins = DVec3::splat(f64::INFINITY);
        let mut maxs = DVec3::splat(f64::NEG_INFINITY);
        for segment in &segments {
            let points = segment.map(DVec3::from_array);
            if points.iter().any(|point| {
                !point.is_finite()
                    || !point.as_vec3().is_finite()
                    || point.abs().max_element() > MAX_HELPER_EXTENT
            }) || points[0].as_vec3() == points[1].as_vec3()
            {
                return Err(
                    "Static helper segments need finite distinct native points within ±65536"
                        .into(),
                );
            }
            for point in points {
                mins = mins.min(point);
                maxs = maxs.max(point);
            }
        }
        self.visual_bounds = Some([mins.to_array(), maxs.to_array()]);
        self.segments = segments;
        self.state = "ready";
        Ok(())
    }
}

fn input(class: Option<&Class>, entity: &Entity, key: &str, defaults: bool) -> Input {
    let value = values::Value::resolve_optional(class, entity, key, defaults);
    let error = if key.is_empty() || key.len() > MAX_KEY_BYTES {
        Some("Static helper property bindings require 1–128 bytes".into())
    } else {
        value
            .validate(MAX_VALUE_BYTES, None)
            .err()
            .map(|error| match error {
                values::Error::Duplicate => "Static helper property occurs more than once".into(),
                values::Error::Oversized => "Static helper value exceeds 256 bytes".into(),
                values::Error::Kind => "Static helper property has an unsupported type".into(),
                values::Error::Invalid(error) => values::bounded(&error, 512),
            })
    };
    Input {
        entity: entity.id,
        property: values::bounded(key, MAX_KEY_BYTES),
        property_type: value
            .property
            .map(|property| values::bounded(&property.kind, MAX_KEY_BYTES)),
        raw: value.raw.map(|raw| values::bounded(raw, MAX_VALUE_BYTES)),
        raw_truncated: value.raw.is_some_and(|raw| raw.len() > MAX_VALUE_BYTES),
        value_source: value.source,
        error,
    }
}

fn numbers(input: &Input, count: usize) -> Result<Vec<f64>, String> {
    if let Some(error) = &input.error {
        return Err(error.clone());
    }
    input
        .raw
        .as_deref()
        .and_then(|raw| values::numbers(raw, &[count]))
        .ok_or_else(|| {
            format!(
                "Static helper {} needs {count} finite native numbers",
                input.property
            )
        })
}

fn vector(input: &Input) -> Result<DVec3, String> {
    numbers(input, 3).map(|values| DVec3::from_slice(&values))
}

fn binding(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_KEY_BYTES
        && key.trim() == key
        && !key.chars().any(char::is_control)
}

pub(super) fn supported(class: &Class, helper: &Helper) -> bool {
    match helper.name.as_str() {
        "wirebox" => {
            helper.arguments.len() == 2
                && helper.arguments.iter().all(|key| {
                    binding(key)
                        && class
                            .property(key)
                            .is_some_and(|property| property.kind == "vector")
                })
        }
        "cylinder" => {
            helper.arguments.len() == 9
                && helper.arguments[..3]
                    .iter()
                    .all(|value| value.parse::<u8>().is_ok())
                && helper.arguments[3..]
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .all(|triple| {
                        triple[0].eq_ignore_ascii_case("targetname")
                            && binding(&triple[1])
                            && class.property(&triple[1]).is_some()
                            && binding(&triple[2])
                    })
        }
        _ => false,
    }
}

fn wirebox(
    guide: &mut Guide,
    class: &Class,
    entity: &Entity,
    helper: &Helper,
    targets: &lines::Targets<'_>,
) -> Result<(), String> {
    for key in &helper.arguments {
        guide.inputs.push(input(Some(class), entity, key, true));
    }
    let mins = vector(&guide.inputs[0])?;
    let maxs = vector(&guide.inputs[1])?;
    if !mins.cmplt(maxs).all() || mins.abs().max(maxs.abs()).max_element() > MAX_HELPER_EXTENT {
        return Err("Wirebox needs ordered positive-volume vectors within ±65536".into());
    }
    let anchor = guide.anchor(class, entity, targets)?;
    guide.color = Some([255, 255, 0]);
    guide.finish(geometry::wirebox(anchor + mins, anchor + maxs))
}

fn reference_input(class: &Class, entity: &Entity, endpoint: &lines::Endpoint) -> Input {
    let key = endpoint.property.as_deref().unwrap_or_default();
    let mut input = input(Some(class), entity, key, true);
    if endpoint.state != "ready" {
        input.error.clone_from(&endpoint.error);
    }
    input
}

fn radius(
    guide: &mut Guide,
    class: Option<&Class>,
    entity: &Entity,
    key: &str,
) -> Result<f64, String> {
    let mut input = input(class, entity, key, true);
    let result = if input.error.is_none() && input.raw.is_none() {
        input.value_source = "helper_default";
        input.raw = Some("0".into());
        Ok(0.0)
    } else {
        numbers(&input, 1).map(|values| values[0])
    };
    guide.inputs.push(input);
    let value = result?;
    if !(0.0..=MAX_HELPER_EXTENT).contains(&value) {
        return Err("Cylinder radii must be finite nonnegative values up to 65536".into());
    }
    Ok(value)
}

fn cylinder(
    guide: &mut Guide,
    document: &Document,
    class: &Class,
    entity: &Entity,
    catalog: &Catalog,
    targets: &lines::Targets<'_>,
    helper: &Helper,
) -> Result<(), String> {
    let args = &helper.arguments;
    for triple in args[3..].as_chunks::<3>().0 {
        let endpoint = targets.endpoint(class, entity, &triple[0], &triple[1]);
        guide.inputs.push(reference_input(class, entity, &endpoint));
        guide.endpoints.push(endpoint);
    }
    if let Some(error) = guide.inputs.iter().find_map(|input| input.error.clone()) {
        return Err(error);
    }
    if let Some(endpoint) = guide
        .endpoints
        .iter()
        .find(|endpoint| endpoint.state != "ready")
    {
        return Err(endpoint
            .error
            .clone()
            .unwrap_or_else(|| "Cylinder endpoint is unavailable".into()));
    }
    let mut points = [DVec3::ZERO; 2];
    let mut radii = [0.0; 2];
    for index in 0..2 {
        let endpoint = &guide.endpoints[index];
        points[index] = DVec3::from_array(endpoint.position.unwrap());
        let entity = document
            .entity(endpoint.entity.unwrap())
            .ok_or("Cylinder endpoint no longer exists")?;
        let class = entity.classname().and_then(|name| catalog.class(name));
        radii[index] = radius(guide, class, entity, &args[5 + index * 3])?;
    }
    guide.radii = Some(radii);
    guide.color = Some(std::array::from_fn(|index| args[index].parse().unwrap()));
    guide.finish(geometry::cylinder(points, radii)?)
}

fn typed(
    guide: &mut Guide,
    class: &Class,
    entity: &Entity,
    property: &Property,
    targets: &lines::Targets<'_>,
) -> Result<(), String> {
    guide.marker_policy = Some(MARKER_POLICY);
    guide
        .inputs
        .push(input(Some(class), entity, &property.name, true));
    let mut segments = Vec::new();
    if guide.kind == "axis" {
        let input = &guide.inputs[0];
        if let Some(error) = &input.error {
            return Err(error.clone());
        }
        let (first, second) = input
            .raw
            .as_deref()
            .and_then(|raw| raw.split_once(','))
            .ok_or("Axis helper needs two world points separated by one comma")?;
        let first = values::numbers(first, &[3]).map(|values| DVec3::from_slice(&values));
        let second = values::numbers(second, &[3]).map(|values| DVec3::from_slice(&values));
        let (first, second) = first
            .zip(second)
            .ok_or("Axis helper needs six finite native coordinates")?;
        geometry::marker(&mut segments, first);
        geometry::marker(&mut segments, second);
        segments.push([first.to_array(), second.to_array()]);
        guide.color = Some([255; 3]);
    } else {
        let point = vector(&guide.inputs[0])?;
        geometry::marker(&mut segments, point);
        if guide.kind == "vecline" {
            let anchor = guide.anchor(class, entity, targets)?;
            if point.as_vec3() != anchor.as_vec3() {
                segments.push([point.to_array(), anchor.to_array()]);
            }
            guide.color = Some([255; 3]);
        } else {
            guide.color = Some([128, 128, 255]);
        }
    }
    guide.finish(segments)
}

fn header_kind(helper: &Helper) -> Option<&'static str> {
    match helper.name.as_str() {
        "wirebox" => Some("wirebox"),
        "cylinder" => Some("cylinder"),
        _ => None,
    }
}

fn property_kind(property: &Property) -> Option<&'static str> {
    match property.kind.as_str() {
        "origin" => Some("origin"),
        "vecline" => Some("vecline"),
        "axis" => Some("axis"),
        _ => None,
    }
}

pub fn resolve(
    document: &Document,
    class: &Class,
    entity: &Entity,
    catalog: &Catalog,
    targets: &lines::Targets<'_>,
) -> Vec<Guide> {
    let mut result = Vec::new();
    let mut omitted = 0;
    for (index, helper) in class.helpers.iter().enumerate() {
        let Some(kind) = header_kind(helper) else {
            continue;
        };
        if result.len() == MAX_GUIDES_PER_ENTITY {
            omitted += 1;
            continue;
        }
        let mut guide = Guide::new(kind);
        guide.declaration_index = Some(index);
        guide.arguments = helper
            .arguments
            .iter()
            .take(9)
            .map(|arg| values::bounded(arg, MAX_VALUE_BYTES))
            .collect();
        guide.arguments_truncated = helper.arguments.len() > 9
            || helper
                .arguments
                .iter()
                .any(|arg| arg.len() > MAX_VALUE_BYTES);
        let check = if !supported(class, helper) {
            guide.state = "unsupported";
            Err("Static helper declaration has unsupported arguments or bindings".into())
        } else if kind == "wirebox" {
            wirebox(&mut guide, class, entity, helper, targets)
        } else {
            cylinder(
                &mut guide, document, class, entity, catalog, targets, helper,
            )
        };
        guide.error = check.err().map(|error| values::bounded(&error, 512));
        result.push(guide);
    }
    for property in &class.properties {
        let Some(kind) = property_kind(property) else {
            continue;
        };
        if result.len() == MAX_GUIDES_PER_ENTITY {
            omitted += 1;
            continue;
        }
        let mut guide = Guide::new(kind);
        guide.property = Some(values::bounded(&property.name, MAX_KEY_BYTES));
        guide.property_truncated = property.name.len() > MAX_KEY_BYTES;
        guide.error = typed(&mut guide, class, entity, property, targets)
            .err()
            .map(|error| values::bounded(&error, 512));
        result.push(guide);
    }
    if omitted > 0 {
        let mut guide = Guide::new("omitted");
        guide.state = "omitted";
        guide.omitted = omitted;
        guide.error = Some(format!(
            "Static guides support at most {MAX_GUIDES_PER_ENTITY} declarations/properties per entity; {omitted} were omitted"
        ));
        result.push(guide);
    }
    result
}
