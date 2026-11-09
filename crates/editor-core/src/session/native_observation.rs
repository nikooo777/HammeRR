use std::path::PathBuf;
use std::sync::Arc;

use compiler::Control;

use super::{Session, Snapshot, baked_build};
use crate::runtime::native_activity;

pub(super) const MAX_METADATA_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct Frame {
    snapshot: Snapshot,
    path: Option<PathBuf>,
    content: u64,
    build: u64,
    options: compiler::Options,
}

impl Frame {
    pub(super) fn verify(&self, session: &Session) -> Result<(), String> {
        if session.closing
            || !session.current_snapshot(&self.snapshot)
            || session.path != self.path
            || session.content_epoch != self.content
            || session.compile_options() != self.options
            || session
                .compiled
                .as_ref()
                .is_none_or(|build| build.lightmap_identity.token != self.build)
        {
            return Err(
                "The document, build, settings or content changed since this native observation"
                    .into(),
            );
        }
        Ok(())
    }
}

pub(super) struct Capture {
    pub(super) frame: Frame,
    pub(super) build: Arc<baked_build::RetainedBuild>,
    pub(super) target: native_activity::Target,
    pub(super) environment: native_runtime::CaptureEnvironment,
}

pub(super) fn capture(session: &Session) -> Result<Capture, String> {
    session.ensure_not_closing()?;
    session.native_cleanup_result()?;
    if !cfg!(target_os = "linux") {
        return Err("Native observation requires the supported Linux CS:S client".into());
    }
    if session.compiling() {
        return Err("Wait for the active build before native observation".into());
    }
    if session.document.is_dirty() || !std::ptr::eq(session.render_document(), session.document()) {
        return Err(
            "Save and compile the map, then finish its geometry draft before native observation"
                .into(),
        );
    }
    let build_state = baked_build::build_state(session);
    if build_state["current"] != true {
        return Err(format!(
            "Native observation needs a current saved build: {}",
            build_state["diagnostics"]
        ));
    }
    let compiled = session
        .compiled
        .as_ref()
        .ok_or("Compile the saved map first")?;
    let source = compiled
        .lightmap_identity
        .source
        .as_ref()
        .map_err(Clone::clone)?;
    if source
        .as_ref()
        .and_then(compiler::FileIdentity::sha256)
        .is_none()
    {
        return Err("Save and compile the map with a captured source hash first".into());
    }
    let build = baked_build::RetainedBuild::capture(compiled)?;
    if build.output.sha256().is_none() {
        return Err("The compiled output has no captured content hash".into());
    }
    let frame = Frame {
        snapshot: compiled.snapshot.clone(),
        path: session.path.clone(),
        content: session.content_epoch,
        build: compiled.lightmap_identity.token,
        options: session.compile_options(),
    };
    let environment = session.capture_environment()?;
    let target = native_activity::Target::capture(&environment)?;
    Ok(Capture {
        frame,
        build,
        target,
        environment,
    })
}

pub(super) fn verify(
    frame: &Frame,
    build: &baked_build::RetainedBuild,
    target: &native_activity::Target,
    session: &Session,
    control: &Control,
) -> Result<(), String> {
    control.checkpoint()?;
    frame.verify(session)?;
    target.verify()?;
    build.verify_metadata(control, MAX_METADATA_BYTES)
}
