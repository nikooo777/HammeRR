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
    fn compile_region_suggested_bounds(&self) -> Result<[glam::DVec3; 2], String> {
        let [mut mins, mut maxs] = self.volume_selection_bounds();
        let selected = if self.face_mode {
            self.face_selection
                .iter()
                .map(|face| map_core::ObjectId::Solid(face.solid))
                .collect()
        } else {
            self.selection.clone()
        };
        let vmf = self.document.vmf();
        let solids = vmf.world.solids.iter().map(|solid| (None, solid)).chain(
            vmf.entities
                .iter()
                .filter(|entity| !entity.hidden)
                .flat_map(|entity| {
                    entity
                        .solids
                        .iter()
                        .map(move |solid| (Some(entity.id), solid))
                }),
        );
        for (owner, solid) in solids {
            if solid.hidden
                || !solid.sides.iter().any(|side| side.displacement.is_some())
                || (!selected.is_empty()
                    && !selected.contains(&map_core::ObjectId::Solid(solid.id))
                    && !owner.is_some_and(|id| selected.contains(&map_core::ObjectId::Entity(id))))
            {
                continue;
            }
            let geometry = map_core::BrushGeometry::from_solid(solid).map_err(|error| {
                format!(
                    "Cannot suggest compile bounds for terrain solid:{}: {error}",
                    solid.id
                )
            })?;
            mins = mins.min(geometry.mins);
            maxs = maxs.max(geometry.maxs);
        }
        Ok([mins, maxs])
    }

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
        let suggested = match self.compile_region_suggested_bounds() {
            Ok([mins, maxs]) => json!({"mins":mins.to_array(),"maxs":maxs.to_array()}),
            Err(error) => json!({"error":error}),
        };
        json!({
            "build_scope_label":self.compile_region_label(),"build_scope":self.compile_region_build_scope(),"snapshot":self.compile_region_stamp(), "committed":configuration(self.document.vmf()),
            "draft":self.compile_region.draft.as_ref().map(|draft| json!({"token":draft.token,"clear":draft.definition.is_none(),"region":definition_json(draft.definition),"valid":draft.generation==self.document_generation && draft.revision==self.document.revision()})),
            "suggested_bounds":suggested,
            "limits":{"minimum_extent":compile_region::MIN_EXTENT,"coordinate":compile_region::MAX_COORDINATE,"shell_thickness":compile_region::SHELL_THICKNESS},
            "help":"Region builds add a black solid boundary and can hide full-map leaks. Include whole terrain, world-liquid volumes and portal/occluder helpers. Imported overlays need all their target surfaces. Crossing helpers and detail/model liquids need a full-map build.",
            "policy":"Intersecting world/detail brushes and included brush models stay whole. Point entities use origins; brush entities use resolved pivots; originless globals, sun and water LOD controls remain. Nonspatial sun/water controls preserve origins without seeding exterior region leaks. World/detail terrain needs its complete base cage and displaced grid contained. Water and opaque slime must be fully contained structural world geometry. Areaportals, windows, occluders and visclusters use complete visible world-space geometry; contained helpers remain, disjoint helpers are omitted and crossing bounds reject. Imported overlays retain their full definitions regardless of marker origin and require every target to emit a surface. Retained visclusters use the normal dry-leaf, area and intersection rules against the sealed regional tree. Detail/model liquids and runtime-model terrain are unsupported. Unsupported instance content rejects before publication. Region builds mask full-map leaks; disable the region for a full-map check. Unapplied drafts never affect builds."
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

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.draft
            .as_ref()
            .map(|draft| serde_json::json!([self.serial, draft.token]))
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
    fn selected_terrain_suggestions_include_the_base_cage() {
        for actor in [Actor::User, Actor::Agent] {
            let (_directory, game) = crate::test_assets::game();
            let mut session = Session::new(Ok(game));
            let map_core::ObjectId::Solid(id) = session
                .create_block(
                    actor,
                    DVec3::new(32.0, 32.0, 0.0),
                    DVec3::new(112.0, 112.0, 32.0),
                    "test",
                    false,
                )
                .unwrap()
            else {
                unreachable!()
            };
            let side = session
                .document()
                .solid(id)
                .unwrap()
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id;
            let face = map_core::surface::FaceId { solid: id, side };
            session
                .select_faces(&[face], super::super::SelectMode::Replace)
                .unwrap();
            session.create_terrain(actor, &[face], 2, 0).unwrap();
            let hint = session.compile_region_state()["suggested_bounds"].clone();
            assert!(hint["mins"][2].as_f64().unwrap() <= 0.0);
            assert!(hint["maxs"][2].as_f64().unwrap() >= 32.0);
        }
    }

    #[test]
    fn imported_active_scope_shares_readback_frame_guides_and_guarded_clear_history() {
        for actor in [Actor::User, Actor::Agent] {
            for multiple_roots in [false, true] {
                let (_directory, game) = crate::test_assets::game();
                let mut session = Session::new(Ok(game));
                let mut text = session.document.vmf().write(&Default::default());
                text.push_str(
                    r#"
cordons
{
    "active" "1"
    "vendor" "preserved root metadata"
    cordon
    {
        "name" "Enabled box"
        "active" "1"
        box { "mins" "(-128 -128 0)" "maxs" "(128 128 256)" }
    }
    cordon
    {
        "name" "Disabled box"
        "active" "0"
        box { "mins" "(512 512 512)" "maxs" "(768 768 768)" }
        vendor { "opaque" "preserved child metadata" }
    }
}
"#,
                );
                if multiple_roots {
                    text.push_str(
                        "cordon { \"active\" \"0\" \"mins\" \"(-512 -512 -512)\" \"maxs\" \"(-256 -256 -256)\" \"name\" \"Disabled root\" }",
                    );
                }
                let source = source_formats::vmf::Vmf::parse(&text).unwrap();
                session.replace_document(map_core::Document::new(source), None);
                for offset in [0.0, 64.0] {
                    session
                        .create_block(
                            actor,
                            DVec3::splat(offset),
                            DVec3::splat(offset + 32.0),
                            "test",
                            false,
                        )
                        .unwrap();
                }
                session.undo(actor).unwrap();
                let source = session.document.vmf().clone();
                let serialized = source.write(&Default::default());
                let revision = session.document.revision();
                let undo = session
                    .document
                    .undo_labels()
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                let redo = session
                    .document
                    .redo_labels()
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                let selection = session.selected_ids();
                let stamp = session.compile_region_stamp();
                let state = session.compile_region_state();
                let region = definition(true).unwrap();
                assert_eq!(state["committed"], definition_json(Some(region)));
                assert_eq!(state["committed"], state["build_scope"]);
                assert_eq!(state["build_scope_label"], "Region build…");
                let expected_camera = session.camera.frame_bounds(
                    region.bounds.mins.as_vec3(),
                    region.bounds.maxs.as_vec3(),
                    1.6,
                );
                session.frame_compile_region(actor, 1.6).unwrap();
                assert_eq!(session.camera, expected_camera);
                let mesh = session.build_scene();
                let guide = mesh
                    .lines
                    .iter()
                    .filter(|line| line.color == [255, 170, 70, 255])
                    .collect::<Vec<_>>();
                assert_eq!(guide.len(), 24);
                assert!(guide.iter().all(|line| {
                    line.position.iter().enumerate().all(|(axis, coordinate)| {
                        *coordinate == region.bounds.mins[axis] as f32
                            || *coordinate == region.bounds.maxs[axis] as f32
                    })
                }));
                assert_eq!(session.document.vmf(), &source);
                assert_eq!(
                    session.document.vmf().write(&Default::default()),
                    serialized
                );
                assert_eq!(session.document.revision(), revision);
                assert_eq!(session.document.undo_labels().collect::<Vec<_>>(), undo);
                assert_eq!(session.document.redo_labels().collect::<Vec<_>>(), redo);
                assert_eq!(session.selected_ids(), selection);
                assert_eq!(session.compile_region_stamp(), stamp);

                let error = session
                    .preview_compile_region(actor, definition(false), &stamp)
                    .unwrap_err();
                assert!(error.contains("clear"));
                assert_eq!(session.compile_region_stamp(), stamp);
                let clear =
                    session.preview_compile_region(actor, None, &stamp).unwrap()["draft"]["token"]
                        .as_u64()
                        .unwrap();
                session.cancel_compile_region(clear).unwrap();
                assert_eq!(
                    session.compile_region_state()["committed"],
                    state["committed"]
                );
                assert_eq!(session.document.vmf(), &source);
                assert_eq!(session.document.revision(), revision);
                assert_eq!(session.document.undo_labels().collect::<Vec<_>>(), undo);
                assert_eq!(session.document.redo_labels().collect::<Vec<_>>(), redo);
                let clear = session
                    .preview_compile_region(actor, None, &session.compile_region_stamp())
                    .unwrap()["draft"]["token"]
                    .as_u64()
                    .unwrap();
                session.apply_compile_region(actor, clear).unwrap();
                assert!(session.compile_region_state()["committed"].is_null());
                assert_eq!(
                    compile_region::active(session.document.vmf()).unwrap(),
                    None
                );
                assert_eq!(session.document.undo_labels().count(), undo.len() + 1);
                session.undo(actor).unwrap();
                assert_eq!(session.document.vmf(), &source);
                assert_eq!(
                    session.document.vmf().write(&Default::default()),
                    serialized
                );
                assert_eq!(
                    session.compile_region_state()["committed"],
                    state["committed"]
                );
                session.frame_compile_region(actor, 1.6).unwrap();
            }
        }
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
