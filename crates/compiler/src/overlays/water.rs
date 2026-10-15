use std::collections::{BTreeMap, BTreeSet};

use source_formats::bsp::{self, WaterOverlay, WaterOverlayData};
use source_formats::vmf::{Entity, Vmf};

use crate::map::Map;
use crate::{Control, Diagnostic, Report};

const MAX_FIELDS: usize = 32;
const MAX_FIELD_BYTES: usize = 4096;

pub(super) struct Definition {
    entity: u32,
    record: WaterOverlay,
    sides: Vec<u32>,
}

impl super::Plan {
    pub fn water_data(
        &self,
        bindings: &super::Bindings,
        control: &Control,
    ) -> Result<WaterOverlayData, String> {
        let mut data = WaterOverlayData::default();
        if self.water.is_empty() {
            return Ok(data);
        }
        let mut references = 0;
        for definition in &self.definitions {
            control.checkpoint()?;
            references += bindings
                .targets(&definition.sides, definition.entity, bsp::MAX_OVERLAY_FACES)?
                .len();
        }
        for definition in &self.water {
            control.checkpoint()?;
            let targets = bindings.targets(
                &definition.sides,
                definition.entity,
                bsp::MAX_WATER_OVERLAY_FACES,
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

fn definitions(entity: &Entity) -> Result<Vec<Entity>, String> {
    if entity.solids.iter().any(|solid| !solid.hidden) {
        return Err("info_overlay_transition is a point helper; remove its brush geometry".into());
    }
    let mut definitions = Vec::new();
    for transition in entity
        .extra
        .iter()
        .filter(|node| node.key.eq_ignore_ascii_case("overlaytransition"))
    {
        let children = transition
            .children()
            .ok_or("Water overlaytransition must be a block")?;
        if children.is_empty() {
            return Err("Water overlaytransition needs at least one overlaydata block".into());
        }
        for data in children {
            if !data.key.eq_ignore_ascii_case("overlaydata") {
                return Err(
                    "Water overlaytransition supports only explicit overlaydata blocks".into(),
                );
            }
            if definitions.len() >= bsp::MAX_WATER_OVERLAYS {
                return Err("Water overlays exceed their 1024-record allocation budget".into());
            }
            let fields = data.children().ok_or("Water overlaydata must be a block")?;
            if fields.len() > MAX_FIELDS {
                return Err("Water overlaydata exceeds its 32-field budget".into());
            }
            let mut projection = Entity::new(entity.id, "info_overlay");
            let mut keys = BTreeSet::new();
            for field in fields {
                let value = field
                    .as_str()
                    .ok_or("Water overlaydata fields must be strings")?;
                if field.key.len() > MAX_FIELD_BYTES || value.len() > MAX_FIELD_BYTES {
                    return Err("Water overlaydata field exceeds its 4096-byte budget".into());
                }
                if !keys.insert(field.key.to_ascii_lowercase()) {
                    return Err(format!("Water overlaydata repeats field {}", field.key));
                }
                if [
                    "RenderOrder",
                    "fademindist",
                    "fademaxdist",
                    "targetname",
                    "classname",
                ]
                .iter()
                .any(|key| field.key.eq_ignore_ascii_case(key))
                {
                    return Err(format!("Water overlaydata does not support {}", field.key));
                }
                projection
                    .properties
                    .0
                    .push((field.key.clone(), value.to_owned()));
            }
            definitions.push(projection);
        }
    }
    if definitions.is_empty() {
        return Err(
            "info_overlay_transition needs nested overlaytransition/overlaydata projection blocks"
                .into(),
        );
    }
    Ok(definitions)
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
    for entity in vmf.entities.iter().filter(|entity| {
        !entity.hidden
            && entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("info_overlay_transition"))
    }) {
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
    for projection in definitions(entity)? {
        control.checkpoint()?;
        if map.overlays.water.len() >= bsp::MAX_WATER_OVERLAYS {
            return Err("Water overlays exceed their 1024-record allocation budget".into());
        }
        let index = map.overlays.water.len();
        let definition = super::parse(&projection, map, sources, index)?;
        *references += definition.sides.len();
        if *references > super::MAX_SOURCE_REFERENCES {
            return Err("Overlay sources exceed their shared 65,536-reference budget".into());
        }
        for side in &definition.sides {
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
            entity: entity.id,
            record,
            sides: definition.sides,
        });
    }
    Ok(())
}
