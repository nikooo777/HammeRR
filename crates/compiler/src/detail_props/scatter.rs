use std::collections::BTreeMap;

use glam::{DMat3, DQuat, DVec3, EulerRot};
use source_formats::bsp::{DetailProp, contents};
use source_formats::gamefs::asset_relative;
use source_formats::keyvalues::{self, Node};
use source_formats::vmf::Vmf;

use super::Builder;
use crate::{Control, Diagnostic, Report, emit, faces::Face, map::Map};

const MAX_DICTIONARY_BYTES: usize = 2 * 1024 * 1024;
const MAX_DICTIONARY_NODES: usize = 32_768;
const MAX_SAMPLES: usize = 65_536;
const DENSITY_AREA: f64 = 1_000_000.0;
const LEAF_BIAS: f64 = 0.03125;

struct Model {
    path: String,
    amount: f64,
    min_cos: f64,
    max_cos: f64,
    upright: bool,
}

struct Definition {
    density: f64,
    models: Vec<Model>,
}

fn value<'a>(nodes: &'a [Node], key: &str) -> Result<Option<&'a str>, String> {
    let mut found = nodes
        .iter()
        .filter(|node| node.key.eq_ignore_ascii_case(key));
    let Some(node) = found.next() else {
        return Ok(None);
    };
    if found.next().is_some() {
        return Err(format!("Detail dictionary repeats {key}"));
    }
    node.as_str()
        .map(Some)
        .ok_or_else(|| format!("Detail dictionary {key} must be a value"))
}

fn number(nodes: &[Node], key: &str, default: f64, min: f64, max: f64) -> Result<f64, String> {
    let Some(raw) = value(nodes, key)? else {
        return Ok(default);
    };
    raw.trim()
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite() && (min..=max).contains(number))
        .ok_or_else(|| format!("Detail dictionary {key} must be finite and within {min}–{max}"))
}

fn model(nodes: &[Node]) -> Result<Model, String> {
    for node in nodes {
        if ![
            "model",
            "amount",
            "upright",
            "minangle",
            "maxangle",
            "detailorientation",
        ]
        .iter()
        .any(|key| node.key.eq_ignore_ascii_case(key))
        {
            return Err(format!(
                "Detail dictionary field {} is unsupported; material scattering supports models only, without sprites or shapes",
                node.key
            ));
        }
    }
    let path = asset_relative(value(nodes, "model")?.ok_or(
        "Detail dictionary entry requires a model; sprite and shape scattering is unsupported",
    )?)
    .map_err(|error| error.to_string())?;
    if !path.starts_with("models/") || !path.ends_with(".mdl") {
        return Err("Detail dictionary model must name a models/*.mdl asset".into());
    }
    let upright = number(nodes, "upright", 0.0, 0.0, 1.0)?;
    if upright != 0.0 && upright != 1.0 {
        return Err("Detail dictionary upright must be 0 or 1".into());
    }
    if number(nodes, "detailOrientation", 0.0, 0.0, 2.0)? != 0.0 {
        return Err("Detail dictionary models require detailOrientation=0".into());
    }
    let min_cos = number(nodes, "minAngle", 180.0, 0.0, 180.0)?
        .to_radians()
        .cos();
    let max_cos = number(nodes, "maxAngle", 180.0, 0.0, 180.0)?
        .to_radians()
        .cos();
    Ok(Model {
        path,
        amount: number(nodes, "amount", 1.0, 0.0, DENSITY_AREA)?,
        min_cos: min_cos.max(max_cos),
        max_cos,
        upright: upright == 1.0,
    })
}

fn definition(nodes: &[Node]) -> Result<Definition, String> {
    let density = number(nodes, "density", 0.0, 0.0, DENSITY_AREA)?;
    let mut groups = Vec::new();
    for node in nodes {
        let Some(children) = node.children() else {
            if !node.key.eq_ignore_ascii_case("density") {
                return Err(format!("Unsupported detail type field {}", node.key));
            }
            continue;
        };
        let alpha = number(children, "alpha", 1.0, 0.0, 1.0)?;
        let mut models = Vec::new();
        let mut total = 0.0;
        for entry in children {
            if let Some(fields) = entry.children() {
                let mut parsed = model(fields)?;
                total += parsed.amount;
                parsed.amount = total;
                models.push(parsed);
            } else if !entry.key.eq_ignore_ascii_case("alpha") {
                return Err(format!("Unsupported detail group field {}", entry.key));
            }
        }
        if total > 1.0 {
            for model in &mut models {
                model.amount /= total;
            }
        }
        if groups.iter().any(|(existing, _)| *existing == alpha) {
            return Err("Detail dictionary group alpha values must be distinct".into());
        }
        groups.push((alpha, models));
    }
    groups.sort_by(|left, right| left.0.total_cmp(&right.0));
    let (_, models) = groups
        .pop()
        .ok_or("Detail type must contain a model group")?;
    Ok(Definition { density, models })
}

