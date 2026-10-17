use crate::identity::id_string;
use map_core::{Document, EditError, ObjectId, repeat};
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};

pub struct Preview {
    pub document: Document,
    pub result: repeat::Result,
    token: u64,
    sources: Vec<ObjectId>,
    options: repeat::Options,
    local: bool,
    generation: u64,
    revision: u64,
    definitions: u64,
}

#[derive(Default)]
pub struct State {
    serial: u64,
    pub preview: Option<Preview>,
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
                "scale": preview.options.scale.to_array(),
                "pivot": preview.options.pivot.to_array(),
                "space": if preview.local { "local" } else { "world" },
                "basis": preview.options.basis.to_cols_array_2d(),
                "frame_source": preview.local.then(|| preview.sources.iter().copied().min().map(id_string)).flatten(),
                "texture_lock": preview.options.texture_lock,
                "bounds": self.object_bounds_in(&preview.document, &objects).map(|(mins, maxs)| [mins.to_array(), maxs.to_array()]),
                "copies": preview.result.copies.iter().map(|copy| json!({
                    "index": copy.index,
                    "scale": preview.options.copy_scale(copy.index).to_array(),
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
            "transform_policy":"Copy n uses the original objects, scales each XYZ axis by its per-copy factor raised to n, then rotates by n × XYZ degrees, then translates by n × the move step. Scale and rotation share one fixed world pivot; all steps use the chosen world or captured source-local frame. Source local uses the first source in stable object-ID order (brushes before entities), captured once for every copy. Scale defaults to 1 and nonidentity factors support only brushes and brush entities. Count excludes originals.",
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
        self.preview_repeat_array_in_space(sources, options, false, snapshot)
    }

    pub fn preview_repeat_array_in_space(
        &mut self,
        sources: &[ObjectId],
        mut options: repeat::Options,
        local: bool,
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.repeat_stamp() {
            return Err("Map, selection or preview changed; inspect the repeat array again".into());
        }
        self.ensure_partitions_resolved()?;
        options.basis = self.transform_basis(sources, local)?;
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
            local,
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
            scale: DVec3::ONE,
            pivot: DVec3::ZERO,
            basis: glam::DMat3::IDENTITY,
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
    fn local_arrays_capture_one_source_frame_and_share_atomic_apply_for_both_actors() {
        for actor in [Actor::User, Actor::Agent] {
            let (mut session, source) = fixture();
            let ObjectId::Entity(id) = source else {
                panic!()
            };
            let other = session
                .edit(actor, "orient sources", |tx| {
                    tx.entity_mut(id)?.properties.set("angles", "23 37 -11");
                    edit::create_entity(
                        tx,
                        "info_target",
                        Some(DVec3::new(3.5, -6.25, 9.125)),
                        &[("angles".into(), "0 90 0".into())],
                        Vec::new(),
                    )
                })
                .unwrap();
            let sources = [other, source];
            session.select(&sources, SelectMode::Replace).unwrap();
            let before = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let basis = session.transform_basis(&[source], true).unwrap();
            let request = repeat::Options {
                translation: DVec3::new(4.5, -2.25, 1.125),
                rotation: DVec3::X * 90.0,
                pivot: DVec3::new(1.25, -3.5, 2.125),
                ..options()
            };
            let report = session
                .preview_repeat_array_in_space(&sources, request, true, &session.repeat_stamp())
                .unwrap();
            assert_eq!(report["preview"]["space"], "local");
            assert_eq!(report["preview"]["frame_source"], id_string(source));
            assert_eq!(report["preview"]["basis"], json!(basis.to_cols_array_2d()));
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.document.undo_labels().count(), history);
            for copy in &session
                .geometry
                .repeat
                .preview
                .as_ref()
                .unwrap()
                .result
                .copies
            {
                let ObjectId::Entity(id) = copy.objects[1] else {
                    panic!()
                };
                let origin = session
                    .render_document()
                    .entity(id)
                    .unwrap()
                    .origin()
                    .unwrap();
                let local = basis.transpose() * (DVec3::X * 16.0 - request.pivot);
                let rotated = match copy.index {
                    1 => DVec3::new(local.x, -local.z, local.y),
                    2 => DVec3::new(local.x, -local.y, -local.z),
                    3 => DVec3::new(local.x, local.z, -local.y),
                    _ => unreachable!(),
                };
                assert!(origin.abs_diff_eq(
                    request.pivot + basis * (rotated + request.translation * f64::from(copy.index)),
                    1e-8,
                ));
            }
            let expected = session.render_document().vmf().clone();
            session
                .apply_repeat_array(actor, report["preview"]["token"].as_u64().unwrap())
                .unwrap();
            assert_eq!(session.document.vmf(), &expected);
            assert_eq!(session.document.undo_labels().count(), history + 1);
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &before);
            session.redo(actor).unwrap();
            assert_eq!(session.document.vmf(), &expected);
        }
    }

    #[test]
    fn invalid_local_updates_preserve_draft_and_world_default_stays_exact() {
        let (mut session, source) = fixture();
        let invalid = session
            .edit(Actor::Agent, "invalid frame", |tx| {
                [
                    ("info_target", "angles", "garbage"),
                    ("info_target", "angles", "NaN 0 0"),
                    ("info_target", "angle", "garbage"),
                    ("info_target", "angle", "NaN"),
                    ("light_spot", "pitch", "garbage"),
                    ("light_environment", "pitch", "NaN"),
                ]
                .into_iter()
                .map(|(class, key, value)| {
                    edit::create_entity(
                        tx,
                        class,
                        Some(DVec3::ZERO),
                        &[(key.into(), value.into())],
                        Vec::new(),
                    )
                })
                .collect::<Result<Vec<_>, _>>()
            })
            .unwrap();
        let request = repeat::Options {
            rotation: DVec3::ZERO,
            ..options()
        };
        let report = session
            .preview_repeat_array_in_space(&[source], request, true, &session.repeat_stamp())
            .unwrap();
        let preview = session.render_document().vmf().clone();
        for &source in &invalid {
            assert!(
                session
                    .preview_repeat_array_in_space(
                        &[source],
                        request,
                        true,
                        &session.repeat_stamp()
                    )
                    .is_err()
            );
            assert_eq!(session.repeat_array(), report);
            assert_eq!(session.render_document().vmf(), &preview);
        }
        assert!(
            session
                .preview_repeat_array_in_space(&[source], options(), true, "stale")
                .is_err()
        );
        assert_eq!(session.repeat_array(), report);
        assert_eq!(session.render_document().vmf(), &preview);
        session
            .cancel_repeat_array(report["preview"]["token"].as_u64().unwrap())
            .unwrap();
        for source in invalid {
            let world = session
                .preview_repeat_array(&[source], request, &session.repeat_stamp())
                .unwrap();
            assert_eq!(world["preview"]["space"], "world");
            session
                .cancel_repeat_array(world["preview"]["token"].as_u64().unwrap())
                .unwrap();
        }
        let implicit = session
            .preview_repeat_array(&[source], options(), &session.repeat_stamp())
            .unwrap();
        let expected = session.render_document().vmf().clone();
        session
            .cancel_repeat_array(implicit["preview"]["token"].as_u64().unwrap())
            .unwrap();
        let explicit = session
            .preview_repeat_array_in_space(&[source], options(), false, &session.repeat_stamp())
            .unwrap();
        assert_eq!(explicit["preview"]["space"], "world");
        assert_eq!(explicit["preview"]["frame_source"], Value::Null);
        assert_eq!(session.render_document().vmf(), &expected);
    }

    #[test]
    fn mixed_local_sources_use_the_existing_brush_frame_independent_of_argument_order() {
        let (mut session, point) = fixture();
        let brush = session
            .edit(Actor::User, "oblique brush", |tx| {
                let brush = edit::create_block(
                    tx,
                    DVec3::new(-3.125, 2.25, 1.5),
                    DVec3::new(13.5, 9.75, 24.125),
                    "brick/wall",
                )?;
                edit::transform(
                    tx,
                    &[brush],
                    edit::Transform {
                        rotation: DVec3::new(17.0, 29.0, -11.0),
                        ..edit::Transform::default()
                    },
                )?;
                Ok(brush)
            })
            .unwrap();
        let basis = session.transform_basis(&[brush], true).unwrap();
        let request = repeat::Options {
            count: 1,
            rotation: DVec3::ZERO,
            ..options()
        };
        let report = session
            .preview_repeat_array_in_space(&[point, brush], request, true, &session.repeat_stamp())
            .unwrap();
        assert_eq!(report["preview"]["frame_source"], id_string(brush));
        assert_eq!(report["preview"]["basis"], json!(basis.to_cols_array_2d()));
        let expected_point = session
            .render_document()
            .vmf()
            .entities
            .last()
            .unwrap()
            .origin()
            .unwrap();
        session
            .cancel_repeat_array(report["preview"]["token"].as_u64().unwrap())
            .unwrap();
        let reversed = session
            .preview_repeat_array_in_space(&[brush, point], request, true, &session.repeat_stamp())
            .unwrap();
        assert_eq!(reversed["preview"]["basis"], report["preview"]["basis"]);
        let copies = session.geometry.repeat.preview.as_ref().unwrap();
        let ObjectId::Solid(original) = brush else {
            panic!()
        };
        let ObjectId::Solid(copy) = copies.result.copies[0].objects[0] else {
            panic!()
        };
        let delta = basis * request.translation;
        for (source, copied) in session
            .document
            .solid(original)
            .unwrap()
            .sides
            .iter()
            .zip(&copies.document.solid(copy).unwrap().sides)
        {
            for (before, after) in source.plane.0.into_iter().zip(copied.plane.0) {
                assert!(after.abs_diff_eq(before + delta, 1e-8));
            }
        }
        let ObjectId::Entity(copy) = copies.result.copies[0].objects[1] else {
            panic!()
        };
        let actual_point = copies.document.entity(copy).unwrap().origin().unwrap();
        assert!(actual_point.abs_diff_eq(expected_point, 1e-8));
        assert!(actual_point.abs_diff_eq(DVec3::X * 16.0 + delta, 1e-8));
    }

    #[test]
    fn scale_arrays_preserve_sources_and_valid_drafts_for_both_actors() {
        for actor in [Actor::User, Actor::Agent] {
            let (mut session, point) = fixture();
            let source = session
                .edit(actor, "oblique scale source", |tx| {
                    let source = edit::create_block(
                        tx,
                        DVec3::new(-4.125, 2.25, 1.5),
                        DVec3::new(12.5, 15.75, 24.125),
                        "brick/wall",
                    )?;
                    edit::transform(
                        tx,
                        &[source],
                        edit::Transform {
                            rotation: DVec3::new(17.0, 29.0, -11.0),
                            ..edit::Transform::default()
                        },
                    )?;
                    Ok(source)
                })
                .unwrap();
            session.select(&[source], SelectMode::Replace).unwrap();
            let before = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let camera = session.camera();
            let request = repeat::Options {
                translation: DVec3::ZERO,
                rotation: DVec3::ZERO,
                scale: DVec3::new(1.1, 0.9, 1.25),
                pivot: DVec3::new(1.25, -3.5, 2.125),
                ..options()
            };
            let report = session
                .preview_repeat_array_in_space(&[source], request, true, &session.repeat_stamp())
                .unwrap();
            assert_eq!(report["preview"]["scale"], json!([1.1, 0.9, 1.25]));
            let third: [f64; 3] =
                serde_json::from_value(report["preview"]["copies"][2]["scale"].clone()).unwrap();
            assert!(
                DVec3::from_array(third).abs_diff_eq(DVec3::new(1.331, 0.729, 1.953125), 1e-12)
            );
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.document.undo_labels().count(), history);
            assert_eq!(session.camera(), camera);
            let expected = session.render_document().vmf().clone();
            for (sources, scale) in [
                (vec![source], DVec3::ZERO),
                (vec![source], DVec3::NAN),
                (vec![source], DVec3::splat(1e300)),
                (vec![source, point], DVec3::splat(1.1)),
            ] {
                assert!(
                    session
                        .preview_repeat_array_in_space(
                            &sources,
                            repeat::Options { scale, ..request },
                            true,
                            &session.repeat_stamp()
                        )
                        .is_err()
                );
                assert_eq!(session.repeat_array(), report);
                assert_eq!(session.render_document().vmf(), &expected);
                assert_eq!(session.document.vmf(), &before);
                assert_eq!(session.document.undo_labels().count(), history);
            }
            let token = report["preview"]["token"].as_u64().unwrap();
            session.apply_repeat_array(actor, token).unwrap();
            assert_eq!(session.document.vmf(), &expected);
            assert_eq!(session.document.undo_labels().count(), history + 1);
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &before);
            session.redo(actor).unwrap();
            assert_eq!(session.document.vmf(), &expected);
            let report = session
                .preview_repeat_array_in_space(&[source], request, true, &session.repeat_stamp())
                .unwrap();
            let ObjectId::Solid(id) = source else {
                unreachable!()
            };
            session
                .edit(actor, "move scale source", |tx| {
                    edit::translate(tx, &[ObjectId::Solid(id)], DVec3::X, true)
                })
                .unwrap();
            assert!(
                session
                    .apply_repeat_array(actor, report["preview"]["token"].as_u64().unwrap())
                    .is_err()
            );
        }
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
