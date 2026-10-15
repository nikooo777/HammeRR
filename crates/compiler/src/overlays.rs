use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use glam::DVec3;
use source_formats::bsp::{self, Overlay, OverlayData, OverlayFade};
use source_formats::gamefs::asset_relative;
use source_formats::vmf::{Entity, Vmf, format_vec3, parse_vec3};

use crate::map::{MAX_COMPILE_COORD, Map};
use crate::{Control, Diagnostic, Report};

const MAX_SOURCE_REFERENCES: usize = 65_536;
const BASIS_TOLERANCE: f64 = 0.01;

pub(crate) struct Definition {
    pub entity: u32,
    pub record: Overlay,
    pub fade: OverlayFade,
    pub sides: Vec<u32>,
    pub accessor: Option<Vec<(String, String)>>,
}

#[derive(Default)]
pub(crate) struct Plan {
    pub definitions: Vec<Definition>,
    membership: BTreeMap<u32, Arc<[usize]>>,
}

impl Plan {
    pub fn membership(&self, source: Option<u32>) -> Arc<[usize]> {
        source
            .and_then(|source| self.membership.get(&source))
            .cloned()
            .unwrap_or_default()
    }

    pub fn data(&self, bindings: &Bindings, control: &Control) -> Result<OverlayData, String> {
        let mut data = OverlayData::default();
        for definition in &self.definitions {
            control.checkpoint()?;
            let mut targets = BTreeSet::new();
            for side in &definition.sides {
                let faces = bindings.faces.get(side).ok_or_else(|| {
                    format!(
                        "entity:{}: overlay side {side} has no emitted world face; choose a visible exposed world, detail or terrain surface",
                        definition.entity
                    )
                })?;
                for &face in faces {
                    targets.insert(face);
                    if targets.len() > bsp::MAX_OVERLAY_FACES {
                        return Err(format!(
                            "entity:{}: overlay exceeds its {} compiled-face budget; use fewer surfaces or a larger lightmap scale",
                            definition.entity,
                            bsp::MAX_OVERLAY_FACES
                        ));
                    }
                }
            }
            let mut record = definition
                .record
                .with_face_count(targets.len())
                .map_err(|error| error.to_string())?;
            for (slot, target) in record.faces.iter_mut().zip(targets) {
                *slot = i32::from(target);
            }
            data.overlays.push(record);
            data.fades.push(definition.fade);
        }
        Ok(data)
    }
}

#[derive(Default)]
pub(crate) struct Bindings {
    faces: BTreeMap<u32, Vec<u16>>,
    references: usize,
}

impl Bindings {
    pub fn add(&mut self, sources: &[u32], face: u16, plan: &Plan) -> Result<(), String> {
        for source in sources {
            if !plan.membership.contains_key(source) {
                continue;
            }
            self.references += 1;
            if self.references > MAX_SOURCE_REFERENCES {
                return Err("Overlay face bindings exceed their 65,536-reference budget".into());
            }
            let faces = self.faces.entry(*source).or_default();
            if faces.len() >= bsp::MAX_OVERLAY_FACES {
                return Err(format!(
                    "Overlay side {source} exceeds its {} compiled-face budget; increase its lightmap scale",
                    bsp::MAX_OVERLAY_FACES
                ));
            }
            faces.push(face);
        }
        Ok(())
    }
}

