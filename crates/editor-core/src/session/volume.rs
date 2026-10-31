use super::previews;
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
    filters: Option<u64>,
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
            || preview
                .filters
                .is_some_and(|revision| revision != self.selection_filter_revision())
        {
            return Err("Map changed; preview the volume again".into());
        }
        Ok(preview)
    }

    pub fn volume_selection_bounds(&self) -> [DVec3; 2] {
        let selection = if self.face_mode {
            self.selection_bounds()
        } else {
            self.presentation_object_bounds_in(&self.document, &self.selected_ids(), None)
        };
        let (mut mins, mut maxs) = selection
            .or_else(|| {
                let ids = self.document.objects().collect::<Vec<_>>();
                if self.face_mode {
                    self.object_bounds_in(&self.document, &ids)
                } else {
                    self.presentation_object_bounds_in(&self.document, &ids, None)
                }
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
        self.clear_geometry_previews_except(&[previews::Kind::Paint]);

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
            filters: (!spec.faces).then(|| self.selection_filter_revision()),
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
            self.select_picked_objects(&preview.matches.objects, preview.spec.mode)?;
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
        json!({"preview":preview,"selection_filters":self.selection_filter_state(),"suggested_bounds":self.volume_selection_bounds().map(|point|point.to_array()),
            "hidden_policy":"Hidden objects, child brushes and hidden tool brushes are excluded; terrain uses actual displaced sheets, excluding ordinary parent faces and volumes. Loaded models use their transformed asset bounds; unsupported models retain helper bounds. Enabled admitted rope spans use conservative sampled-curve bounds including half Width, unioned with their source helper; targets retain independent ownership. Rope visibility changes retire pending area/volume reviews."})
    }

    pub fn append_volume_guide(&self, mesh: &mut render::SceneMesh) {
        let Some(preview) = self.volume_preview() else {
            return;
        };
        mesh.add_bounds_lines(preview.spec.mins, preview.spec.maxs, [80, 205, 255, 255]);
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.preview
            .as_ref()
            .map(|preview| serde_json::json!([self.serial, preview.token]))
    }
}
