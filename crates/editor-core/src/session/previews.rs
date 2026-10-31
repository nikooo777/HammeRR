use super::*;

#[derive(Default)]
pub(super) struct State {
    pub(super) instance_collapse: instance_collapse::State,
    pub(super) entity_gallery: entity_gallery::State,
    pub(super) entity_placement: entity_placement::State,
    pub(super) primitives: primitives::State,
    pub(super) repeat: repeat::State,
    pub(super) linked_path: linked_path::State,
    pub(super) clipboard: clipboard::State,
    pub(super) components: components::State,
    pub(super) merging: merging::State,
    pub(super) carving: carving::State,
    pub(super) hollowing: hollowing::State,
    pub(super) paint: paint::State,
    pub(super) transform_preview: Option<TransformPreview>,
    pub(super) clip_preview: Option<ClipPreview>,
    pub(super) face_preview: Option<FacePreview>,
}

impl State {
    pub(super) fn document<'a>(&'a self, committed: &'a Document) -> &'a Document {
        self.instance_collapse
            .document()
            .or_else(|| self.entity_placement.document())
            .or_else(|| self.entity_gallery.document())
            .or_else(|| {
                self.primitives
                    .preview
                    .as_ref()
                    .map(|preview| &preview.document)
            })
            .or_else(|| {
                self.repeat
                    .preview
                    .as_ref()
                    .map(|preview| &preview.document)
            })
            .or_else(|| {
                self.linked_path
                    .preview
                    .as_ref()
                    .map(|preview| &preview.document)
            })
            .or_else(|| {
                self.clipboard
                    .preview
                    .as_ref()
                    .map(|preview| &preview.document)
            })
            .or_else(|| {
                self.transform_preview
                    .as_ref()
                    .map(|preview| &preview.document)
            })
            .or_else(|| {
                self.components
                    .preview
                    .as_ref()
                    .map(|preview| &preview.document)
            })
            .or_else(|| {
                self.merging
                    .preview
                    .as_ref()
                    .map(|preview| &preview.document)
            })
            .or_else(|| {
                self.carving
                    .preview
                    .as_ref()
                    .map(|preview| &preview.document)
            })
            .or_else(|| {
                self.hollowing
                    .preview
                    .as_ref()
                    .map(|preview| &preview.document)
            })
            .or_else(|| self.clip_preview.as_ref().map(|preview| &preview.document))
            .or_else(|| self.face_preview.as_ref().map(|preview| &preview.document))
            .or_else(|| {
                self.paint
                    .stroke
                    .as_ref()
                    .and_then(|stroke| stroke.texture.as_ref())
                    .map(|texture| &texture.document)
            })
            .unwrap_or(committed)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    InstanceCollapse,
    EntityPlacement,
    EntityGallery,
    Paste,
    Repeat,
    LinkedPath,
    Clip,
    Hollow,
    Carve,
    Merge,
    Components,
    Primitive,
    Transform,
    Face,
    Paint,
}

impl Session {
    pub(crate) fn edit_handle(kind: &str, token: Option<u64>) -> serde_json::Value {
        token.map_or(
            serde_json::Value::Null,
            |token| serde_json::json!({"kind": kind, "token": token.to_string()}),
        )
    }

    pub fn face_preview_handle(&self) -> serde_json::Value {
        Self::edit_handle("face_preview", self.face_preview_token())
    }

    pub(super) fn feedback_handles(&self) -> Vec<serde_json::Value> {
        [
            self.hollow_handle(),
            self.carve_handle(),
            self.repeat_array_handle(),
            self.face_preview_handle(),
            self.terrain_stroke_handle(),
        ]
        .into_iter()
        .filter(|handle| !handle.is_null())
        .collect()
    }

    pub fn select_grouped_report(
        &mut self,
        ids: &[ObjectId],
        mode: SelectMode,
    ) -> Result<serde_json::Value, String> {
        self.require_root_objects(ids)?;
        let before = self.feedback_handles();
        self.select_grouped(ids, mode)?;
        let after = self.feedback_handles();
        let cleared = before
            .iter()
            .filter(|handle| !after.contains(handle))
            .map(|handle| handle["kind"].clone())
            .collect::<Vec<_>>();
        Ok(serde_json::json!({
            "selection": self.selection.iter().copied().map(crate::identity::id_string).collect::<Vec<_>>(),
            "cleared_preview_kinds": cleared,
        }))
    }

    pub(super) fn clear_geometry_previews_except(&mut self, retained: &[Kind]) {
        if !retained.contains(&Kind::InstanceCollapse) {
            self.clear_instance_collapse();
        }
        if !retained.contains(&Kind::EntityGallery) {
            self.clear_entity_gallery();
        }
        if !retained.contains(&Kind::EntityPlacement) {
            self.clear_entity_placement();
        }
        if !retained.contains(&Kind::Paste) {
            self.clear_paste_preview();
        }
        if !retained.contains(&Kind::Repeat) {
            self.clear_repeat_preview();
        }
        if !retained.contains(&Kind::LinkedPath) {
            self.clear_linked_path_preview();
        }
        if !retained.contains(&Kind::Clip) {
            self.cancel_clip_preview();
        }
        if !retained.contains(&Kind::Hollow) {
            self.clear_hollow_preview();
        }
        if !retained.contains(&Kind::Carve) {
            self.clear_carve_preview();
        }
        if !retained.contains(&Kind::Merge) {
            self.clear_merge_preview();
        }
        if !retained.contains(&Kind::Components) {
            self.clear_component_edit();
        }
        if !retained.contains(&Kind::Primitive) {
            self.clear_primitive_preview();
        }
        if !retained.contains(&Kind::Transform) {
            self.cancel_transform_preview();
        }
        if !retained.contains(&Kind::Face) {
            self.clear_face_preview();
        }
        if !retained.contains(&Kind::Paint) {
            self.clear_paint_stroke();
        }
    }
}
