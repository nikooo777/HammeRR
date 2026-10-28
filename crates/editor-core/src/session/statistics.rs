use serde::{Deserialize, Serialize};

use super::*;

pub const PAGE_SIZE: usize = 50;
pub const MAX_PAGE_SIZE: usize = 200;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    #[default]
    Materials,
    EntityClasses,
    Models,
    LightmapScales,
    DisplacementPowers,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub objects: usize,
    pub world_brushes: usize,
    pub entity_brushes: usize,
    pub brush_sides: usize,
    pub point_entities: usize,
    pub brush_entities: usize,
    pub connections: usize,
    pub hidden_brushes: usize,
    pub hidden_entities: usize,
    pub materials: usize,
    pub entity_classes: usize,
    pub model_paths: usize,
    pub displacements: usize,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub generation: String,
    pub revision: String,
    pub totals: Totals,
    pub section: Section,
    pub offset: usize,
    pub total_rows: usize,
    pub next_offset: Option<usize>,
    pub rows: Vec<Entry>,
}

fn count(entries: &mut BTreeMap<String, usize>, name: String) {
    *entries.entry(name).or_default() += 1;
}

impl Session {
    pub fn map_statistics(
        &self,
        section: Section,
        offset: usize,
        limit: usize,
    ) -> Result<Report, String> {
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(format!("Statistics page size must be 1–{MAX_PAGE_SIZE}"));
        }
        let vmf = self.document.vmf();
        let mut totals = Totals {
            objects: vmf.world.solids.len() + vmf.entities.len(),
            world_brushes: vmf.world.solids.len(),
            ..Default::default()
        };
        let mut materials = BTreeMap::new();
        let mut classes = BTreeMap::new();
        let mut models = BTreeMap::new();
        let mut scales = BTreeMap::new();
        let mut powers = BTreeMap::new();
        for entity in &vmf.entities {
            if entity.solids.is_empty() {
                totals.point_entities += 1;
            } else {
                totals.brush_entities += 1;
                totals.entity_brushes += entity.solids.len();
            }
            totals.hidden_entities += usize::from(entity.hidden);
            totals.connections += entity.connections.as_ref().map_or(0, Vec::len);
            count(
                &mut classes,
                entity.classname().unwrap_or("<missing class>").to_owned(),
            );
            if let Some(model) = entity.properties.get("model") {
                let model = model.trim().replace('\\', "/").to_ascii_lowercase();
                if !model.is_empty() && !model.starts_with('*') {
                    count(&mut models, model);
                }
            }
        }
        let brushes = vmf
            .world
            .solids
            .iter()
            .map(|solid| (solid, solid.hidden))
            .chain(vmf.entities.iter().flat_map(|entity| {
                entity
                    .solids
                    .iter()
                    .map(move |solid| (solid, entity.hidden || solid.hidden))
            }));
        for (solid, hidden) in brushes {
            totals.hidden_brushes += usize::from(hidden);
            totals.brush_sides += solid.sides.len();
            for side in &solid.sides {
                let material = material_search::material_name(&side.material)
                    .unwrap_or_else(|_| side.material.clone());
                count(&mut materials, material);
                count(&mut scales, side.lightmap_scale.to_string());
                if let Some(displacement) = &side.displacement {
                    totals.displacements += 1;
                    count(&mut powers, displacement.power.to_string());
                }
            }
        }
        totals.materials = materials.len();
        totals.entity_classes = classes.len();
        totals.model_paths = models.len();
        let entries = match section {
            Section::Materials => materials,
            Section::EntityClasses => classes,
            Section::Models => models,
            Section::LightmapScales => scales,
            Section::DisplacementPowers => powers,
        };
        let mut rows = entries
            .into_iter()
            .map(|(name, count)| Entry { name, count })
            .collect::<Vec<_>>();
        rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        let total_rows = rows.len();
        let rows = rows
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        let end = offset.saturating_add(rows.len());
        Ok(Report {
            generation: self.document_generation().to_string(),
            revision: self.document.revision().to_string(),
            totals,
            section,
            offset,
            total_rows,
            next_offset: (end < total_rows).then_some(end),
            rows,
        })
    }
}
