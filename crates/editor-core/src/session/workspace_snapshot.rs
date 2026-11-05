use source_formats::file_publication::BoundDestination;

use super::content_profiles::{Candidate, SourceGuard};
use super::{Actor, Session};
use crate::workspace_snapshot::{CameraSnapshot, DocumentSnapshot, ViewSnapshot};

pub(crate) struct WorkspaceSourceGuard {
    map: BoundDestination,
    content: Option<SourceGuard>,
}

impl WorkspaceSourceGuard {
    pub(crate) fn verify(&self) -> Result<(), String> {
        self.map
            .verify()
            .map_err(|error| format!("{}: {error}", self.map.authored_path().display()))?;
        if let Some(content) = &self.content {
            content.verify()?;
        }
        Ok(())
    }
}

impl Session {
    pub(crate) fn is_initial_workspace_placeholder(&self) -> bool {
        self.path.is_none()
            && self.document_generation == 0
            && self.document.revision() == 0
            && self.document.undo_steps().is_empty()
            && self.document.redo_steps().is_empty()
            && self.document.vmf() == map_core::Document::empty().vmf()
            && !self.instance_workers_busy()
            && self.ensure_content_transition_available(false).is_ok()
    }

    /// Verify the returned source guard after releasing the Session lock.
    pub(crate) fn capture_workspace_document(
        &self,
    ) -> Result<(DocumentSnapshot, WorkspaceSourceGuard), String> {
        self.ensure_not_closing()?;
        if self.path.is_none() {
            return Err("Use Save as for every document before saving the workspace".into());
        }
        if self.document_close_review().requires_discard() {
            return Err(format!(
                "{} has unsaved changes or pending drafts; save, apply or cancel them before saving the workspace",
                self.title()
            ));
        }
        if self.instance_workers_busy() {
            return Err("Wait for active document workers before saving the workspace".into());
        }
        self.ensure_content_transition_available(false)?;
        if self.game_error.is_some() || self.definitions_error.is_some() {
            return Err("Load valid content and definitions before saving the workspace".into());
        }
        let mut content = self
            .current_content_profile_config()
            .ok_or("Current content configuration is unavailable; select content paths first")?;
        content.fgd = Some(
            self.definitions
                .sources
                .last()
                .cloned()
                .ok_or("Workspace restoration requires definitions loaded from an FGD file")?,
        );
        let map = self
            .file_claims
            .source
            .as_ref()
            .ok_or("The named map has no retained source identity; reopen or use Save as")?
            .destination();
        let path = map.physical_path().to_owned();
        let authored = map.authored_path();
        let snapshot = DocumentSnapshot {
            entry: crate::recent_maps::Entry {
                reopen_path: (authored != path).then(|| authored.to_owned()),
                path,
                game_directory: Some(self.game.game_dir.clone()),
                build_settings: content.build.clone(),
                content: Some(content),
            },
            views: ViewSnapshot {
                layout: self.view_layout.layout,
                active_pane: self.view_layout.active,
                linked_orthographic: self.view_layout.linked,
                cameras: self.view_layout.cameras.map(CameraSnapshot::from_camera),
            },
            layout: self.workspace_layout(),
        };
        snapshot.validate()?;
        Ok((snapshot, WorkspaceSourceGuard { map, content: None }))
    }

    pub(crate) fn verify_workspace_document_snapshot(
        &self,
        expected: &DocumentSnapshot,
    ) -> Result<(), String> {
        let (current, _) = self.capture_workspace_document()?;
        if &current != expected {
            return Err(
                "Document path, content, build settings or views changed; inspect the workspace again"
                    .into(),
            );
        }
        Ok(())
    }

    pub(crate) fn prepare_workspace_document(
        &mut self,
        actor: Actor,
        snapshot: &DocumentSnapshot,
        control: &compiler::Control,
    ) -> Result<WorkspaceSourceGuard, String> {
        if !self.is_initial_workspace_placeholder() {
            return Err("Workspace documents must be prepared in a fresh private Session".into());
        }
        snapshot.validate()?;
        control.checkpoint()?;
        let config = snapshot
            .entry
            .content
            .as_ref()
            .ok_or("Saved workspace has no content configuration")?;
        let destination = super::recent::entry_destination(&snapshot.entry)?;
        let claim = self.file_claims.source(destination.clone())?;
        let prepared = super::recent::PreparedMap::load(destination.clone(), control)?;
        let content = Candidate::load(config, control)?;
        if snapshot.entry.game_directory.as_deref() != Some(content.game_directory()) {
            return Err(
                "Saved workspace content directory differs from its retained configuration".into(),
            );
        }
        let guard = WorkspaceSourceGuard {
            map: destination,
            content: Some(content.source_guard()),
        };
        guard.verify()?;
        let camera = snapshot.views.cameras[snapshot.views.active_pane].camera()?;
        let camera = self.replacement_camera(&prepared.document, Some(camera))?;
        control.checkpoint()?;
        self.adopt_document(
            prepared.document,
            Some(guard.map.authored_path().to_owned()),
            camera,
        );
        self.adopt_prepared_content(content, config.clone(), None);
        self.update_recovery_path(prepared.recovery);
        self.file_claims.source = Some(claim);
        self.apply_workspace_layout(snapshot.layout)?;
        self.update_view_layout(actor, Some(snapshot.views.layout), Some(false), None)?;
        for (index, camera) in snapshot.views.cameras.iter().enumerate() {
            self.set_view_pane_camera(actor, index, camera.camera()?, 1.0, None)?;
        }
        self.activate_view_pane(actor, snapshot.views.active_pane, None)?;
        self.update_view_layout(actor, None, Some(snapshot.views.linked_orthographic), None)?;
        guard.verify()?;
        control.checkpoint()?;
        Ok(guard)
    }
}
