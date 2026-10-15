use super::{Actor, Session};
use map_core::compile_region::{self, Definition};
use serde_json::{Value, json};

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    draft: Option<Draft>,
}

struct Draft {
    token: u64,
    generation: u64,
    revision: u64,
    definition: Option<Definition>,
}

pub(super) fn configuration(vmf: &source_formats::vmf::Vmf) -> Value {
    match compile_region::inspect(vmf) {
        Ok(value) => definition_json(value),
        Err(error) => json!({"error":error.to_string()}),
    }
}

pub(super) fn build_configuration(vmf: &source_formats::vmf::Vmf) -> Value {
    match compile_region::active(vmf) {
        Ok(Some(bounds)) => definition_json(Some(Definition {
            enabled: true,
            bounds,
        })),
        Ok(None) => json!({"enabled":false}),
        Err(error) => json!({"error":error.to_string()}),
    }
}

fn definition_json(value: Option<Definition>) -> Value {
    value.map_or(Value::Null, |value| json!({"enabled":value.enabled,"mins":value.bounds.mins.to_array(),"maxs":value.bounds.maxs.to_array()}))
}

impl Session {
    pub fn compile_region_build_scope(&self) -> Value {
        build_configuration(self.document.vmf())
    }

    pub fn compile_region_label(&self) -> &'static str {
        match compile_region::active(self.document.vmf()) {
            Ok(Some(_)) => "Region build…",
            Ok(None) => "Compile region…",
            Err(_) => "Region error…",
        }
    }

    pub fn compile_region_stamp(&self) -> String {
        format!(
            "{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.compile_region.serial
        )
    }

    pub fn compile_region_state(&self) -> Value {
        let [mins, maxs] = self.volume_selection_bounds();
        json!({
            "build_scope_label":self.compile_region_label(),"build_scope":self.compile_region_build_scope(),"snapshot":self.compile_region_stamp(), "committed":configuration(self.document.vmf()),
            "draft":self.compile_region.draft.as_ref().map(|draft| json!({"token":draft.token,"clear":draft.definition.is_none(),"region":definition_json(draft.definition),"valid":draft.generation==self.document_generation && draft.revision==self.document.revision()})),
            "suggested_bounds":{"mins":mins.to_array(),"maxs":maxs.to_array()},
            "limits":{"minimum_extent":compile_region::MIN_EXTENT,"coordinate":compile_region::MAX_COORDINATE,"shell_thickness":compile_region::SHELL_THICKNESS},
            "help":"Region builds add a black solid boundary and can hide full-map leaks. Water must fit completely inside the region and belong to the world. Terrain, slime, water in entities, overlays, areaportals and occluders require a full-map build.",
            "policy":"Intersecting world/detail brushes and included brush models stay whole. Point entities use origins; brush entities use resolved pivots; originless globals, sun and water LOD controls remain. Nonspatial sun/water controls preserve origins without seeding exterior region leaks. Water must be fully contained and structural world geometry. Terrain, slime, detail/model water and consumed region helpers are unsupported. Region builds mask full-map leaks; disable the region for a full-map check. Unapplied drafts never affect builds."
        })
    }

    pub fn preview_compile_region(
        &mut self,
        actor: Actor,
        definition: Option<Definition>,
        snapshot: &str,
    ) -> Result<Value, String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        if snapshot != self.compile_region_stamp() {
            return Err("Map or compile-region draft changed; inspect the region again".into());
        }
        compile_region::validate_edit(self.document.vmf(), definition)
            .map_err(|error| error.to_string())?;
        self.compile_region.serial += 1;
        self.compile_region.draft = Some(Draft {
            token: self.compile_region.serial,
            generation: self.document_generation,
            revision: self.document.revision(),
            definition,
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        self.log(actor, "Preview compile region");
        Ok(self.compile_region_state())
    }

    pub fn apply_compile_region(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self
            .compile_region
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Compile-region draft was closed or replaced")?;
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
        {
            return Err("Map changed; preview the compile region again".into());
        }
        let definition = draft.definition;
        self.edit(actor, "Set compile region", |tx| {
            compile_region::apply(tx, definition)
        })?;
        self.compile_region.draft = None;
        self.compile_region.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(self.compile_region_state())
    }

    pub fn cancel_compile_region(&mut self, token: u64) -> Result<Value, String> {
        self.compile_region
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Compile-region draft was closed or replaced")?;
        self.compile_region.draft = None;
        self.compile_region.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(self.compile_region_state())
    }

    fn displayed_compile_region(&self) -> Option<Definition> {
        if let Some(draft) = &self.compile_region.draft
            && draft.generation == self.document_generation
            && draft.revision == self.document.revision()
        {
            return draft.definition;
        }
        compile_region::inspect(self.document.vmf()).ok().flatten()
    }

    pub fn frame_compile_region(&mut self, actor: Actor, aspect: f32) -> Result<(), String> {
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("Viewport aspect must be finite and positive".into());
        }
        let bounds = self
            .displayed_compile_region()
            .ok_or("No compile-region bounds to frame")?
            .bounds;
        self.set_camera(self.camera.frame_bounds(
            bounds.mins.as_vec3(),
            bounds.maxs.as_vec3(),
            aspect,
        ));
        self.log(actor, "Frame compile region");
        Ok(())
    }

    pub(super) fn append_compile_region_guide(&self, mesh: &mut render::SceneMesh) {
        let Some(region) = self.displayed_compile_region() else {
            return;
        };
        if region.enabled
            || self.compile_region.draft.as_ref().is_some_and(|draft| {
                draft.generation == self.document_generation
                    && draft.revision == self.document.revision()
            })
        {
            mesh.add_bounds_lines(region.bounds.mins, region.bounds.maxs, [255, 170, 70, 255]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use map_core::compile_region::Bounds;

    fn definition(enabled: bool) -> Option<Definition> {
        Some(Definition {
            enabled,
            bounds: Bounds {
                mins: DVec3::new(-128.0, -128.0, 0.0),
                maxs: DVec3::new(128.0, 128.0, 256.0),
            },
        })
    }

    #[test]
    fn owned_region_drafts_share_cancel_undo_and_frozen_compile_snapshots() {
        for actor in [Actor::User, Actor::Agent] {
            let (_directory, game) = crate::test_assets::game();
            let mut session = Session::new(Ok(game));
            let object = session
                .create_block(actor, DVec3::ZERO, DVec3::splat(32.0), "test", false)
                .unwrap();
            let before = session.document.vmf().clone();
            let selection = session.selected_ids();
            let initial = session.compile_region_stamp();
            let first = session
                .preview_compile_region(actor, definition(true), &initial)
                .unwrap()["draft"]["token"]
                .as_u64()
                .unwrap();
            assert_eq!(session.document.vmf(), &before);
            assert!(
                session
                    .preview_compile_region(actor, definition(false), &initial)
                    .is_err()
            );
            let second = session
                .preview_compile_region(actor, definition(false), &session.compile_region_stamp())
                .unwrap()["draft"]["token"]
                .as_u64()
                .unwrap();
            assert!(session.cancel_compile_region(first).is_err());
            session.apply_compile_region(actor, second).unwrap();
            assert_eq!(session.selected_ids(), selection);
            let saved = session.document.vmf().clone();
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &before);
            session.redo(actor).unwrap();
            assert_eq!(session.document.vmf(), &saved);
            let pending = session
                .preview_compile_region(actor, definition(true), &session.compile_region_stamp())
                .unwrap()["draft"]["token"]
                .as_u64()
                .unwrap();
            let request = session
                .begin_compile(
                    actor,
                    Some(std::path::Path::new("/tmp/region-snapshot.bsp")),
                )
                .unwrap();
            session.apply_compile_region(actor, pending).unwrap();
            assert_eq!(
                session.compile_status()["job"]["compile_region"]["enabled"],
                false
            );
            session
                .cancel_compile(actor, session.compile_token().unwrap())
                .unwrap();
            assert!(request.run().is_err());
            session.finish_compile(actor, &Err("cancelled fixture".into()));
            let token = session
                .preview_compile_region(actor, None, &session.compile_region_stamp())
                .unwrap()["draft"]["token"]
                .as_u64()
                .unwrap();
            session
                .edit(actor, "other edit", |tx| {
                    map_core::edit::translate(tx, &[object], DVec3::X, true)
                })
                .unwrap();
            assert!(session.apply_compile_region(actor, token).is_err());
            session.cancel_compile_region(token).unwrap();
            assert_eq!(session.compile_region_state()["committed"]["enabled"], true);
        }
    }
}
