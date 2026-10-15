use glam::DVec3;
use map_core::{Document, EditError, ObjectId, merge, subobject};
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};
use super::carving::Source;

pub struct Preview {
    pub document: Document,
    pub result: merge::Result,
    token: u64,
    scope: Vec<u32>,
    generation: u64,
    revision: u64,
}

#[derive(Default)]
pub struct State {
    pub preview: Option<Preview>,
    discarded_scope: Option<Vec<u32>>,
    serial: u64,
}

pub fn result_json(result: &merge::Result) -> Value {
    json!({"sources": result.sources, "solid": result.solid,
        "object": object_id(result.object), "removed": result.removed,
        "faces": result.faces.iter().map(|face| json!({"side":face.side,"source_solid":face.source_solid,"source_side":face.source_side})).collect::<Vec<_>>()})
}

fn object_id(id: ObjectId) -> String {
    match id {
        ObjectId::Solid(id) => format!("solid:{id}"),
        ObjectId::Entity(id) => format!("entity:{id}"),
    }
}

impl Session {
    fn merge_source_ids(&self) -> Vec<u32> {
        self.merging
            .preview
            .as_ref()
            .map(|preview| &preview.scope)
            .or(self.merging.discarded_scope.as_ref())
            .cloned()
            .unwrap_or_else(|| self.component_solids())
    }

    pub fn merge_sources(&self) -> Result<Vec<Source>, String> {
        let ids = self.merge_source_ids();
        if ids.len() > merge::MAX_BRUSHES {
            return Err("Choose at most 16 source brushes to merge".into());
        }
        ids.into_iter().map(|id| {
            let (owner, solid) = subobject::solid(&self.document, id).map_err(|error| error.to_string())?;
            let shape = subobject::topology(solid).map_err(|error| error.to_string())?;
            let (mins, maxs) = shape.vertices.iter().fold((DVec3::MAX, DVec3::MIN), |(mins, maxs), vertex| (mins.min(vertex.position), maxs.max(vertex.position)));
            Ok(Source { solid:id, owner, mins, maxs, material:solid.sides[0].material.clone() })
        }).collect()
    }

    pub fn merge_stamp(&self) -> String {
        format!("{}:{}:{}:{}",self.document_generation,self.document.revision(),self.scene_serial,self.merging.serial)
    }

    pub fn merge_report(&self) -> Value {
        let mut report = json!({"stamp":self.merge_stamp(),"preview":self.merging.preview.as_ref().map(|preview|json!({"token":preview.token.to_string(),"generation":preview.generation,"revision":preview.revision,"result":result_json(&preview.result)})),
            "limits":{"brushes":merge::MAX_BRUSHES,"proof_fragments":merge::MAX_PROOF_FRAGMENTS,"sides_per_brush":subobject::MAX_SIDES},
            "inheritance_policy":"Lowest brush ID supplies solid/editor metadata. Each coplanar outer face inherits the lowest brush ID, then lowest side ID, with unchanged world texture projection.",
            "owner_policy":"World brushes together, or children of one brush entity. Mixed owners and displacements reject."});
        match self.merge_sources() {
            Ok(sources) => report["sources"] = json!(sources.iter().map(|source|json!({"solid":source.solid,"owner":object_id(source.owner),"mins":source.mins.to_array(),"maxs":source.maxs.to_array(),"material":source.material})).collect::<Vec<_>>()),
            Err(error) => report["source_error"] = json!(error),
        }
        report
    }

