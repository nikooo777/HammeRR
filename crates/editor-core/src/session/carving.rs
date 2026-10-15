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
    json!({"cutter": result.cutter, "cutters":result.cutters,
        "changes": result.changes.iter().map(|change| json!({"source": change.source, "fragments": change.fragments})).collect::<Vec<_>>(),
        "removed": result.removed, "unchanged": result.unchanged,
        "objects": result.objects.iter().map(|id| match id {ObjectId::Solid(id) => format!("solid:{id}"), ObjectId::Entity(id) => format!("entity:{id}")}).collect::<Vec<_>>(),
        "faces": result.faces.iter().map(|face| json!({"solid": face.solid, "side": face.side, "source_solid": face.source_solid, "source_side": face.source_side, "cutter": face.cutter})).collect::<Vec<_>>()})
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
        json!({"stamp":self.carve_stamp(),"preview": self.geometry.carving.preview.as_ref().map(|preview| json!({"token":preview.token.to_string(),"generation":preview.generation,"revision":preview.revision,"targets":preview.targets,"result":result_json(&preview.result)})),
            "limits":{"cutters":carve::MAX_CUTTERS,"targets":carve::MAX_TARGETS,"fragments":carve::MAX_FRAGMENTS,"sides_per_brush":subobject::MAX_SIDES},
            "full_removal_policy":"World brushes may be removed. Empty brush entities reject; delete those owners explicitly."})
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
        if stamp != self.carve_stamp() {
            return Err(
                "Map, selection or preview changed; inspect the carving state again".into(),
            );
        }
        let mut document = self.document.preview_snapshot();
        let result = document
            .edit("preview carve", |tx| {
                carve::carve_many(tx, cutters, targets)
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
            let result = carve::carve_many(tx, &expected.cutters, &targets)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Session, u32, u32) {
        let mut session = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(target) = session
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(128.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let ObjectId::Solid(cutter) = session
            .create_block(
                Actor::Agent,
                DVec3::splat(32.0),
                DVec3::splat(96.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        (session, target, cutter)
    }

    #[test]
    fn cutter_batches_share_actor_preview_guards_and_exact_apply_undo() {
        for actor in [Actor::User, Actor::Agent] {
            let (mut session, target, first) = fixture();
            let ObjectId::Solid(second) = session
                .create_block(
                    Actor::Agent,
                    DVec3::splat(48.0),
                    DVec3::splat(112.0),
                    "brick/second",
                    false,
                )
                .unwrap()
            else {
                unreachable!()
            };
            let source = session.document().vmf().clone();
            let stamp = session.carve_stamp();
            session
                .preview_carve_many(&[second, first], &[target], &stamp)
                .unwrap();
            let token = session.geometry.carving.preview.as_ref().unwrap().token;
            let draft = session.render_document().vmf().clone();
            let report = session.carve_report();
            assert_eq!(
                report["preview"]["result"]["cutters"],
                json!([first, second])
            );
            assert_eq!(session.document().vmf(), &source);
            assert!(
                session
                    .preview_carve_many(&[first, second], &[target], &stamp)
                    .is_err()
            );
            assert!(
                session
                    .preview_carve_many(&[first, first], &[target], &session.carve_stamp())
                    .is_err()
            );
            assert_eq!(
                session.geometry.carving.preview.as_ref().unwrap().token,
                token
            );
            session.cancel_carve(token).unwrap();
            assert_eq!(session.render_document().vmf(), &source);
            session
                .preview_carve_many(&[first, second], &[target], &session.carve_stamp())
                .unwrap();
            let current = session.geometry.carving.preview.as_ref().unwrap().token;
            assert!(session.commit_carve(actor, token).is_err());
            session.commit_carve(actor, current).unwrap();
            assert_eq!(session.document().vmf(), &draft);
            session.undo(actor).unwrap();
            assert_eq!(session.document().vmf(), &source);
        }
    }

    #[test]
    fn carving_shares_preview_cancel_apply_undo_and_exact_allocations() {
        for actor in [Actor::User, Actor::Agent] {
            let (mut session, target, cutter) = fixture();
            let source = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let stamp = session.carve_stamp();
            session.preview_carve(cutter, &[target], &stamp).unwrap();
            let token = session.geometry.carving.preview.as_ref().unwrap().token;
            let draft = session.render_document().vmf().clone();
            let fragments = session
                .geometry
                .carving
                .preview
                .as_ref()
                .unwrap()
                .result
                .changes[0]
                .fragments
                .clone();
            assert_eq!(session.component_topology(&fragments).unwrap().len(), 6);
            assert_eq!(session.document.vmf(), &source);
            assert_eq!(session.document.undo_labels().count(), history);
            assert!(session.preview_carve(cutter, &[target], &stamp).is_err());
            assert!(
                session
                    .preview_carve(cutter, &[cutter], &session.carve_stamp())
                    .is_err()
            );
            assert_eq!(session.render_document().vmf(), &draft);
            session.cancel_carve(token).unwrap();
            assert_eq!(session.render_document().vmf(), &source);
            session
                .preview_carve(cutter, &[target], &session.carve_stamp())
                .unwrap();
            let newer = session.geometry.carving.preview.as_ref().unwrap().token;
            assert_eq!(session.render_document().vmf(), &draft);
            assert!(session.cancel_carve(token).is_err());
            assert!(session.commit_carve(actor, token).is_err());
            let result = session.commit_carve(actor, newer).unwrap();
            assert_eq!(session.document.vmf(), &draft);
            assert_eq!(session.document.undo_labels().count(), history + 1);
            assert_eq!(session.selected_ids(), result.objects);
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &source);
            session.redo(actor).unwrap();
            assert_eq!(session.document.vmf(), &draft);
        }
    }

    #[test]
    fn carve_allocation_changes_and_other_tools_cannot_apply_stale_geometry() {
        let (mut session, target, cutter) = fixture();
        session
            .preview_carve(cutter, &[target], &session.carve_stamp())
            .unwrap();
        let token = session.geometry.carving.preview.as_ref().unwrap().token;
        let source = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        session
            .document
            .edit("reserve", |tx| tx.new_side_id())
            .unwrap();
        assert!(
            session
                .commit_carve(Actor::Agent, token)
                .unwrap_err()
                .contains("allocation")
        );
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.undo_labels().count(), history);
        assert_eq!(
            session.geometry.carving.preview.as_ref().unwrap().token,
            token
        );
        session
            .preview_hollow(
                &[ObjectId::Solid(target), ObjectId::Solid(cutter)],
                map_core::hollow::Spec {
                    thickness: 16.0,
                    outward: false,
                    open_faces: Default::default(),
                },
                &session.hollow_stamp(),
            )
            .unwrap();
        assert!(session.geometry.carving.preview.is_none());
        assert!(session.commit_carve(Actor::Agent, token).is_err());
        session
            .preview_carve(cutter, &[target], &session.carve_stamp())
            .unwrap();
        assert!(session.geometry.hollowing.preview.is_none());
        let token = session.geometry.carving.preview.as_ref().unwrap().token;
        session
            .create_block(
                Actor::Agent,
                DVec3::splat(256.0),
                DVec3::splat(288.0),
                "brick/test",
                false,
            )
            .unwrap();
        assert!(session.commit_carve(Actor::User, token).is_err());
    }

    #[test]
    fn explicit_carve_sources_stay_scoped_inside_a_large_brush_entity() {
        let (mut session, target, cutter) = fixture();
        let mut ids = vec![target];
        for index in 0..carve::MAX_TARGETS + carve::MAX_CUTTERS {
            let ObjectId::Solid(id) = session
                .create_block(
                    Actor::Agent,
                    DVec3::new(256.0 + index as f64 * 64.0, 0.0, 0.0),
                    DVec3::new(288.0 + index as f64 * 64.0, 32.0, 32.0),
                    "brick/test",
                    false,
                )
                .unwrap()
            else {
                unreachable!()
            };
            ids.push(id);
        }
        let owner = session
            .edit(Actor::Agent, "tie", |tx| {
                map_core::edit::tie_to_entity(tx, &ids, "func_detail", &[])
            })
            .unwrap();
        session
            .preview_carve(cutter, &[target], &session.carve_stamp())
            .unwrap();
        let report = session.carve_report();
        assert_eq!(report["sources"].as_array().unwrap().len(), 2);
        assert!(report.get("source_error").is_none());
        let token = session.geometry.carving.preview.as_ref().unwrap().token;
        session.commit_carve(Actor::Agent, token).unwrap();
        let ObjectId::Entity(owner) = owner else {
            unreachable!()
        };
        assert_eq!(
            session.document.entity(owner).unwrap().solids.len(),
            ids.len() + 5
        );
        session.document.vmf().validate_ids().unwrap();
    }
}
