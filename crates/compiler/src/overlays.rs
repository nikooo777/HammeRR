use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use glam::DVec3;
use map_core::overlay_projection::Frame;
use source_formats::bsp::{self, Overlay, OverlayData, OverlayFade};
use source_formats::vmf::{Entity, Vmf, format_vec3};

use crate::map::Map;
use crate::{Control, Diagnostic, Report};

mod water;

const MAX_SOURCE_REFERENCES: usize = 65_536;

pub(crate) struct Source {
    pub entity: u32,
    sides: Vec<u32>,
    frame: Frame,
}

pub(crate) struct Definition {
    pub source: Source,
    pub record: Overlay,
    pub fade: OverlayFade,
    pub accessor: Option<Vec<(String, String)>>,
}

#[derive(Default)]
pub(crate) struct Plan {
    pub definitions: Vec<Definition>,
    water: Vec<water::Definition>,
    membership: BTreeMap<u32, Arc<[usize]>>,
}

impl Plan {
    pub fn membership(&self, source: Option<u32>) -> Arc<[usize]> {
        source
            .and_then(|source| self.membership.get(&source))
            .cloned()
            .unwrap_or_default()
    }

    pub fn data(
        &self,
        bindings: &Bindings,
        report: &mut Report,
        control: &Control,
        covers: &impl Fn(u16, &Frame) -> bool,
    ) -> Result<OverlayData, String> {
        let mut data = OverlayData::default();
        for definition in &self.definitions {
            control.checkpoint()?;
            let targets =
                bindings.targets(&definition.source, bsp::MAX_OVERLAY_FACES, report, covers)?;
            let mut record = definition
                .record
                .with_face_count(targets.len())
                .map_err(|error| error.to_string())
                .inspect_err(|message| helper_error(report, definition, message))?;
            for (slot, target) in record.faces.iter_mut().zip(targets) {
                *slot = i32::from(target);
            }
            data.overlays.push(record);
            data.fades.push(definition.fade);
        }
        Ok(data)
    }
}

fn helper_error(report: &mut Report, definition: &Definition, message: &str) {
    report.diagnostics.push(Diagnostic::error(
        message,
        Some(format!("entity:{}", definition.source.entity)),
        Some(DVec3::from_array(definition.record.origin.map(f64::from))),
    ));
}

#[derive(Default)]
pub(crate) struct Bindings {
    faces: BTreeMap<u32, Vec<u16>>,
    references: usize,
}

impl Bindings {
    fn targets(
        &self,
        source: &Source,
        maximum: usize,
        report: &mut Report,
        covers: &impl Fn(u16, &Frame) -> bool,
    ) -> Result<BTreeSet<u16>, String> {
        let object = || Some(format!("entity:{}", source.entity));
        let position = Some(source.frame.origin());
        let mut targets = BTreeSet::new();
        let mut missing = Vec::new();
        for side in &source.sides {
            let Some(faces) = self.faces.get(side) else {
                missing.push(*side);
                continue;
            };
            for &face in faces {
                targets.insert(face);
                if targets.len() > maximum {
                    let error = format!(
                        "Overlay exceeds its {maximum} compiled-face budget; use fewer surfaces or a larger lightmap scale"
                    );
                    report
                        .diagnostics
                        .push(Diagnostic::error(&error, object(), position));
                    return Err(error);
                }
            }
        }
        if targets.is_empty() {
            let error =
                format!("Overlay sides {missing:?} have no exposed world, detail or terrain faces");
            report
                .diagnostics
                .push(Diagnostic::error(&error, object(), position));
            return Err(error);
        }
        if !missing.is_empty() {
            if !targets.iter().any(|&face| covers(face, &source.frame)) {
                let error = format!(
                    "Overlay sides {missing:?} have no exposed final faces and its projection misses the remaining faces"
                );
                report
                    .diagnostics
                    .push(Diagnostic::error(&error, object(), position));
                return Err(error);
            }
            report.diagnostics.push(Diagnostic::warning(
                format!("Overlay sides {missing:?} produced no exposed final faces; the projection uses its remaining faces"),
                object(),
                position,
            ));
        }
        Ok(targets)
    }

