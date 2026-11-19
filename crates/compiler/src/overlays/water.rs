use map_core::overlay::water as transition;
use std::collections::{BTreeMap, BTreeSet};

use source_formats::bsp::{self, WaterOverlay, WaterOverlayData};
use source_formats::vmf::{Entity, Vmf};

use crate::map::Map;
use crate::{Control, Diagnostic, Report};

pub(super) struct Definition {
    source: super::Source,
    record: WaterOverlay,
}

impl super::Plan {
    pub fn water_data(
        &self,
        bindings: &super::Bindings,
        regular_references: usize,
        report: &mut Report,
        control: &Control,
        covers: &impl Fn(u16, &map_core::overlay_projection::Frame) -> bool,
    ) -> Result<WaterOverlayData, String> {
        let mut data = WaterOverlayData::default();
        if self.water.is_empty() {
            return Ok(data);
        }
        let mut references = regular_references;
        for definition in &self.water {
            control.checkpoint()?;
            let targets = bindings.targets(
                &definition.source,
                bsp::MAX_WATER_OVERLAY_FACES,
                report,
                covers,
            )?;
            references += targets.len();
            if references > super::MAX_SOURCE_REFERENCES {
                return Err("Regular and water overlay output exceeds the shared 65,536-face-reference budget".into());
            }
            let mut record = definition
                .record
                .with_face_count(targets.len())
                .map_err(|error| error.to_string())?;
            for (slot, target) in record.faces.iter_mut().zip(targets) {
                *slot = i32::from(target);
            }
            data.overlays.push(record);
        }
        Ok(data)
    }
}

pub(super) fn load(
    vmf: &Vmf,
    map: &mut Map,
    report: &mut Report,
    control: &Control,
    sources: &BTreeSet<u32>,
    membership: &mut BTreeMap<u32, Vec<usize>>,
    references: &mut usize,
) -> Result<(), String> {
    for entity in vmf
        .entities
        .iter()
        .filter(|entity| !entity.hidden && transition::is_transition(entity))
    {
        control.checkpoint()?;
        let result = load_entity(entity, map, control, sources, membership, references);
        control.checkpoint()?;
        if let Err(error) = result {
            report.diagnostics.push(Diagnostic::error(
                error,
                Some(format!("entity:{}", entity.id)),
                entity.origin(),
            ));
        }
    }
    Ok(())
}

fn load_entity(
    entity: &Entity,
    map: &mut Map,
    control: &Control,
    sources: &BTreeSet<u32>,
    membership: &mut BTreeMap<u32, Vec<usize>>,
    references: &mut usize,
) -> Result<(), String> {
    for projection in transition::parse(entity)? {
        control.checkpoint()?;
        if map.overlays.water.len() >= bsp::MAX_WATER_OVERLAYS {
            return Err("Water overlays exceed their 1024-record allocation budget".into());
        }
        let index = map.overlays.water.len();
        let definition = super::lower_projection(entity.id, projection, map, sources, index)?;
        *references += definition.source.sides.len();
        if *references > super::MAX_SOURCE_REFERENCES {
            return Err("Overlay sources exceed their shared 65,536-reference budget".into());
        }
        for side in &definition.source.sides {
            membership
                .entry(*side)
                .or_default()
                .push(bsp::MAX_OVERLAYS + index);
        }
        let normal = definition.record;
        let record = WaterOverlay {
            id: normal.id,
            texinfo: normal.texinfo,
            u: normal.u,
            v: normal.v,
            uv_points: normal.uv_points,
            origin: normal.origin,
            basis_normal: normal.basis_normal,
            ..WaterOverlay::default()
        };
        map.overlays.water.push(Definition {
            source: definition.source,
            record,
        });
    }
    Ok(())
}
