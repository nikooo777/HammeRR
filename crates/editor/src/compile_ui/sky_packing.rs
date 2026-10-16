use serde_json::Value;
use slint::ComponentHandle;

use super::{Actor, CompileWindow, Session, SharedSession, lock};
use crate::session::compilation::BuildSettingsUpdate;

pub(super) fn refresh(window: &CompileWindow, session: &Session) {
    window.set_pack_sky_assets(session.compile_options().pack_sky_assets);
}

fn change(window: &CompileWindow, session: &mut Session, enabled: bool) {
    let result = session.update_build_settings(
        Actor::User,
        BuildSettingsUpdate {
            pack_sky_assets: Some(enabled),
            ..Default::default()
        },
    );
    refresh(window, session);
    window.set_message(match result {
        Ok(_) if enabled => "Sky asset packing enabled. The next Compile or Run Map packs the selected supported sky family; missing or unsupported assets stop the build.".into(),
        Ok(_) => "Sky asset packing disabled.".into(),
        Err(error) => error.into(),
    });
}

pub(super) fn bind(window: &CompileWindow, shared: &SharedSession) {
    window.on_change_pack_sky_assets({
        let (window, shared) = (window.as_weak(), shared.clone());
        move |enabled| change(&window.unwrap(), &mut lock(&shared), enabled)
    });
}

pub(super) fn details(stats: &Value) -> String {
    let packed = &stats["sky_assets"];
    let (Some(family), Some(files), Some(bytes)) = (
        packed["family"].as_str(),
        packed["files"].as_u64(),
        packed["asset_bytes"].as_u64(),
    ) else {
        return String::new();
    };
    let channels = match packed["channels"].as_str() {
        Some("ldr") => "LDR",
        Some("ldr_hdr") => "LDR + HDR",
        _ => return String::new(),
    };
    format!("\nPacked sky assets: {family} · {channels} · {files} files · {bytes} bytes")
}

#[cfg(test)]
mod tests;
