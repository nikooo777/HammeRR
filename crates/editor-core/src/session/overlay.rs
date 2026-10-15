use glam::DVec3;
use map_core::overlay::{self, Definition, Options};
use map_core::surface::FaceId;
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub source: FaceId,
    pub options: Options,
}

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    context_serial: u64,
    draft: Option<Draft>,
}

impl State {
    pub(super) fn context_changed(&mut self) {
        self.context_serial += 1;
    }
}

struct Draft {
    token: u64,
    generation: u64,
    revision: u64,
    selection: String,
    owner: Actor,
    source: FaceId,
    definition: Definition,
}

fn options_json(options: &Options) -> Value {
    json!({
        "material":options.material,"targetname":options.targetname,
        "render_order":options.render_order,"u":options.u,"v":options.v,
        "fade_min":options.fade_min,"fade_max":options.fade_max,
    })
}

fn bounds_json(definition: &Definition) -> Value {
    let (mins, maxs) = definition
        .corners()
        .into_iter()
        .map(DVec3::from_array)
        .fold((DVec3::MAX, DVec3::MIN), |(mins, maxs), point| {
            (mins.min(point), maxs.max(point))
        });
    json!({"mins":mins.to_array(),"maxs":maxs.to_array()})
}

impl Session {
    fn overlay_selection_stamp(&self) -> String {
        format!(
            "{}:{}:{:?}:{:?}",
            self.overlay.context_serial, self.face_mode, self.selection, self.face_selection
        )
    }

    pub fn overlay_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.overlay.serial,
            self.overlay_selection_stamp(),
            self.overlay_context().is_ok()
        )
    }

    fn overlay_context(&self) -> Result<(), String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        if !std::ptr::eq(self.render_document(), &self.document) {
            return Err("Apply or cancel the geometry preview before creating an overlay".into());
        }
        Ok(())
    }

    fn selected_overlay_source(&self) -> Result<(FaceId, Definition), String> {
        if !self.face_mode || self.face_selection.len() != 1 {
            return Err("Select exactly one visible world or func_detail quad face".into());
        }
        self.overlay_context()?;
        let source = *self.face_selection.first().unwrap();
        let definition = overlay::fit(
            self.document.vmf(),
            source,
            Options {
                material: "dev/dev_measuregeneric01".into(),
                ..Options::default()
            },
        )?;
        Ok((source, definition))
    }

    fn checked_overlay(&self, token: u64) -> Result<&Draft, String> {
        let draft = self
            .overlay
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Overlay draft was closed or replaced")?;
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
            || draft.selection != self.overlay_selection_stamp()
        {
            return Err("Map or selection changed; preview the overlay again".into());
        }
        self.overlay_context()?;
        Ok(draft)
    }

    pub fn get_overlay_state(&self) -> Value {
        let (selected_source, selection_error) = match self.selected_overlay_source() {
            Ok((source, definition)) => (
                json!({"solid":source.solid,"side":source.side,"corners":definition.corners()}),
                Value::Null,
            ),
            Err(error) => (Value::Null, json!(error)),
        };
        let draft = self.overlay.draft.as_ref().map(|draft| {
            json!({
                "token":draft.token,"owner":draft.owner.label(),
                "valid":self.checked_overlay(draft.token).is_ok(),
                "source":{"solid":draft.source.solid,"side":draft.source.side},
                "options":options_json(&draft.definition.options),
                "corners":draft.definition.corners(),"bounds":bounds_json(&draft.definition),
                "basis_origin":draft.definition.basis_origin,"basis_normal":draft.definition.basis_normal,
                "basis_u":draft.definition.basis_u,"basis_v":draft.definition.basis_v,
            })
        });
        json!({
            "snapshot":self.overlay_stamp(),"generation":self.document_generation,
            "revision":self.document.revision(),"selected_source":selected_source,
            "selection_error":selection_error,"draft":draft,
            "policy":"Fit one overlay to one visible planar world or func_detail quad. Preview changes no geometry, selection or history. Review transfers draft ownership. Apply creates and selects the overlay in one Undo step; imported overlay projection editing remains unavailable.",
        })
    }

    pub fn preview_overlay(
        &mut self,
        actor: Actor,
        request: Request,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.overlay_context()?;
        if snapshot != self.overlay_stamp() {
            return Err("Map, selection or overlay draft changed; inspect overlays again".into());
        }
        let definition = overlay::fit(self.document.vmf(), request.source, request.options)?;
        self.overlay.serial += 1;
        self.scene_serial += 1;
        self.overlay.draft = Some(Draft {
            token: self.overlay.serial,
            generation: self.document_generation,
            revision: self.document.revision(),
            selection: self.overlay_selection_stamp(),
            owner: actor,
            source: request.source,
            definition,
        });
        self.view_serial += 1;
        self.log(actor, "Preview overlay");
        Ok(self.get_overlay_state())
    }

    pub fn review_overlay(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.checked_overlay(token)?;
        self.overlay.serial += 1;
        let draft = self.overlay.draft.as_mut().unwrap();
        draft.token = self.overlay.serial;
        draft.owner = actor;
        self.view_serial += 1;
        self.log(actor, "Review overlay");
        Ok(self.get_overlay_state())
    }

    pub fn apply_overlay(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self.checked_overlay(token)?;
        if draft.owner != actor {
            return Err("Review the current overlay draft before applying it".into());
        }
        let source = draft.source;
        let options = draft.definition.options.clone();
        let id = self.edit(actor, "Create overlay", |tx| {
            overlay::create(tx, source, options)
        })?;
        self.overlay.draft = None;
        self.overlay.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        self.select(&[id], SelectMode::Replace)?;
        let mut result = self.get_overlay_state();
        result["created"] = json!(crate::identity::id_string(id));
        Ok(result)
    }

    pub fn cancel_overlay(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self
            .overlay
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Overlay draft was closed or replaced")?;
        if draft.owner != actor {
            return Err("Review the current overlay draft before cancelling it".into());
        }
        self.overlay.draft = None;
        self.overlay.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        self.log(actor, "Cancel overlay");
        Ok(self.get_overlay_state())
    }

    pub(super) fn append_overlay_guide(&self, mesh: &mut render::SceneMesh) {
        let Some(draft) = self
            .overlay
            .draft
            .as_ref()
            .filter(|draft| self.checked_overlay(draft.token).is_ok())
        else {
            return;
        };
        let corners = draft.definition.corners();
        for index in 0..corners.len() {
            for point in [corners[index], corners[(index + 1) % corners.len()]] {
                mesh.lines.push(render::scene::LineVertex {
                    position: DVec3::from_array(point).as_vec3().to_array(),
                    color: [80, 220, 255, 255],
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
