use super::*;

#[derive(Default)]
pub(super) struct State {
    pub(super) primitives: primitives::State,
    pub(super) repeat: repeat::State,
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
        self.primitives
            .preview
            .as_ref()
            .map(|preview| &preview.document)
            .or_else(|| {
                self.repeat
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
    Paste,
    Repeat,
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
    pub(super) fn clear_geometry_previews_except(&mut self, retained: &[Kind]) {
        if !retained.contains(&Kind::Paste) {
            self.clear_paste_preview();
        }
        if !retained.contains(&Kind::Repeat) {
            self.clear_repeat_preview();
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
            self.cancel_face_preview();
        }
        if !retained.contains(&Kind::Paint) {
            self.clear_paint_stroke();
        }
    }
}
