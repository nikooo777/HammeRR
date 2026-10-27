use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

const IPC_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_REPLY_BYTES: u64 = 8192;
const SILENT_RULE: &str = r#"/eval if _G.hammerr_background_display_v1 == nil then _G.hammerr_background_display_v1 = hl.window_rule({ name = 'hammerr-background-display-v1', match = { title = '(?s)^HammeRR background: .* - HammeRR$' }, no_initial_focus = true }) end; _G.hammerr_background_display_v1:set_enabled(true); assert(_G.hammerr_background_display_v1:is_enabled())"#;

pub(super) async fn prepare_silent_show() -> Result<(), String> {
    let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .map_err(|_| "Silent document display requires Hyprland with Lua window rules")?;
    if signature.is_empty()
        || !signature
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || b"_.-".contains(&value))
    {
        return Err("Invalid Hyprland instance identity for silent display".into());
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .ok_or("Silent document display requires the Hyprland runtime directory")?;
    let socket = PathBuf::from(runtime)
        .join("hypr")
        .join(signature)
        .join(".socket.sock");
    let result = tokio::time::timeout(IPC_TIMEOUT, install_rule(socket))
        .await
        .map_err(|_| "Hyprland silent-display admission timed out")?;
    result.map_err(|error| {
        format!("Silent document display was refused before showing the window: {error}")
    })
}

async fn install_rule(socket: PathBuf) -> Result<(), String> {
    let mut stream = tokio::net::UnixStream::connect(socket)
        .await
        .map_err(|error| error.to_string())?;
    stream
        .write_all(SILENT_RULE.as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    stream.shutdown().await.map_err(|error| error.to_string())?;
    let mut response = Vec::new();
    stream
        .take(MAX_REPLY_BYTES + 1)
        .read_to_end(&mut response)
        .await
        .map_err(|error| error.to_string())?;
    if response.len() as u64 > MAX_REPLY_BYTES {
        return Err("Hyprland silent-display response exceeded its byte limit".into());
    }
    let response = std::str::from_utf8(&response)
        .map_err(|_| "Hyprland returned an invalid silent-display response")?;
    if response.trim() != "ok" {
        return Err(format!(
            "Hyprland did not admit the background window rule: {response}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn silent_display_requires_the_exact_bounded_compositor_acknowledgement() {
        for (reply, admitted) in [
            (b"ok".to_vec(), true),
            (b"error: no Lua window rules".to_vec(), false),
            (Vec::new(), false),
            (vec![b'x'; MAX_REPLY_BYTES as usize + 1], false),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let socket = directory.path().join("compositor.sock");
            let listener = tokio::net::UnixListener::bind(&socket).unwrap();
            let compositor = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                stream.read_to_end(&mut request).await.unwrap();
                assert_eq!(request, SILENT_RULE.as_bytes());
                stream.write_all(&reply).await.unwrap();
            });
            assert_eq!(install_rule(socket).await.is_ok(), admitted);
            compositor.await.unwrap();
        }
    }
}
