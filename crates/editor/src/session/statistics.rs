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

#[cfg(test)]
mod tests {
    use super::*;
    use source_formats::vmf::Entity;

    #[test]
    fn counts_authored_data_including_nested_hidden_brushes_without_drafts() {
        let mut session = Session::new(Err("no assets".into()));
        let ObjectId::Solid(id) = session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "Materials/BRICK/test.vmt",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let mut vmf = session.document.vmf().clone();
        let mut entity = Entity::new(100, "func_detail");
        entity.hidden = true;
        let mut solid = vmf.world.solids[0].clone();
        solid.id = 101;
        for (index, side) in solid.sides.iter_mut().enumerate() {
            side.id = 102 + index as u32;
            side.material = "brick/test".into();
        }
        solid.sides[0].displacement = Some(source_formats::vmf::DispInfo {
            power: 4,
            start_position: DVec3::ZERO,
            flags: None,
            elevation: 0.0,
            subdivided: false,
            normals: None,
            distances: None,
            offsets: None,
            offset_normals: None,
            alphas: None,
            triangle_tags: None,
            allowed_verts: None,
            extra_properties: Default::default(),
            extra: Vec::new(),
        });
        entity.connections = Some(vec![source_formats::vmf::Connection {
            output: "OnUser1".into(),
            value: "!self,FireUser2,,0,-1".into(),
        }]);
        entity.solids.push(solid);
        let mut prop = Entity::new(200, "prop_static");
        prop.properties.set("model", "Models\\Props/test.mdl");
        vmf.entities.extend([entity, prop]);
        session.document = Document::new(vmf);
        let before = session.document.vmf().clone();
        session
            .edit_faces(
                Actor::Agent,
                &[FaceId {
                    solid: id,
                    side: before.world.solids[0].sides[0].id,
                }],
                SurfaceEdit {
                    material: Some("draft/test".into()),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
        let report = session.map_statistics(Section::Materials, 0, 50).unwrap();
        assert_eq!(
            report.totals,
            Totals {
                objects: 3,
                world_brushes: 1,
                entity_brushes: 1,
                brush_sides: 12,
                point_entities: 1,
                brush_entities: 1,
                hidden_brushes: 1,
                hidden_entities: 1,
                materials: 1,
                entity_classes: 2,
                model_paths: 1,
                displacements: 1,
                connections: 1,
            }
        );
        assert_eq!(
            report.rows,
            vec![Entry {
                name: "brick/test".into(),
                count: 12
            }]
        );
        let models = session.map_statistics(Section::Models, 0, 1).unwrap();
        assert_eq!(models.rows[0].name, "models/props/test.mdl");
        assert_eq!(
            session
                .map_statistics(Section::DisplacementPowers, 0, 50)
                .unwrap()
                .rows,
            vec![Entry {
                name: "4".into(),
                count: 1
            }]
        );
        assert_eq!(session.document.vmf(), &before);
        assert!(session.face_preview.is_some());
        assert_eq!(session.document.undo_labels().count(), 0);
    }

    #[test]
    fn pages_are_stable_bounded_and_empty_after_map_replacement() {
        let mut session = Session::new(Err("no assets".into()));
        for (index, material) in ["b", "a", "b", "c"].iter().enumerate() {
            let min = DVec3::X * index as f64 * 128.0;
            session
                .create_block(Actor::Agent, min, min + DVec3::splat(64.0), material, false)
                .unwrap();
        }
        let first = session.map_statistics(Section::Materials, 0, 1).unwrap();
        assert_eq!(
            first.rows[0],
            Entry {
                name: "b".into(),
                count: 12
            }
        );
        assert_eq!(first.next_offset, Some(1));
        let last = session.map_statistics(Section::Materials, 1, 2).unwrap();
        assert_eq!(
            last.rows
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            ["a", "c"]
        );
        assert_eq!(last.next_offset, None);
        assert!(
            session
                .map_statistics(Section::Materials, usize::MAX, 1)
                .unwrap()
                .rows
                .is_empty()
        );
        assert!(session.map_statistics(Section::Materials, 0, 0).is_err());
        assert!(
            session
                .map_statistics(Section::Materials, 0, MAX_PAGE_SIZE + 1)
                .is_err()
        );
        session.new_map(Actor::User, true).unwrap();
        let empty = session.map_statistics(Section::Materials, 0, 50).unwrap();
        assert_eq!(empty.totals, Totals::default());
        assert_ne!(empty.generation, first.generation);
    }
}
