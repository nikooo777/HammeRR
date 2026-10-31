use super::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentCloseReview {
    scope: String,
    generation: u64,
    revision: u64,
    saved: u64,
    path: Option<PathBuf>,
    dirty: bool,
    scene: u64,
    view: u64,
    interaction: u64,
    definitions: u64,
    closing: bool,
    drafts: BTreeMap<&'static str, Value>,
    work: Value,
}

impl DocumentCloseReview {
    pub(super) fn first_draft_except(&self, kind: &str) -> Option<&'static str> {
        self.drafts.keys().copied().find(|draft| *draft != kind)
    }

    pub fn requires_discard(&self) -> bool {
        self.dirty || !self.drafts.is_empty()
    }

    pub fn token(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"document-close-v1");
        digest.update(self.scope.as_bytes());
        if let Some(path) = &self.path {
            let bytes = path.as_os_str().as_encoded_bytes();
            digest.update((bytes.len() as u64).to_le_bytes());
            digest.update(bytes);
        }
        let identity = json!([
            self.generation,
            self.revision,
            self.saved,
            self.path.is_some(),
            self.dirty,
            self.scene,
            self.view,
            self.interaction,
            self.definitions,
            self.closing,
            self.drafts,
            self.work,
        ]);
        digest.update(identity.to_string().as_bytes());
        format!("close-v1:{:x}", digest.finalize())
    }

    pub fn inspection(&self) -> Value {
        json!({
            "review_token": self.token(),
            "unsaved_changes": self.dirty,
            "drafts": self.drafts.keys().collect::<Vec<_>>(),
            "discard_required": self.requires_discard(),
            "work": self.work,
            "closing": self.closing,
            "policy": "Terminal Close discards only reviewed unsaved changes and drafts with consent, then stops owned game/audio and cancels/drains owned work. In-flight admitted requests can require waiting or cancellation and a fresh review. This inspection does not close the document.",
        })
    }
}

impl Session {
    pub fn document_close_review(&self) -> DocumentCloseReview {
        let mut drafts = BTreeMap::new();
        for (kind, state) in [
            ("entity_gallery", self.geometry.entity_gallery.close_draft()),
            (
                "entity_placement",
                self.geometry.entity_placement.close_draft(),
            ),
            ("primitive", self.geometry.primitives.close_draft()),
            ("repeat", self.geometry.repeat.close_draft()),
            ("linked_path", self.geometry.linked_path.close_draft()),
            ("paste", self.geometry.clipboard.close_draft()),
            ("components", self.geometry.components.close_draft()),
            ("merge", self.geometry.merging.close_draft()),
            ("carve", self.geometry.carving.close_draft()),
            ("hollow", self.geometry.hollowing.close_draft()),
            ("paint", self.geometry.paint.close_draft()),
            ("groups", self.groups.close_draft()),
            ("layers", self.layers.close_draft()),
            ("compile_region", self.compile_region.close_draft()),
            ("overlay", self.overlay.close_draft()),
            ("cubemap_bindings", self.cubemap_bindings.close_draft()),
            ("model_choice", self.model_browser.close_draft()),
            ("instance_reference", self.instance_reference.close_draft()),
            (
                "instance_collapse",
                self.geometry.instance_collapse.close_draft(),
            ),
            (
                "instance_extraction",
                self.instance_extraction.close_draft(),
            ),
            ("sound_choice", self.sounds.close_draft()),
            ("particle_choice", self.particles.close_draft()),
            ("surface_placement", self.placement.close_draft()),
            ("measurement", self.measurements.close_draft()),
            ("map_fix", self.checks.close_draft()),
            ("context", self.context.close_draft()),
            ("volume", self.volume.close_draft()),
            ("model_attachment", self.model_attachments.close_draft()),
            ("terrain_stroke", self.terrain_strokes.close_draft()),
            ("terrain_handles", self.terrain_handles.close_draft()),
            ("transform_pivot_pick", self.transform_pivot.close_draft()),
            ("clip_points", self.clip_points_report()),
            (
                "properties",
                self.property_draft
                    .as_ref()
                    .map(|draft| json!([self.management_serial, draft.token])),
            ),
            (
                "connections",
                self.connection_draft
                    .as_ref()
                    .map(|draft| json!([self.connection_serial, draft.token])),
            ),
            (
                "rename",
                self.rename_draft
                    .as_ref()
                    .map(|draft| json!([self.rename_serial, draft.token])),
            ),
            (
                "transform",
                self.geometry
                    .transform_preview
                    .as_ref()
                    .map(|_| json!(true)),
            ),
            (
                "clip",
                self.geometry.clip_preview.as_ref().map(|_| json!(true)),
            ),
            (
                "faces",
                self.geometry
                    .face_preview
                    .as_ref()
                    .map(|preview| json!(preview.token)),
            ),
            (
                "transform_drag",
                self.transform_drag.as_ref().map(|drag| drag.close_draft()),
            ),
            (
                "clip_drag",
                self.clip_drag.as_ref().map(|drag| json!(drag.token)),
            ),
            (
                "area_selection",
                self.area_preview
                    .as_ref()
                    .map(|preview| json!([preview.end.to_array()])),
            ),
            (
                "block",
                self.block_preview
                    .map(|(mins, maxs)| json!([mins.to_array(), maxs.to_array()])),
            ),
        ] {
            if let Some(state) = state {
                drafts.insert(kind, state);
            }
        }
        DocumentCloseReview {
            scope: self.buffer_review_scope.clone(),
            generation: self.document_generation,
            revision: self.document.revision(),
            saved: self.history_save_serial,
            path: self.path.clone(),
            dirty: self.document.is_dirty(),
            scene: self.scene_serial,
            view: self.view_serial,
            interaction: self.interaction_epoch,
            definitions: self.definitions_serial,
            closing: self.closing,
            drafts,
            work: json!({
                "compile": self.compilation.close_work(),
                "launch": {"sequence": self.launch_serial, "pending": self.pending_launch.as_ref().map(|origin| json!({"active": origin.active(), "cancelled": origin.cancelled()}))},
                "game": self.game_run.as_ref().map(game::Run::close_identity),
                "audio": self.sounds.close_playback(),
                "contents": self.instance_contents.close_work(),
                "sky3d": self.sky3d_preview.close_work(),
                "extraction": self.instance_extraction.close_work(),
                "collapse": self.geometry.instance_collapse.close_work(),
                "definitions": self.definitions_load.close_work(),
                "recovery": self.recovery.close_work(),
                "native_cleanup_error": self.native_cleanup_error,
                "native": self.native_activity.as_ref().map(|client| client.close_work()),
            }),
        }
    }

    pub(crate) fn ensure_document_close_review(
        &self,
        review: &DocumentCloseReview,
        discard_changes: bool,
    ) -> Result<(), String> {
        if review != &self.document_close_review() {
            return Err("Map, draft or activity changed since Close was reviewed; review the current document and try again".into());
        }
        if review.requires_discard() && !discard_changes {
            return Err("The document has unsaved changes or pending drafts; save/apply/cancel them, or explicitly discard the reviewed changes".into());
        }
        Ok(())
    }

    pub(crate) fn commit_terminal_closing(&mut self) {
        self.closing = true;
    }
}