    pub fn add(
        &mut self,
        sources: &[u32],
        face: u16,
        plan: &Plan,
        report: &mut Report,
    ) -> Result<(), String> {
        for source in sources {
            let Some(membership) = plan.membership.get(source) else {
                continue;
            };
            self.references += 1;
            if self.references > MAX_SOURCE_REFERENCES {
                return Err("Overlay face bindings exceed their 65,536-reference budget".into());
            }
            let faces = self.faces.entry(*source).or_default();
            let maximum = if membership.iter().any(|&index| index >= bsp::MAX_OVERLAYS) {
                bsp::MAX_WATER_OVERLAY_FACES
            } else {
                bsp::MAX_OVERLAY_FACES
            };
            if faces.len() >= maximum {
                let message = format!(
                    "Overlay side {source} exceeds its {} compiled-face budget; increase its lightmap scale",
                    maximum
                );
                if maximum == bsp::MAX_OVERLAY_FACES
                    && let Some(definition) = membership
                        .iter()
                        .find_map(|&index| plan.definitions.get(index))
                {
                    helper_error(report, definition, &message);
                }
                return Err(message);
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
    if !vmf.entities.iter().any(|entity| {
        !entity.hidden
            && entity.classname().is_some_and(|class| {
                class.eq_ignore_ascii_case("info_overlay")
                    || class.eq_ignore_ascii_case("info_overlay_transition")
            })
    }) {
        return Ok(());
    }
    let entities = vmf.entities.iter().filter(|entity| {
        !entity.hidden
            && entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("info_overlay"))
    });
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
                references += definition.source.sides.len();
                if references > MAX_SOURCE_REFERENCES {
                    return Err("Overlay sources exceed their 65,536-reference budget".into());
                }
                let index = map.overlays.definitions.len();
                for side in &definition.source.sides {
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
    water::load(
        vmf,
        map,
        report,
        control,
        &sources,
        &mut membership,
        &mut references,
    )?;
    map.overlays.membership = membership
        .into_iter()
        .map(|(side, scope)| (side, scope.into()))
        .collect();
    Ok(())
}

fn parse(
    entity: &Entity,
    map: &mut Map,
    sources: &BTreeSet<u32>,
    id: usize,
) -> Result<Definition, String> {
    let projection = map_core::overlay::parse(entity)?;
    lower_projection(entity.id, projection, map, sources, id)
}

fn lower_projection(
    entity: u32,
    projection: map_core::overlay::Definition,
    map: &mut Map,
    sources: &BTreeSet<u32>,
    id: usize,
) -> Result<Definition, String> {
    for side in &projection.sides {
        if !sources.contains(side) {
            return Err(format!(
                "Overlay side {side} is absent, hidden, non-rendering or belongs to an unsupported brush model; choose world, detail or terrain surfaces"
            ));
        }
    }
    let normal = DVec3::from_array(projection.basis_normal);
    let u = DVec3::from_array(projection.basis_u);
    let v = DVec3::from_array(projection.basis_v);
    let origin = DVec3::from_array(projection.basis_origin);
    let mut points = projection
        .uv_points
        .map(|point| [point[0] as f32, point[1] as f32, 0.0]);
    for index in 0..3 {
        points[index][2] = u[index] as f32;
    }
    points[3][2] = u8::from(normal.cross(u).dot(v) < 0.0) as f32;
    let [min_distance_squared, max_distance_squared] = projection.fade_squared()?;
    let fade = OverlayFade {
        min_distance_squared,
        max_distance_squared,
    };
    let options = &projection.options;
    let material = map.materials.load(&options.material)?;
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
        u: options.u,
        v: options.v,
        uv_points: points,
        origin: origin.as_vec3().to_array(),
        basis_normal: normal.as_vec3().to_array(),
        ..Overlay::default()
    }
    .with_render_order(options.render_order)
    .map_err(|error| error.to_string())?;
    let accessor = options.targetname.as_ref().map(|name| {
        vec![
            ("classname".into(), "info_overlay_accessor".into()),
            ("targetname".into(), name.to_owned()),
            ("origin".into(), format_vec3(origin)),
            ("OverlayID".into(), id.to_string()),
            ("hammerid".into(), entity.to_string()),
        ]
    });
    Ok(Definition {
        source: Source {
            entity,
            frame: Frame::new(&projection)?,
            sides: projection.sides,
        },
        record,
        fade,
        accessor,
    })
}
