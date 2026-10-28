use glam::DVec3;
use map_core::{Document, EditError, ObjectId, carve, subobject};
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};

pub struct Source {
    pub solid: u32,
    pub owner: ObjectId,
    pub mins: DVec3,
    pub maxs: DVec3,
    pub material: String,
}

pub struct Preview {
    pub document: Document,
    pub result: carve::Result,
    token: u64,
    targets: Vec<u32>,
    scope: Vec<u32>,
    generation: u64,
    revision: u64,
}

#[derive(Default)]
pub struct State {
    pub preview: Option<Preview>,
    serial: u64,
}

pub fn result_json(result: &carve::Result) -> Value {
    json!({"cutter": result.cutter, "cutters":result.cutters,"cut_material":cut_material_json(&result.cut_material),
        "changes": result.changes.iter().map(|change| json!({"source": change.source, "fragments": change.fragments})).collect::<Vec<_>>(),
        "removed": result.removed, "unchanged": result.unchanged,
        "objects": result.objects.iter().map(|id| match id {ObjectId::Solid(id) => format!("solid:{id}"), ObjectId::Entity(id) => format!("entity:{id}")}).collect::<Vec<_>>(),
        "faces": result.faces.iter().map(|face| json!({"solid": face.solid, "side": face.side, "source_solid": face.source_solid, "source_side": face.source_side, "surface_source_solid":face.surface_source_solid,"surface_source_side":face.surface_source_side,"cutter": face.cutter})).collect::<Vec<_>>()})
}

pub fn cut_material_json(policy: &carve::CutMaterialPolicy) -> Value {
    match policy {
        carve::CutMaterialPolicy::Cutter => json!({"policy":"cutter"}),
        carve::CutMaterialPolicy::Target => json!({"policy":"target"}),
        carve::CutMaterialPolicy::Material(material) => {
            json!({"policy":"material","material":material})
        }
    }
}

impl Session {
    pub fn carve_sources(&self) -> Result<Vec<Source>, String> {
        let ids = if let Some(preview) = &self.geometry.carving.preview {
            preview.scope.clone()
        } else {
            self.component_solids()
        };
        let ids = ids
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if ids.len() > carve::MAX_TARGETS + carve::MAX_CUTTERS {
            return Err("Choose at most 24 brushes: up to 8 cutters and 16 targets".into());
        }
        ids.into_iter()
            .map(|id| {
                let (owner, solid) =
                    subobject::solid(&self.document, id).map_err(|error| error.to_string())?;
                let shape = subobject::topology(solid).map_err(|error| error.to_string())?;
                let (mins, maxs) = shape
                    .vertices
                    .iter()
                    .fold((DVec3::MAX, DVec3::MIN), |(mins, maxs), vertex| {
                        (mins.min(vertex.position), maxs.max(vertex.position))
                    });
                Ok(Source {
                    solid: id,
                    owner,
                    mins,
                    maxs,
                    material: solid.sides[0].material.clone(),
                })
            })
            .collect()
    }

