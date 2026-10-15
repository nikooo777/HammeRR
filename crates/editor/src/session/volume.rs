use map_core::region::{Match, Region, Selection};
use serde_json::{Value, json};

use super::*;

#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub mins: DVec3,
    pub maxs: DVec3,
    pub rule: Match,
    pub mode: SelectMode,
    pub faces: bool,
}

pub struct Preview {
    pub token: u64,
    pub spec: Spec,
    pub matches: Selection,
    generation: u64,
    revision: u64,
}

#[derive(Default)]
pub struct State {
    serial: u64,
    preview: Option<Preview>,
}

impl Session {
    pub fn volume_preview(&self) -> Option<&Preview> {
        self.volume.preview.as_ref()
    }

    pub fn clear_volume_selection(&mut self) {
        if self.volume.preview.take().is_some() {
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub(super) fn checked_volume(&self, token: u64) -> Result<&Preview, String> {
        let preview = self
            .volume_preview()
            .filter(|preview| preview.token == token)
            .ok_or("Volume preview was closed or replaced")?;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
        {
            return Err("Map changed; preview the volume again".into());
        }
        Ok(preview)
    }

    pub fn volume_selection_bounds(&self) -> [DVec3; 2] {
        let (mut mins, mut maxs) = self
            .selection_bounds()
            .or_else(|| {
                self.object_bounds_in(&self.document, &self.document.objects().collect::<Vec<_>>())
            })
            .unwrap_or((DVec3::splat(-64.0), DVec3::splat(64.0)));
        for axis in 0..3 {
            if maxs[axis] - mins[axis] < 1e-4 {
                mins[axis] -= self.spatial.grid_size.max(1.0);
                maxs[axis] += self.spatial.grid_size.max(1.0);
            }
        }
        [mins, maxs]
    }

    pub fn preview_volume_selection(&mut self, spec: Spec) -> Result<u64, String> {
        let region = Region::from_bounds(spec.mins, spec.maxs)?;
        if spec.mins.abs().max_element() > spatial::MAX_EXTENT
            || spec.maxs.abs().max_element() > spatial::MAX_EXTENT
        {
            return Err(format!(
                "Volume corners must stay within ±{} world units",
                spatial::MAX_EXTENT
            ));
        }
        self.cancel_block_preview();
        self.cancel_face_preview();
        self.cancel_clip_preview();
        self.clear_hollow_preview();
        self.clear_carve_preview();
        self.clear_merge_preview();
        self.clear_component_edit();
        self.clear_primitive_preview();
        self.cancel_transform_preview();
        self.cancel_area_selection();
        self.clip_tool = false;
        self.spatial.draw_block = false;
        let matches = self.select_region(&region, spec.rule, spec.faces);
        self.volume.serial += 1;
        let token = self.volume.serial;
        self.volume.preview = Some(Preview {
            token,
            spec,
            matches,
            generation: self.document_generation,
            revision: self.document.revision(),
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(token)
    }

    pub fn commit_volume_selection(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Selection, String> {
        self.checked_volume(token)?;
        let preview = self.volume.preview.take().unwrap();
        self.scene_serial += 1;
        self.view_serial += 1;
        if preview.spec.faces {
            self.select_faces(&preview.matches.faces, preview.spec.mode)?;
        } else {
            self.select(&preview.matches.objects, preview.spec.mode)?;
        }
        self.log(
            actor,
            format!(
                "selected by volume: {} {} matched",
                if preview.spec.faces {
                    preview.matches.faces.len()
                } else {
                    preview.matches.objects.len()
                },
                if preview.spec.faces {
                    "faces"
                } else {
                    "objects"
                }
            ),
        );
        Ok(preview.matches)
    }

    pub fn cancel_volume_selection(&mut self, token: u64) -> Result<(), String> {
        self.checked_volume(token)?;
        self.clear_volume_selection();
        Ok(())
    }

    pub fn frame_volume_selection(&mut self, token: u64, aspect: f32) -> Result<(), String> {
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("Viewport aspect must be positive and finite".into());
        }
        let spec = self.checked_volume(token)?.spec;
        self.set_camera(
            self.camera
                .frame_bounds(spec.mins.as_vec3(), spec.maxs.as_vec3(), aspect),
        );
        Ok(())
    }

    pub fn volume_selection_json(&self, offset: usize, limit: usize) -> Value {
        let preview = self.volume_preview().map(|preview| {
            json!({"token":preview.token.to_string(),"mins":preview.spec.mins.to_array(),"maxs":preview.spec.maxs.to_array(),
                "rule":if preview.spec.rule==Match::Contained {"contained"} else {"intersecting"},
                "mode":match preview.spec.mode { SelectMode::Replace=>"replace",SelectMode::Add=>"add",SelectMode::Remove=>"remove",SelectMode::Toggle=>"toggle" },
                "selection_mode":if preview.spec.faces {"faces"} else {"objects"},
                "generation":preview.generation,"revision":preview.revision,
                "object_count":preview.matches.objects.len(),"face_count":preview.matches.faces.len(),"offset":offset,
                "objects":preview.matches.objects.iter().skip(offset).take(limit.clamp(1,500)).map(|id| match id {ObjectId::Solid(id)=>format!("solid:{id}"),ObjectId::Entity(id)=>format!("entity:{id}")}).collect::<Vec<_>>(),
                "faces":preview.matches.faces.iter().skip(offset).take(limit.clamp(1,500)).map(|face|json!({"solid":face.solid,"side":face.side})).collect::<Vec<_>>()})
        });
        json!({"preview":preview,"suggested_bounds":self.volume_selection_bounds().map(|point|point.to_array()),
            "hidden_policy":"Hidden objects, child brushes and hidden tool brushes are excluded; terrain uses actual displaced sheets, excluding ordinary parent faces and volumes. Loaded models use their transformed asset bounds; unsupported models retain helper bounds."})
    }

    pub fn append_volume_guide(&self, mesh: &mut render::SceneMesh) {
        let Some(preview) = self.volume_preview() else {
            return;
        };
        let corners: Vec<_> = (0..8)
            .map(|bits| {
                DVec3::new(
                    if bits & 1 == 0 {
                        preview.spec.mins.x
                    } else {
                        preview.spec.maxs.x
                    },
                    if bits & 2 == 0 {
                        preview.spec.mins.y
                    } else {
                        preview.spec.maxs.y
                    },
                    if bits & 4 == 0 {
                        preview.spec.mins.z
                    } else {
                        preview.spec.maxs.z
                    },
                )
            })
            .collect();
        for index in 0..8 {
            for bit in [1, 2, 4] {
                let other = index ^ bit;
                if index < other {
                    for point in [corners[index], corners[other]] {
                        mesh.lines.push(render::scene::LineVertex {
                            position: point.as_vec3().to_array(),
                            color: [80, 205, 255, 255],
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(session: &mut Session, x: f64) -> ObjectId {
        session
            .create_block(
                Actor::User,
                DVec3::new(x, 0.0, 0.0),
                DVec3::new(x + 64.0, 64.0, 64.0),
                "brick/test",
                false,
            )
            .unwrap()
    }

    fn spec(x: f64) -> Spec {
        Spec {
            mins: DVec3::new(x - 1.0, -1.0, -1.0),
            maxs: DVec3::new(x + 65.0, 65.0, 65.0),
            rule: Match::Intersecting,
            mode: SelectMode::Replace,
            faces: false,
        }
    }

    #[test]
    fn volume_preview_survives_camera_changes_and_combines_selection_without_history() {
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no assets".into()));
            let a = block(&mut session, 0.0);
            let b = block(&mut session, 128.0);
            session.document.mark_saved();
            let before = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let token = session.preview_volume_selection(spec(0.0)).unwrap();
            assert_eq!(session.selected_ids(), [b]);
            assert_eq!(session.volume_preview().unwrap().matches.objects, [a]);
            assert_eq!(
                session
                    .build_scene()
                    .lines
                    .iter()
                    .filter(|line| line.color == [80, 205, 255, 255])
                    .count(),
                24
            );
            for projection in [
                Projection::Top,
                Projection::Front,
                Projection::Side,
                Projection::Perspective,
            ] {
                session.set_projection(actor, projection);
                session.frame_volume_selection(token, 1.6).unwrap();
                assert_eq!(session.volume_preview().unwrap().token, token);
            }
            session.cancel_volume_selection(token).unwrap();
            assert_eq!(session.selected_ids(), [b]);
            assert!(session.commit_volume_selection(actor, token).is_err());
            let token = session.preview_volume_selection(spec(0.0)).unwrap();
            session.commit_volume_selection(actor, token).unwrap();
            assert_eq!(session.selected_ids(), [a]);
            for (x, mode, expected) in [
                (128.0, SelectMode::Add, vec![a, b]),
                (0.0, SelectMode::Remove, vec![b]),
                (128.0, SelectMode::Toggle, vec![]),
            ] {
                let token = session
                    .preview_volume_selection(Spec { mode, ..spec(x) })
                    .unwrap();
                session.commit_volume_selection(actor, token).unwrap();
                assert_eq!(session.selected_ids(), expected);
            }
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.document.undo_labels().count(), history);
            assert!(!session.document.is_dirty());
        }
    }

    #[test]
    fn volume_matches_visible_brush_entities_and_faces_with_geometry_rules() {
        let mut session = Session::new(Err("no assets".into()));
        let a = block(&mut session, 0.0);
        let hidden = block(&mut session, 0.0);
        session
            .set_visibility(Actor::User, "hide", &[hidden])
            .unwrap();
        let near = block(&mut session, 0.0);
        let far = block(&mut session, 256.0);
        let entity = session
            .tie_to_entity(Actor::User, &[near, far], "func_detail", BTreeMap::new())
            .unwrap();
        session.preview_volume_selection(spec(0.0)).unwrap();
        assert_eq!(
            session.volume_preview().unwrap().matches.objects,
            [a, entity]
        );
        session
            .preview_volume_selection(Spec {
                rule: Match::Contained,
                ..spec(0.0)
            })
            .unwrap();
        assert_eq!(session.volume_preview().unwrap().matches.objects, [a]);
        session
            .set_visibility(Actor::User, "hide", &[entity])
            .unwrap();
        session
            .preview_volume_selection(Spec {
                mins: DVec3::splat(16.0),
                maxs: DVec3::splat(32.0),
                ..spec(0.0)
            })
            .unwrap();
        assert_eq!(session.volume_preview().unwrap().matches.objects, [a]);
        let token = session
            .preview_volume_selection(Spec {
                mins: DVec3::new(-1.0, -1.0, 63.0),
                maxs: DVec3::new(65.0, 65.0, 65.0),
                faces: true,
                rule: Match::Contained,
                ..spec(0.0)
            })
            .unwrap();
        assert!(!session.face_mode());
        assert_eq!(session.volume_preview().unwrap().matches.faces.len(), 1);
        session
            .commit_volume_selection(Actor::Agent, token)
            .unwrap();
        assert!(session.face_mode());
        assert_eq!(session.selected_faces().len(), 1);
        assert!(session.selected_ids().is_empty());
        let bounds = session.volume_selection_bounds();
        assert!(bounds[0].cmplt(bounds[1]).all());
    }

    #[test]
    fn volume_tokens_guard_replacements_invalid_input_and_other_previews() {
        let mut session = Session::new(Err("no assets".into()));
        let id = block(&mut session, 0.0);
        let original = session.document.vmf().clone();
        let first = session.preview_volume_selection(spec(0.0)).unwrap();
        for invalid in [
            Spec {
                maxs: DVec3::splat(-1.0),
                ..spec(0.0)
            },
            Spec {
                maxs: DVec3::splat(f64::NAN),
                ..spec(0.0)
            },
            Spec {
                maxs: DVec3::splat(100000.0),
                ..spec(0.0)
            },
        ] {
            assert!(session.preview_volume_selection(invalid).is_err());
            assert_eq!(session.volume_preview().unwrap().token, first);
        }
        let second = session.preview_volume_selection(spec(128.0)).unwrap();
        assert!(session.cancel_volume_selection(first).is_err());
        assert!(
            session
                .commit_volume_selection(Actor::Agent, first)
                .is_err()
        );
        assert_eq!(session.volume_preview().unwrap().token, second);
        assert!(session.frame_volume_selection(second, f32::NAN).is_err());
        session.preview_block(DVec3::ZERO, DVec3::ONE).unwrap();
        assert!(session.volume_preview().is_none());
        let token = session.preview_volume_selection(spec(0.0)).unwrap();
        assert!(session.block_preview().is_none());
        session.select(&[], SelectMode::Replace).unwrap();
        assert!(
            session
                .commit_volume_selection(Actor::Agent, token)
                .is_err()
        );
        let token = session.preview_volume_selection(spec(0.0)).unwrap();
        let mut view = session.view();
        view.tool_textures = !view.tool_textures;
        session.set_view(view);
        assert!(session.commit_volume_selection(Actor::User, token).is_err());
        assert_eq!(session.document.vmf(), &original);
        session.select(&[id], SelectMode::Replace).unwrap();
        let token = session.preview_volume_selection(spec(0.0)).unwrap();
        session.new_map(Actor::Agent, true).unwrap();
        assert!(session.commit_volume_selection(Actor::User, token).is_err());
        block(&mut session, 0.0);
        assert_ne!(session.preview_volume_selection(spec(0.0)).unwrap(), first);
    }
}