fn detail_type<'a>(map: &'a Map, face: &Face) -> Option<&'a str> {
    let info = &map.texinfos.infos[face.texinfo];
    map.materials
        .get(map.texdata[info.texdata as usize])
        .detail_type
        .as_deref()
}

pub(super) fn requested(map: &Map, models: &[emit::Model]) -> bool {
    models
        .iter()
        .flat_map(|model| &model.faces)
        .any(|face| detail_type(map, face).is_some())
}

struct Random(u32);

impl Random {
    fn face(face: &Face) -> Result<Self, String> {
        let mut seed = *face
            .source_sides
            .first()
            .ok_or("Material detail scattering requires an authored face ID")?;
        for point in &face.winding.0 {
            for value in crate::emit::stored_vertex(*point).as_vec3().to_array() {
                seed = seed.rotate_left(5) ^ value.to_bits();
            }
        }
        Ok(Self(seed))
    }

    fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        (f64::from((self.0 >> 16) & 0x7fff) + 0.5) / 32_768.0
    }
}

fn angles(normal: DVec3, upright: bool, yaw: f64) -> [f32; 3] {
    if upright {
        return [0.0, yaw as f32, 0.0];
    }
    let arbitrary = if normal.x.abs() > 0.999 {
        DVec3::Y
    } else {
        DVec3::X
    };
    let y = normal.cross(arbitrary).normalize();
    let x = y.cross(normal).normalize();
    let rotation = DQuat::from_mat3(&DMat3::from_cols(x, y, normal))
        * DQuat::from_rotation_z(yaw.to_radians());
    let (yaw, pitch, roll) = rotation.to_euler(EulerRot::ZYX);
    [pitch, yaw, roll].map(|value| value.to_degrees() as f32)
}

fn emit_face(
    map: &Map,
    face: &Face,
    definition: &Definition,
    builder: &mut Builder,
    samples: &mut usize,
    control: &Control,
) -> Result<(), String> {
    if face.displacement.is_some() {
        return Err("Material detail scattering on terrain is unsupported; use a flat world or func_detail face".into());
    }
    let material = map
        .materials
        .get(map.texdata[map.texinfos.infos[face.texinfo].texdata as usize]);
    if material.contents & (contents::WATER | contents::SLIME | contents::AREAPORTAL) != 0 {
        return Err(
            "Material detail scattering requires a solid world or func_detail surface".into(),
        );
    }
    if definition.models.is_empty() {
        return Ok(());
    }
    let mut random = Random::face(face)?;
    let normal = map.planes.get(face.plane).normal;
    let points = face
        .winding
        .0
        .iter()
        .copied()
        .map(crate::emit::stored_vertex)
        .collect::<Vec<_>>();
    let first = *points
        .first()
        .ok_or("Detail scattering face has no polygon")?;
    for triangle in points[1..].windows(2) {
        control.checkpoint()?;
        let e1 = triangle[0] - first;
        let e2 = triangle[1] - first;
        let area = e1.cross(e2).length() * 0.5;
        let count = (area * definition.density / DENSITY_AREA).floor();
        if !count.is_finite() || count > MAX_SAMPLES.saturating_sub(*samples) as f64 {
            return Err("Material detail scattering exceeds its 65,536-sample build budget".into());
        }
        *samples += count as usize;
        for _ in 0..count as usize {
            control.checkpoint()?;
            let mut u = random.unit();
            let mut v = random.unit();
            if u + v > 1.0 {
                u = 1.0 - u;
                v = 1.0 - v;
            }
            let choice = random.unit();
            let Some(model) = definition.models.iter().find(|model| choice < model.amount) else {
                continue;
            };
            if normal.z < model.max_cos {
                continue;
            }
            if normal.z < model.min_cos
                && random.unit() > (normal.z - model.max_cos) / (model.min_cos - model.max_cos)
            {
                continue;
            }
            let origin = (first + u * e1 + v * e2).as_vec3();
            let prop = DetailProp {
                origin: origin.to_array(),
                angles: angles(normal, model.upright, random.unit() * 360.0),
                ..Default::default()
            };
            let inward = face.winding.center() - origin.as_dvec3();
            let leaf_point = origin.as_dvec3()
                + normal * LEAF_BIAS
                + inward * (LEAF_BIAS / inward.length().max(LEAF_BIAS));
            builder.add(&model.path, prop, leaf_point, control)?;
        }
    }
    Ok(())
}