pub(crate) fn load(
    vmf: &Vmf,
    map: &mut Map,
    report: &mut Report,
    control: &Control,
) -> Result<(), String> {
    let mut entities = vmf
        .entities
        .iter()
        .filter(|entity| {
            !entity.hidden
                && entity
                    .classname()
                    .is_some_and(|class| class.eq_ignore_ascii_case("info_overlay"))
        })
        .peekable();
    if entities.peek().is_none() {
        return Ok(());
    }
    let mut sources = BTreeSet::new();
    for brush in &map.brushes {
        control.checkpoint()?;
        if brush.entity == 0 {
            sources.extend(
                brush
                    .sides
                    .iter()
                    .filter(|side| side.visible)
                    .filter_map(|side| side.source_id),
            );
        }
    }
    sources.extend(map.terrains.iter().map(|terrain| terrain.side));
    let mut membership = BTreeMap::<u32, Vec<usize>>::new();
    let mut references = 0;
    for entity in entities {
        control.checkpoint()?;
        let definition = if map.overlays.definitions.len() >= bsp::MAX_OVERLAYS {
            Err(format!(
                "Overlays exceed their {}-record allocation budget",
                bsp::MAX_OVERLAYS
            ))
        } else {
            parse(entity, map, &sources, map.overlays.definitions.len())
        };
        match definition {
            Ok(definition) => {
                references += definition.sides.len();
                if references > MAX_SOURCE_REFERENCES {
                    return Err("Overlay sources exceed their 65,536-reference budget".into());
                }
                let index = map.overlays.definitions.len();
                for side in &definition.sides {
                    membership.entry(*side).or_default().push(index);
                }
                map.overlays.definitions.push(definition);
            }
            Err(error) => report.diagnostics.push(Diagnostic::error(
                error,
                Some(format!("entity:{}", entity.id)),
                entity.origin(),
            )),
        }
    }
    map.overlays.membership = membership
        .into_iter()
        .map(|(side, scope)| (side, scope.into()))
        .collect();
    Ok(())
}

fn number(entity: &Entity, key: &str, default: f32) -> Result<f32, String> {
    entity.properties.get(key).map_or(Ok(default), |raw| {
        raw.trim()
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or_else(|| format!("Overlay {key} must be a finite native number"))
    })
}

fn vector(entity: &Entity, key: &str) -> Result<DVec3, String> {
    entity
        .properties
        .get(key)
        .and_then(parse_vec3)
        .filter(|value| value.is_finite() && value.as_vec3().is_finite())
        .ok_or_else(|| format!("Overlay {key} must contain three finite native numbers"))
}

fn fade(entity: &Entity, key: &str, default: f32) -> Result<f32, String> {
    let value = number(entity, key, default)?;
    let squared = if value > 0.0 { value * value } else { value };
    if !squared.is_finite() {
        return Err(format!(
            "Overlay {key} squared distance exceeds native float bounds"
        ));
    }
    Ok(squared)
}