    pub fn clear_merge_preview(&mut self) {
        let preview = self.merging.preview.take();
        let scope = self.merging.discarded_scope.take();
        if preview.is_some() || scope.is_some() {
            self.merging.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn preview_merge(&mut self, sources: &[u32], stamp: &str) -> Result<(), String> {
        if stamp != self.merge_stamp() {
            return Err("Map, selection or preview changed; inspect the merge state again".into());
        }
        let mut document = self.document.preview_snapshot();
        let result = document.edit("preview merge", |tx| merge::merge(tx, sources)).map_err(|error| error.to_string())?;
        let selected = self.merge_source_ids();
        let scope = if selected.len() <= merge::MAX_BRUSHES && sources.iter().all(|id| selected.contains(id)) {
            selected
        } else {
            result.sources.clone()
        };
        let owners = scope.iter().map(|id|subobject::solid(&self.document,*id).map(|(owner,_)|owner)).collect::<Result<std::collections::BTreeSet<_>,_>>().map_err(|error|error.to_string())?;
        self.select(&owners.into_iter().collect::<Vec<_>>(), SelectMode::Replace)?;
        self.cancel_block_preview();
        self.cancel_clip_preview();
        self.cancel_transform_preview();
        self.cancel_area_selection();
        self.clear_volume_selection();
        self.cancel_face_preview();
        self.clear_component_edit();
        self.clear_primitive_preview();
        self.clear_paint_stroke();
        self.clear_hollow_preview();
        self.clear_carve_preview();
        self.clear_repeat_preview();
        self.clear_merge_preview();
        self.clip_tool = false;
        self.paint.enabled = false;
        self.spatial.draw_block = false;
        self.merging.serial += 1;
        self.merging.preview = Some(Preview {
            document, result, scope, token:self.merging.serial,
            generation:self.document_generation,revision:self.document.revision(),
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    fn checked_merge(&self, token:u64) -> Result<&Preview,String> {
        let preview = self.merging.preview.as_ref().filter(|preview|preview.token==token).ok_or("Merge preview was closed or replaced")?;
        if preview.generation != self.document_generation || preview.revision != self.document.revision() {
            return Err("Map changed; preview the merge again".into());
        }
        Ok(preview)
    }

    pub fn commit_merge(&mut self, actor:Actor, token:u64) -> Result<merge::Result,String> {
        let preview = self.checked_merge(token)?;
        let expected = preview.result.clone();
        let solid = subobject::solid(&preview.document,expected.solid).map_err(|error|error.to_string())?.1.clone();
        let result = self.edit(actor,"merge brushes",|tx| {
            let result = merge::merge(tx,&expected.sources)?;
            if result != expected || subobject::solid(tx.document(),result.solid)?.1 != &solid {
                return Err(EditError::Invalid("Merged geometry or face inheritance changed; preview the merge again".into()));
            }
            Ok(result)
        })?;
        self.select(&[result.object],SelectMode::Replace)?;
        Ok(result)
    }

    pub fn cancel_merge(&mut self, token:u64) -> Result<(),String> {
        let scope = self.checked_merge(token)?.scope.clone();
        self.clear_merge_preview();
        self.merging.discarded_scope = Some(scope);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Session,u32,u32) {
        let mut session = Session::new(Err("no assets needed".into()));
        let mut ids = Vec::new();
        for (mins,maxs) in [
            (DVec3::ZERO,DVec3::splat(64.0)),
            (DVec3::new(64.0,0.0,0.0),DVec3::new(128.0,64.0,64.0)),
        ] {
            let id = session.create_block(Actor::Agent,mins,maxs,"brick/test",false).unwrap();
            let ObjectId::Solid(id) = id else {unreachable!()};
            ids.push(id);
        }
        session.select(&ids.iter().map(|id|ObjectId::Solid(*id)).collect::<Vec<_>>(),SelectMode::Replace).unwrap();
        (session,ids[0],ids[1])
    }

    fn token(session:&Session) -> u64 {
        session.merging.preview.as_ref().unwrap().token
    }

    #[test]
    fn merge_shares_actor_preview_cancel_exact_apply_and_one_step_undo() {
        for actor in [Actor::User,Actor::Agent] {
            let (mut session,first,second) = fixture();
            let original = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let stamp = session.merge_stamp();
            session.preview_merge(&[second,first],&stamp).unwrap();
            let first_token = token(&session);
            let result = session.merging.preview.as_ref().unwrap().result.clone();
            let draft = session.render_document().vmf().clone();
            assert_eq!(draft.world.solids.len(),1);
            assert_eq!(session.document.vmf(),&original);
            assert_eq!(session.document.undo_labels().count(),history);
            assert!(session.transform_guide().is_none());
            assert_eq!(session.build_scene().stats.brushes,1);
            assert_eq!(session.merge_sources().unwrap().len(),2);
            assert_eq!(subobject::topology(subobject::solid(session.render_document(),first).unwrap().1).unwrap().faces.len(),6);
            assert!(session.preview_merge(&[first,second],&stamp).is_err());
            assert!(session.preview_merge(&[first,first],&session.merge_stamp()).is_err());
            assert_eq!(token(&session),first_token);
            session.cancel_merge(first_token).unwrap();
            assert_eq!(session.render_document().vmf(),&original);
            assert_eq!(session.build_scene().stats.brushes,2);
            assert_eq!(session.document.undo_labels().count(),history);
            session.preview_merge(&[first,second],&session.merge_stamp()).unwrap();
            assert_ne!(token(&session),first_token);
            assert_eq!(session.merging.preview.as_ref().unwrap().result,result);
            assert_eq!(session.render_document().vmf(),&draft);
            assert!(session.commit_merge(actor,first_token).is_err());
            assert!(session.cancel_merge(first_token).is_err());
            session.commit_merge(actor,token(&session)).unwrap();
            assert_eq!(session.document.vmf(),&draft);
            assert_eq!(session.document.undo_labels().count(),history+1);
            assert!(session.merging.preview.is_none());
            assert_eq!(session.selected_ids(),vec![ObjectId::Solid(first)]);
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(),&original);
            session.redo(actor).unwrap();
            assert_eq!(session.document.vmf(),&draft);
        }
    }

    #[test]
    fn primitive_numeric_and_source_edits_invalidate_merge_without_foreign_cancellation() {
        let (mut session,first,second) = fixture();
        session.preview_merge(&[first,second],&session.merge_stamp()).unwrap();
        let merge_token = token(&session);
        session.create_primitive(Actor::Agent,map_core::primitive::Spec {
            mins:DVec3::splat(256.0),maxs:DVec3::splat(320.0),..Default::default()
        },true).unwrap();
        let primitive_token = session.primitive_preview().unwrap().token;
        assert!(session.merging.preview.is_none());
        assert!(session.cancel_merge(merge_token).is_err());
        assert_eq!(session.primitive_preview().unwrap().token,primitive_token);
        assert_eq!(session.build_scene().stats.brushes,3);
        session.preview_merge(&[first,second],&session.merge_stamp()).unwrap();
        assert!(session.primitive_preview().is_none());
        let merge_token = token(&session);
        session.preview_transform(Actor::Agent,&[ObjectId::Solid(first),ObjectId::Solid(second)],map_core::transform::Transform {translation:DVec3::X*16.0,..Default::default()},None).unwrap();
        assert!(session.merging.preview.is_none());
        assert!(session.cancel_merge(merge_token).is_err());
        assert!(session.transform_preview().is_some());
        session.preview_merge(&[first,second],&session.merge_stamp()).unwrap();
        assert!(session.transform_preview().is_none());
        let merge_token = token(&session);
        session.edit(Actor::Agent,"surface edit",|tx| {
            tx.solid_mut(first)?.sides[0].material="CHANGED/MATERIAL".into();
            Ok(())
        }).unwrap();
        assert!(session.merging.preview.is_none());
        assert!(session.commit_merge(Actor::Agent,merge_token).is_err());
    }

    #[test]
    fn selection_generation_and_invalid_geometry_keep_tokens_scoped() {
        let (mut session,first,second) = fixture();
        session.preview_merge(&[first,second],&session.merge_stamp()).unwrap();
        let merge_token = token(&session);
        session.document_generation += 1;
        assert!(session.commit_merge(Actor::Agent,merge_token).is_err());
        assert!(session.cancel_merge(merge_token).is_err());
        session.document_generation -= 1;
        session.select(&[ObjectId::Solid(first)],SelectMode::Replace).unwrap();
        assert!(session.merging.preview.is_none());
        assert!(session.cancel_merge(merge_token).is_err());
        session.preview_merge(&[first,second],&session.merge_stamp()).unwrap();
        let merge_token = token(&session);
        let original = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        assert!(session.preview_merge(&[first,u32::MAX],&session.merge_stamp()).is_err());
        assert_eq!(token(&session),merge_token);
        assert_eq!(session.document.vmf(),&original);
        assert_eq!(session.document.undo_labels().count(),history);
    }

    #[test]
    fn entity_subset_stays_scoped_when_owner_has_more_than_sixteen_brushes() {
        let (mut session,first,second) = fixture();
        let mut sources = vec![first,second];
        for index in 0..18 {
            let x = 256.0+f64::from(index)*64.0;
            let id = session.create_block(Actor::Agent,DVec3::new(x,0.0,0.0),DVec3::new(x+32.0,32.0,32.0),"brick/test",false).unwrap();
            let ObjectId::Solid(id) = id else {unreachable!()};
            sources.push(id);
        }
        let owner = session.edit(Actor::Agent,"tie",|tx|map_core::edit::tie_to_entity(tx,&sources,"func_detail",&[])).unwrap();
        session.select(&[owner],SelectMode::Replace).unwrap();
        assert!(session.merge_sources().is_err());
        let candidates = sources[..3].to_vec();
        session.set_component_picking(Some(super::super::component_picking::Kind::Face),&candidates,&session.component_stamp()).unwrap();
        session.preview_merge(&[first,second],&session.merge_stamp()).unwrap();
        assert_eq!(session.merge_sources().unwrap().len(),3);
        let discarded_token = token(&session);
        session.cancel_merge(discarded_token).unwrap();
        assert_eq!(session.merge_sources().unwrap().iter().map(|source|source.solid).collect::<Vec<_>>(),candidates);
        assert!(session.cancel_merge(discarded_token).is_err());
        session.preview_merge(&[first,second],&session.merge_stamp()).unwrap();
        session.preview_merge(&[first,second],&session.merge_stamp()).unwrap();
        assert_eq!(session.merge_sources().unwrap().len(),3);
        let merge_token = token(&session);
        session.commit_merge(Actor::Agent,merge_token).unwrap();
        let ObjectId::Entity(entity) = owner else {unreachable!()};
        assert_eq!(session.document.entity(entity).unwrap().solids.len(),19);
        assert_eq!(session.selected_ids(),vec![owner]);
        assert!(session.merge_sources().is_err());
    }
}