fn dictionary(vmf: &Vmf, builder: &Builder) -> Result<Vec<Node>, String> {
    let raw = vmf
        .world
        .properties
        .get("detailvbsp")
        .unwrap_or("detail.vbsp")
        .trim();
    let raw = if raw.is_empty() { "detail.vbsp" } else { raw };
    let path = asset_relative(raw).map_err(|error| error.to_string())?;
    if !path.ends_with(".vbsp") {
        return Err("World detailvbsp must name a game-relative .vbsp dictionary".into());
    }
    let bytes = builder
        .fs
        .read_limited(&path, MAX_DICTIONARY_BYTES)
        .map_err(|error| format!("Detail dictionary {path}: {error}"))?
        .ok_or_else(|| format!("Detail dictionary {path} was not found"))?;
    let text = std::str::from_utf8(&bytes).map_err(|error| format!("{path}: {error}"))?;
    let nodes =
        keyvalues::parse_bounded(text.trim_start_matches('\u{feff}'), 8, MAX_DICTIONARY_NODES)
            .map_err(|error| format!("Detail dictionary {path}: {error}"))?;
    if nodes.len() != 1 {
        return Err("Detail dictionary requires one root block".into());
    }
    nodes[0]
        .children()
        .map(<[Node]>::to_vec)
        .ok_or_else(|| "Detail dictionary requires one root block".into())
}

pub(super) fn bake(
    vmf: &Vmf,
    map: &Map,
    models: &[emit::Model],
    builder: &mut Builder,
    report: &mut Report,
    control: &Control,
) -> Result<(), String> {
    if !requested(map, models) {
        return Ok(());
    }
    control.checkpoint()?;
    let nodes = match dictionary(vmf, builder) {
        Ok(nodes) => nodes,
        Err(message) => {
            report.diagnostics.push(Diagnostic::error(
                message,
                Some(format!("entity:{}", vmf.world.id)),
                None,
            ));
            return Ok(());
        }
    };
    let mut definitions = BTreeMap::new();
    let mut samples = 0;
    for model in models {
        for face in &model.faces {
            control.checkpoint()?;
            let Some(name) = detail_type(map, face) else {
                continue;
            };
            let result = (|| {
                if model.entity != 0 {
                    return Err("Material detail scattering on runtime brush models is unsupported; use world or func_detail surfaces".into());
                }
                if !definitions.contains_key(name) {
                    let matches = nodes
                        .iter()
                        .filter(|node| node.key.eq_ignore_ascii_case(name))
                        .collect::<Vec<_>>();
                    if matches.len() != 1 {
                        return Err(format!(
                            "Material %detailtype {name} must name exactly one dictionary type"
                        ));
                    }
                    let fields = matches[0]
                        .children()
                        .ok_or("Detail type requires a block")?;
                    let parsed = definition(fields)?;
                    if parsed.models.is_empty() {
                        report.diagnostics.push(Diagnostic::info(
                            format!("Material %detailtype {name} selects an empty group for flat-face alpha 1; this type emits no detail models"),
                            None,
                            None,
                        ));
                    }
                    definitions.insert(name.to_owned(), parsed);
                }
                emit_face(
                    map,
                    face,
                    &definitions[name],
                    builder,
                    &mut samples,
                    control,
                )
            })();
            if let Err(message) = result {
                control.checkpoint()?;
                let source = face.source_sides.first().and_then(|side| {
                    map.brushes
                        .iter()
                        .find(|brush| {
                            brush
                                .sides
                                .iter()
                                .any(|candidate| candidate.source_id == Some(*side))
                        })
                        .map(|brush| format!("solid:{}", brush.id))
                });
                report.diagnostics.push(Diagnostic::error(
                    format!("Material %detailtype {name}: {message}"),
                    source,
                    Some(face.winding.center()),
                ));
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_source_angles_align_model_up_with_the_surface_normal() {
        for normal in [
            DVec3::Z,
            DVec3::NEG_Z,
            DVec3::X,
            DVec3::new(0.3, -0.4, 0.75_f64.sqrt()),
        ] {
            for yaw in [0.0, 37.0, 179.0, 271.0] {
                let values = angles(normal, false, yaw).map(|value| f64::from(value).to_radians());
                let rotation = DQuat::from_euler(EulerRot::ZYX, values[1], values[0], values[2]);
                assert!((rotation * DVec3::Z - normal).length() < 1e-5);
                assert_eq!(angles(normal, true, yaw), [0.0, yaw as f32, 0.0]);
            }
        }
    }
}
