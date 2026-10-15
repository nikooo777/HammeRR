use map_core::{Document, EditError, ObjectId, repeat};
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};

pub struct Preview {
    pub document: Document,
    pub result: repeat::Result,
    token: u64,
    sources: Vec<ObjectId>,
    options: repeat::Options,
    generation: u64,
    revision: u64,
    definitions: u64,
}

#[derive(Default)]
pub struct State {
    serial: u64,
    pub preview: Option<Preview>,
}

fn id_string(id: ObjectId) -> String {
    match id {
        ObjectId::Solid(id) => format!("solid:{id}"),
        ObjectId::Entity(id) => format!("entity:{id}"),
    }
}

impl Session {
    pub fn repeat_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.definitions_serial,
            self.scene_serial,
            self.geometry.repeat.serial
        )
    }

    pub fn repeat_array(&self) -> Value {
        let preview = self.geometry.repeat.preview.as_ref().map(|preview| {
            let objects = preview.result.objects();
            json!({
                "token": preview.token,
                "sources": preview.sources.iter().copied().map(id_string).collect::<Vec<_>>(),
                "count": preview.options.count,
                "translation": preview.options.translation.to_array(),
                "rotation": preview.options.rotation.to_array(),
                "pivot": preview.options.pivot.to_array(),
                "texture_lock": preview.options.texture_lock,
                "bounds": self.object_bounds_in(&preview.document, &objects).map(|(mins, maxs)| [mins.to_array(), maxs.to_array()]),
                "copies": preview.result.copies.iter().map(|copy| json!({
                    "index": copy.index,
                    "objects": copy.objects.iter().copied().map(id_string).collect::<Vec<_>>(),
                    "names": copy.names.iter().map(|name| json!({"source":id_string(ObjectId::Entity(name.source)), "copy":id_string(ObjectId::Entity(name.copy)), "before":name.before, "after":name.after})).collect::<Vec<_>>(),
                    "remapped_references": copy.remapped_references,
                })).collect::<Vec<_>>(),
                "entities": objects.iter().filter_map(|id| {
                    let ObjectId::Entity(id) = id else { return None };
                    let entity = preview.document.entity(*id)?;
                    Some(json!({"id":id_string(ObjectId::Entity(*id)),"origin":entity.origin().map(|origin| origin.to_array()),"properties":entity.properties.0,"connections":entity.connections.iter().flatten().map(|connection|json!({"output":connection.output,"value":connection.value})).collect::<Vec<_>>()}))
                }).collect::<Vec<_>>(),
                "object_count": objects.len(),
                "valid": preview.generation == self.document_generation && preview.revision == self.document.revision() && preview.definitions == self.definitions_serial,
            })
        });
        json!({
            "snapshot": self.repeat_stamp(),
            "preview": preview,
            "limits": {"sources":repeat::MAX_SOURCES,"copies":repeat::MAX_COPIES,"objects_including_owned_brushes":repeat::MAX_OBJECTS,"sides":repeat::MAX_SIDES,"name_bytes":repeat::MAX_NAME_BYTES},
            "transform_policy":"Copy n uses the original objects, n × world XYZ rotation about one fixed pivot, then n × world translation. Count excludes originals.",
            "reference_policy":"Unique source targetnames become name_copyN, with a collision suffix when needed. Certain exact internal FGD/I/O/parent references follow that copy; external links and procedural references remain unchanged. Shared names, ambiguous internal matches, wildcard captures of new names, overlay face-ID references and node IDs reject. Named source sets with scripts or dynamic I/O also reject.",
            "metadata_policy":"Object and owned-brush context notes copy with their existing authorship/observations and full map budgets. Saved partitions remain attached to originals. Other opaque custom metadata and embedded text stay verbatim; their semantics are not inferred or remapped.",
        })
    }

    pub fn clear_repeat_preview(&mut self) {
        if self.geometry.repeat.preview.take().is_some() {
            self.geometry.repeat.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    fn checked_repeat(&self, token: u64) -> Result<&Preview, String> {
        let preview = self
            .geometry
            .repeat
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Repeat array was closed or replaced")?;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
            || preview.definitions != self.definitions_serial
        {
            return Err("Map or entity definitions changed; preview the array again".into());
        }
        Ok(preview)
    }

    pub fn preview_repeat_array(
        &mut self,
        sources: &[ObjectId],
        options: repeat::Options,
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.repeat_stamp() {
            return Err("Map, selection or preview changed; inspect the repeat array again".into());
        }
        self.ensure_partitions_resolved()?;
        let mut document = self.document.preview_snapshot();
        let result = document
            .edit("preview repeat array", |tx| {
                repeat::create(tx, sources, options, &self.definitions)
            })
            .map_err(|error| error.to_string())?;
        self.select(sources, SelectMode::Replace)?;
        self.cancel_block_preview();
        self.clear_geometry_previews_except(&[]);

        self.cancel_area_selection();
        self.clear_volume_selection();

        self.clip_tool = false;
        self.geometry.paint.enabled = false;
        self.spatial.draw_block = false;
        self.geometry.repeat.serial += 1;
        self.geometry.repeat.preview = Some(Preview {
            document,
            result,
            token: self.geometry.repeat.serial,
            sources: sources.to_vec(),
            options,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(self.repeat_array())
    }

    pub fn apply_repeat_array(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let preview = self.checked_repeat(token)?;
        let expected = preview.document.vmf().clone();
        let sources = preview.sources.clone();
        let options = preview.options;
        let expected_result = preview.result.clone();
        let definitions = self.definitions.clone();
        let result = self.edit(actor, "repeat array", |tx| {
            let result = repeat::create(tx, &sources, options, &definitions)?;
            if result != expected_result || tx.document().vmf() != &expected {
                return Err(EditError::Invalid(
                    "Array differs from its preview; preview the array again".into(),
                ));
            }
            Ok(result)
        })?;
        let copies = result.objects();
        self.select(&copies, SelectMode::Replace)?;
        Ok(
            json!({"objects":copies.iter().copied().map(id_string).collect::<Vec<_>>(), "count":result.copies.len(),"state":self.repeat_array()}),
        )
    }

    pub fn cancel_repeat_array(&mut self, token: u64) -> Result<(), String> {
        self.geometry
            .repeat
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Repeat array was closed or replaced")?;
        self.clear_paste_preview();
        self.clear_repeat_preview();
        Ok(())
    }
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) mod fixtures;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use glam::DVec3;
    use map_core::{edit, primitive};

    use super::fixtures::fixture;

    pub(crate) fn options() -> repeat::Options {
        repeat::Options {
            count: 3,
            translation: DVec3::X * 32.0,
            rotation: DVec3::Z * 15.0,
            pivot: DVec3::ZERO,
            texture_lock: true,
        }
    }

    fn preview(session: &mut Session, source: ObjectId) -> u64 {
        let report = session
            .preview_repeat_array(&[source], options(), &session.repeat_stamp())
            .unwrap();
        report["preview"]["token"].as_u64().unwrap()
    }

    #[test]
    fn repeat_preview_cancel_apply_and_undo_share_one_document() {
        let (mut session, source) = fixture();
        let before = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let token = preview(&mut session, source);
        assert_eq!(session.document.vmf(), &before);
        assert_ne!(session.render_document().vmf(), &before);
        assert_eq!(session.document.undo_labels().count(), history);
        session.cancel_repeat_array(token).unwrap();
        assert_eq!(session.render_document().vmf(), &before);
        assert_eq!(session.document.undo_labels().count(), history);
        let token = preview(&mut session, source);
        let expected = session.render_document().vmf().clone();
        let report = session.apply_repeat_array(Actor::Agent, token).unwrap();
        assert_eq!(report["objects"].as_array().unwrap().len(), 3);
        assert_eq!(session.document.vmf(), &expected);
        assert_eq!(session.document.undo_labels().count(), history + 1);
        assert!(session.geometry.repeat.preview.is_none());
        session.undo(Actor::User).unwrap();
        assert_eq!(session.document.vmf(), &before);
        session.redo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &expected);
    }

    #[test]
    fn invalid_updates_and_stale_tokens_preserve_current_preview() {
        let (mut session, source) = fixture();
        let token = preview(&mut session, source);
        let valid = session.repeat_array();
        for (options, snapshot) in [
            (
                repeat::Options {
                    count: 0,
                    ..options()
                },
                session.repeat_stamp(),
            ),
            (options(), "stale".into()),
        ] {
            assert!(
                session
                    .preview_repeat_array(&[source], options, &snapshot)
                    .is_err()
            );
            assert_eq!(session.repeat_array(), valid);
        }
        let replacement = preview(&mut session, source);
        assert_ne!(token, replacement);
        assert!(session.cancel_repeat_array(token).is_err());
        assert!(session.apply_repeat_array(Actor::Agent, token).is_err());
        assert_eq!(session.repeat_array()["preview"]["token"], replacement);
    }

    #[test]
    fn numeric_and_primitive_replacements_survive_old_array_cancel() {
        let (mut session, source) = fixture();
        let token = preview(&mut session, source);
        session
            .preview_transform(
                Actor::Agent,
                &[source],
                edit::Transform {
                    translation: DVec3::Y * 20.0,
                    ..edit::Transform::default()
                },
                None,
            )
            .unwrap();
        let expected = session.render_document().vmf().clone();
        assert!(session.geometry.repeat.preview.is_none());
        assert!(session.cancel_repeat_array(token).is_err());
        assert_eq!(session.render_document().vmf(), &expected);
        session.cancel_transform_preview();
        let token = preview(&mut session, source);
        let spec = primitive::Spec {
            mins: DVec3::ZERO,
            maxs: DVec3::splat(32.0),
            ..primitive::Spec::default()
        };
        session.create_primitive(Actor::Agent, spec, true).unwrap();
        let expected = session.render_document().vmf().clone();
        assert!(session.geometry.repeat.preview.is_none());
        assert!(session.cancel_repeat_array(token).is_err());
        assert_eq!(session.render_document().vmf(), &expected);
    }

    #[test]
    fn edits_selection_and_definitions_prevent_stale_apply() {
        let (mut session, source) = fixture();
        let token = preview(&mut session, source);
        session.definitions_serial += 1;
        assert!(session.apply_repeat_array(Actor::Agent, token).is_err());
        session.cancel_repeat_array(token).unwrap();
        let token = preview(&mut session, source);
        session.select(&[], SelectMode::Replace).unwrap();
        assert!(session.apply_repeat_array(Actor::User, token).is_err());
        assert!(session.geometry.repeat.preview.is_none());
    }

    #[test]
    fn hollow_replaces_repeat_only_after_validation_and_survives_old_array_cancel() {
        let mut session = Session::new(Err("No game".into()));
        let source = session
            .edit(Actor::User, "source", |tx| {
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(128.0), "brick/wall")
            })
            .unwrap();
        session.select(&[source], SelectMode::Replace).unwrap();
        let before = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let token = preview(&mut session, source);
        let array = session.render_document().vmf().clone();
        let invalid = map_core::hollow::Spec {
            thickness: 0.0,
            outward: false,
            open_faces: Default::default(),
        };
        assert!(
            session
                .preview_hollow(&[source], invalid, &session.hollow_stamp())
                .is_err()
        );
        assert_eq!(session.repeat_array()["preview"]["token"], token);
        assert_eq!(session.render_document().vmf(), &array);
        let valid = map_core::hollow::Spec {
            thickness: 16.0,
            outward: false,
            open_faces: Default::default(),
        };
        session
            .preview_hollow(&[source], valid, &session.hollow_stamp())
            .unwrap();
        let hollow = session.render_document().vmf().clone();
        assert_ne!(hollow, array);
        assert!(session.geometry.repeat.preview.is_none());
        assert!(session.cancel_repeat_array(token).is_err());
        assert!(session.apply_repeat_array(Actor::User, token).is_err());
        assert_eq!(session.render_document().vmf(), &hollow);
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.document.undo_labels().count(), history);
        session.clear_hollow_preview();
        assert_eq!(session.render_document().vmf(), &before);
    }
}
