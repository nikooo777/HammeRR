use std::cell::RefCell;
use std::path::{Path, PathBuf};

use native_runtime::{CaptureEnvironment, ContentMount};
use serde_json::{Value, json};

use super::{Actor, Session};

#[derive(Default)]
pub(super) struct State {
    engine_root: Option<PathBuf>,
    steam_root: Option<PathBuf>,
    cache: RefCell<Option<(u64, Result<CaptureEnvironment, String>)>>,
}

impl State {
    pub(super) fn invalidate(&self) {
        *self.cache.borrow_mut() = None;
    }
}

fn checked_engine(path: &Path) -> Result<PathBuf, String> {
    let root = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if !root.join("cstrike_linux64").is_file() || !root.join("cstrike/gameinfo.txt").is_file() {
        return Err("Native engine root must be the selected Linux CS:S installation".into());
    }
    Ok(root)
}

impl Session {
    #[cfg(test)]
    pub(crate) fn use_test_capture_environment(&self, environment: CaptureEnvironment) {
        *self.native_capture.cache.borrow_mut() = Some((self.content_epoch, Ok(environment)));
    }
    fn resolve_native_capture(
        &self,
        engine: Option<&Path>,
        steam: Option<&Path>,
    ) -> Result<CaptureEnvironment, String> {
        let engine_root = checked_engine(engine.unwrap_or(&self.game.base_dir))?;
        let environment = CaptureEnvironment {
            engine_root,
            steam_root: steam.map(Path::to_owned).unwrap_or_default(),
            mounts: self
                .game
                .mounts
                .iter()
                .map(|mount| ContentMount {
                    ids: mount.ids.clone(),
                    path: mount.mount.location().to_owned(),
                })
                .collect(),
        };
        let environment =
            native_runtime::resolve_environment(&environment).map_err(|e| e.to_string())?;
        native_runtime::validate_environment(&environment).map_err(|e| e.to_string())?;
        Ok(environment)
    }

    pub(super) fn capture_environment(&self) -> Result<CaptureEnvironment, String> {
        if let Some((epoch, cached)) = self.native_capture.cache.borrow().as_ref()
            && *epoch == self.content_epoch
        {
            return cached.clone();
        }
        let resolved = self.resolve_native_capture(
            self.native_capture.engine_root.as_deref(),
            self.native_capture.steam_root.as_deref(),
        );
        *self.native_capture.cache.borrow_mut() = Some((self.content_epoch, resolved.clone()));
        resolved
    }

    pub fn native_capture_runtime(&self) -> Value {
        let environment = self.capture_environment();
        json!({
            "configured_engine_root":self.native_capture.engine_root,
            "configured_steam_root":self.native_capture.steam_root,
            "engine_root":environment.as_ref().ok().map(|e|&e.engine_root),
            "steam_root":environment.as_ref().ok().map(|e|&e.steam_root),
            "steam_root_source":if self.native_capture.steam_root.is_some(){"configured"}else{"running_steam_client"},
            "ready":environment.is_ok(),
            "native_activity":self.native_activity_status(),
            "reason":environment.as_ref().err(),
            "editable":!self.compiling()&&!self.closing,
            "channel":if self.compile_options().lighting == compiler::LightingMode::DirectLdrHdr {"ldr_hdr"}else{"ldr"},
            "supported_channels":["ldr","hdr"], "default_sample_size":128, "maximum_sample_size":128,
        })
    }

    pub fn configure_native_capture_runtime(
        &mut self,
        actor: Actor,
        engine_root: Option<&Path>,
        steam_root: Option<&Path>,
    ) -> Result<Value, String> {
        if self.compiling() || self.closing {
            return Err(
                "Wait for the active build to finish before changing the native runtime".into(),
            );
        }
        let engine_root = engine_root.map(checked_engine).transpose()?;
        let steam_root = steam_root
            .map(|path| path.canonicalize().map_err(|e| e.to_string()))
            .transpose()?;
        if engine_root != self.native_capture.engine_root
            || steam_root != self.native_capture.steam_root
        {
            self.resolve_native_capture(engine_root.as_deref(), steam_root.as_deref())?;
            self.native_capture.engine_root = engine_root;
            self.native_capture.steam_root = steam_root;
            *self.native_capture.cache.borrow_mut() = None;
            self.content_epoch = self.content_epoch.wrapping_add(1);
            self.log(actor, "Changed native reflection capture runtime");
        }
        Ok(self.native_capture_runtime())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_both_overrides_validates_default_before_mutation() {
        let (directory, game) = crate::test_assets::game();
        let mut session = Session::new(Ok(game));
        let engine = directory.path().join("previous-engine");
        let steam = directory.path().join("previous-steam");
        session.native_capture.engine_root = Some(engine.clone());
        session.native_capture.steam_root = Some(steam.clone());
        let epoch = session.content_epoch;
        assert!(
            session
                .configure_native_capture_runtime(Actor::Agent, None, None)
                .is_err()
        );
        assert_eq!(session.native_capture.engine_root.as_ref(), Some(&engine));
        assert_eq!(session.native_capture.steam_root.as_ref(), Some(&steam));
        assert_eq!(session.content_epoch, epoch);
    }

    #[test]
    fn repeated_descriptor_reads_are_cached_and_content_changes_retry() {
        let (_directory, game) = crate::test_assets::game();
        let mut session = Session::new(Ok(game));
        let descriptor = session.native_capture_runtime();
        assert_eq!(descriptor["ready"], false);
        *session.native_capture.cache.borrow_mut() =
            Some((session.content_epoch, Err("cached result".into())));
        assert_eq!(session.native_capture_runtime()["reason"], "cached result");
        assert_eq!(session.native_capture_runtime()["reason"], "cached result");
        session.reload_material_assets(Actor::User);
        assert_ne!(session.native_capture_runtime()["reason"], "cached result");
        *session.native_capture.cache.borrow_mut() =
            Some((session.content_epoch, Err("cached again".into())));
        session.replace_document(map_core::Document::empty(), None);
        assert_ne!(session.native_capture_runtime()["reason"], "cached again");
    }

    #[test]
    fn native_configuration_is_frozen_during_active_jobs() {
        let (_directory, game) = crate::test_assets::game();
        let mut session = Session::new(Ok(game));
        session.compiling = true;
        assert!(
            session
                .configure_native_capture_runtime(Actor::User, None, None)
                .is_err()
        );
        assert_eq!(session.native_capture_runtime()["editable"], false);
        session.compiling = false;
        session.closing = true;
        assert!(
            session
                .configure_native_capture_runtime(Actor::Agent, None, None)
                .is_err()
        );
    }

    #[test]
    fn runtime_support_and_selected_channels_follow_shared_lighting_without_editing_the_map() {
        let (_directory, game) = crate::test_assets::game();
        let mut session = Session::new(Ok(game));
        let source = session.document().vmf().clone();
        let initial = session.native_capture_runtime();
        assert_eq!(initial["supported_channels"], json!(["ldr", "hdr"]));
        assert_eq!(initial["channel"], "ldr");
        session
            .set_compile_settings(Actor::Agent, "direct_ldr_hdr")
            .unwrap();
        let dual = session.native_capture_runtime();
        assert_eq!(dual["channel"], "ldr_hdr");
        assert_eq!(dual["ready"], initial["ready"]);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), 0);
    }
}