    pub fn carve_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.scene_serial,
            self.geometry.carving.serial
        )
    }

    pub fn carve_state(&self) -> Value {
        json!({"handle":self.carve_handle(),"stamp":self.carve_stamp(),"preview": self.geometry.carving.preview.as_ref().map(|preview| json!({"token":preview.token.to_string(),"generation":preview.generation,"revision":preview.revision,"targets":preview.targets,"result":result_json(&preview.result)})),
            "limits":{"cutters":carve::MAX_CUTTERS,"targets":carve::MAX_TARGETS,"fragments":carve::MAX_FRAGMENTS,"sides_per_brush":subobject::MAX_SIDES},
            "cut_material_policies":{"cutter":"Default. Copy each cutter face's material, world UV projection, lighting and unknown metadata.","target":"Copy the unique original target face with the same outward normal as each generated face; missing or ambiguous parallel faces reject. Preserve that face's world UV projection, lighting and unknown metadata.","material":"Replace only the cutter face's material; keep its world UV projection, lighting and unknown metadata. Nodraw remains allowed."},
            "full_removal_policy":"World brushes may be removed. Empty brush entities reject; delete those owners explicitly."})
    }

    pub(super) fn carve_handle(&self) -> Value {
        Self::edit_handle(
            "carve",
            self.geometry.carving.preview.as_ref().and_then(|preview| {
                self.checked_carve(preview.token)
                    .is_ok()
                    .then_some(preview.token)
            }),
        )
    }

    pub fn carve_report(&self) -> Value {
        let mut report = self.carve_state();
        match self.carve_sources() {
            Ok(sources) => report["sources"] = json!(sources.iter().map(|source| json!({"solid":source.solid,"owner":match source.owner {ObjectId::Solid(id)=>format!("solid:{id}"),ObjectId::Entity(id)=>format!("entity:{id}")},"mins":source.mins.to_array(),"maxs":source.maxs.to_array(),"material":source.material})).collect::<Vec<_>>()),
            Err(error) => report["source_error"] = json!(error),
        }
        report
    }

    pub fn clear_carve_preview(&mut self) {
        if self.geometry.carving.preview.take().is_some() {
            self.geometry.carving.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn preview_carve(
        &mut self,
        cutter: u32,
        targets: &[u32],
        stamp: &str,
    ) -> Result<(), String> {
        self.preview_carve_many(&[cutter], targets, stamp)
    }

    pub fn preview_carve_many(
        &mut self,
        cutters: &[u32],
        targets: &[u32],
        stamp: &str,
    ) -> Result<(), String> {
        self.preview_carve_many_with_material(
            cutters,
            targets,
            &carve::CutMaterialPolicy::Cutter,
            stamp,
        )
    }

    pub fn preview_carve_with_material(
        &mut self,
        cutter: u32,
        targets: &[u32],
        cut_material: &carve::CutMaterialPolicy,
        stamp: &str,
    ) -> Result<(), String> {
        self.preview_carve_many_with_material(&[cutter], targets, cut_material, stamp)
    }

    pub fn preview_carve_many_with_material(
        &mut self,
        cutters: &[u32],
        targets: &[u32],
        cut_material: &carve::CutMaterialPolicy,
        stamp: &str,
    ) -> Result<(), String> {
        if stamp != self.carve_stamp() {
            return Err(
                "Map, selection or preview changed; inspect the carving state again".into(),
            );
        }
        let mut document = self.document.preview_snapshot();
        let result = document
            .edit("preview carve", |tx| {
                carve::carve_many_with_material(tx, cutters, targets, cut_material)
            })
            .map_err(|error| error.to_string())?;
        let requested = targets
            .iter()
            .copied()
            .chain(cutters.iter().copied())
            .collect::<std::collections::BTreeSet<_>>();
        let selected = self.component_solids();
        let scope = if selected.len() <= carve::MAX_TARGETS + carve::MAX_CUTTERS
            && requested.iter().all(|id| selected.contains(id))
        {
            selected
        } else {
            requested.into_iter().collect()
        };
        let owners = scope
            .iter()
            .copied()
            .map(|id| subobject::solid(&self.document, id).map(|(owner, _)| owner))
            .collect::<Result<std::collections::BTreeSet<_>, _>>()
            .map_err(|error| error.to_string())?;
        self.select(&owners.into_iter().collect::<Vec<_>>(), SelectMode::Replace)?;
        self.cancel_block_preview();
        self.clear_geometry_previews_except(&[]);

        self.cancel_area_selection();
        self.clear_volume_selection();

        self.clip_tool = false;
        self.geometry.paint.enabled = false;
        self.spatial.draw_block = false;
        self.geometry.carving.serial += 1;
        self.geometry.carving.preview = Some(Preview {
            document,
            result,
            token: self.geometry.carving.serial,
            targets: targets
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            scope,
            generation: self.document_generation,
            revision: self.document.revision(),
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    fn checked_carve(&self, token: u64) -> Result<&Preview, String> {
        let preview = self
            .geometry
            .carving
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Carve preview was closed or replaced")?;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
        {
            return Err("Map changed; preview the carve again".into());
        }
        Ok(preview)
    }

    pub fn commit_carve(&mut self, actor: Actor, token: u64) -> Result<carve::Result, String> {
        let preview = self.checked_carve(token)?;
        let expected = preview.result.clone();
        let targets = preview.targets.clone();
        let solids = expected
            .changes
            .iter()
            .flat_map(|change| &change.fragments)
            .map(|id| subobject::solid(&preview.document, *id).map(|(_, solid)| solid.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let result = self.edit(actor, "carve brushes", |tx| {
            let result = carve::carve_many_with_material(
                tx,
                &expected.cutters,
                &targets,
                &expected.cut_material,
            )?;
            if result != expected
                || solids.iter().any(|solid| {
                    subobject::solid(tx.document(), solid.id)
                        .map_or(true, |(_, current)| current != solid)
                })
            {
                return Err(EditError::Invalid(
                    "Brush or face allocation changed; preview the carve again".into(),
                ));
            }
            Ok(result)
        })?;
        self.select(&result.objects, SelectMode::Replace)?;
        Ok(result)
    }

    pub fn cancel_carve(&mut self, token: u64) -> Result<(), String> {
        self.checked_carve(token)?;
        self.clear_carve_preview();
        Ok(())
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.preview
            .as_ref()
            .map(|preview| serde_json::json!([self.serial, preview.token]))
    }
}