fn parse(
    entity: &Entity,
    map: &mut Map,
    sources: &BTreeSet<u32>,
    id: usize,
) -> Result<Definition, String> {
    if entity.solids.iter().any(|solid| !solid.hidden) {
        return Err("info_overlay is a point helper; remove its brush geometry".into());
    }
    let normal = vector(entity, "BasisNormal")?;
    let u = vector(entity, "BasisU")?;
    let v = vector(entity, "BasisV")?;
    if [normal, u, v]
        .iter()
        .any(|axis| (axis.length_squared() - 1.0).abs() > BASIS_TOLERANCE)
        || normal
            .dot(u)
            .abs()
            .max(normal.dot(v).abs())
            .max(u.dot(v).abs())
            > BASIS_TOLERANCE
    {
        return Err(
            "Overlay BasisNormal, BasisU and BasisV must form perpendicular unit axes".into(),
        );
    }
    let origin = vector(entity, "BasisOrigin")?;
    let mut points = [[0.0; 3]; 4];
    for (index, point) in points.iter_mut().enumerate() {
        let uv = vector(entity, &format!("uv{index}"))?;
        let world = origin + u * uv.x + v * uv.y;
        if world.abs().max_element() > MAX_COMPILE_COORD {
            return Err(format!(
                "Overlay projected corners must fit ±{MAX_COMPILE_COORD} units"
            ));
        }
        *point = [uv.x as f32, uv.y as f32, 0.0];
    }
    if origin.abs().max_element() > MAX_COMPILE_COORD {
        return Err(format!(
            "Overlay BasisOrigin must fit ±{MAX_COMPILE_COORD} units"
        ));
    }
    let mut turn = 0.0f64;
    for index in 0..4 {
        let [a, b, c] = [index, (index + 1) % 4, (index + 2) % 4].map(|index| points[index]);
        let cross = (f64::from(b[0]) - f64::from(a[0])) * (f64::from(c[1]) - f64::from(b[1]))
            - (f64::from(b[1]) - f64::from(a[1])) * (f64::from(c[0]) - f64::from(b[0]));
        if cross.abs() < 1e-8 || (turn != 0.0 && cross.signum() != turn.signum()) {
            return Err(
                "Overlay uv0–uv3 must form a nondegenerate convex quadrilateral in perimeter order"
                    .into(),
            );
        }
        turn = cross;
    }
    for index in 0..3 {
        points[index][2] = u[index] as f32;
    }
    points[3][2] = u8::from(normal.cross(u).dot(v) < 0.0) as f32;
    let texture_u = [number(entity, "StartU", 0.0)?, number(entity, "EndU", 1.0)?];
    let texture_v = [number(entity, "StartV", 0.0)?, number(entity, "EndV", 1.0)?];
    if texture_u[0] == texture_u[1] || texture_v[0] == texture_v[1] {
        return Err("Overlay U/V start and end must differ".into());
    }
    let order = entity
        .properties
        .get("RenderOrder")
        .unwrap_or("0")
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|order| *order <= 3)
        .ok_or("Overlay RenderOrder must be an integer from 0 to 3")?;
    let fade = OverlayFade {
        min_distance_squared: fade(entity, "fademindist", -1.0)?,
        max_distance_squared: fade(entity, "fademaxdist", 0.0)?,
    };
    let mut sides = BTreeSet::new();
    for raw in entity
        .properties
        .get("sides")
        .unwrap_or("")
        .split_whitespace()
    {
        let side = raw
            .parse::<u32>()
            .map_err(|_| "Overlay sides must contain integer VMF side IDs")?;
        if !sources.contains(&side) {
            return Err(format!(
                "Overlay side {side} is absent, hidden, non-rendering or belongs to an unsupported brush model; choose world, detail or terrain surfaces"
            ));
        }
        sides.insert(side);
        if sides.len() > bsp::MAX_OVERLAY_FACES {
            return Err(format!(
                "Overlay sources exceed their {}-side allocation budget",
                bsp::MAX_OVERLAY_FACES
            ));
        }
    }
    if sides.is_empty() {
        return Err("Overlay requires at least one visible source side in sides".into());
    }
    let material = entity.properties.get("material").unwrap_or("").trim();
    let material =
        asset_relative(material).map_err(|error| format!("Overlay material: {error}"))?;
    let material = material.strip_prefix("materials/").unwrap_or(&material);
    let material = material.strip_suffix(".vmt").unwrap_or(material);
    if material.is_empty() || material.contains(['"', '\n', '\r']) {
        return Err("Overlay material must name a relative game material".into());
    }
    let material = map.materials.load(material)?;
    let texdata = map.texture_data(material);
    let texinfo = map.texinfos.add(bsp::TexInfo {
        texture_vecs: [[0.0, 0.0, 0.0, -99999.0]; 2],
        lightmap_vecs: [[0.0, 0.0, 0.0, -99999.0]; 2],
        flags: 0,
        texdata: i32::try_from(texdata).map_err(|_| "Overlay texture data limit exceeded")?,
    });
    let record = Overlay {
        id: id as i32,
        texinfo: i16::try_from(texinfo).map_err(|_| "Overlay texture info limit exceeded")?,
        u: texture_u,
        v: texture_v,
        uv_points: points,
        origin: origin.as_vec3().to_array(),
        basis_normal: normal.as_vec3().to_array(),
        ..Overlay::default()
    }
    .with_render_order(order)
    .map_err(|error| error.to_string())?;
    let accessor = entity
        .properties
        .get("targetname")
        .filter(|name| !name.trim().is_empty())
        .map(|name| {
            vec![
                ("classname".into(), "info_overlay_accessor".into()),
                ("targetname".into(), name.to_owned()),
                ("origin".into(), format_vec3(origin)),
                ("OverlayID".into(), id.to_string()),
                ("hammerid".into(), entity.id.to_string()),
            ]
        });
    Ok(Definition {
        entity: entity.id,
        record,
        fade,
        sides: sides.into_iter().collect(),
        accessor,
    })
}
