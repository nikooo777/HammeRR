use std::io;

use regex::Regex;
use source_formats::{
    bsp::{self, Bsp},
    vmf::Vmf,
};

use crate::util::invalid;

const CONTROLLER: &str = "\n{\n\"classname\" \"logic_script\"\n\"targetname\" \"__hammerr_capture_controller\"\n\"vscripts\" \"hammerr_capture.nut\"\n\"thinkfunction\" \"HammeRRCaptureThink\"\n}\n";

fn dangerous_class(class: &str) -> bool {
    matches!(
        class.to_ascii_lowercase().as_str(),
        "logic_script"
            | "point_servercommand"
            | "point_clientcommand"
            | "point_broadcastclientcommand"
    )
}

fn dangerous_input(input: &str) -> bool {
    let input = input.trim().to_ascii_lowercase();
    input.starts_with("runscript")
        || input.contains("consolecommand")
        || matches!(
            input.as_str(),
            "command" | "callscriptfunction" | "setthinkfunction"
        )
}

pub fn preflight_vmf(vmf: &Vmf) -> io::Result<()> {
    if vmf.world.properties.iter().any(|(key, value)| {
        matches!(
            key.to_ascii_lowercase().as_str(),
            "vscripts" | "thinkfunction"
        ) && !value.trim().is_empty()
    }) {
        return Err(invalid(format!(
            "world:{} cannot participate in private reflection capture: authored scripts are unsupported",
            vmf.world.id
        )));
    }
    for entity in vmf.entities.iter().filter(|e| !e.hidden) {
        let unsafe_property = entity.properties.iter().any(|(key, value)| {
            matches!(
                key.to_ascii_lowercase().as_str(),
                "vscripts" | "thinkfunction"
            ) && !value.trim().is_empty()
        });
        let unsafe_connection = entity
            .connections
            .iter()
            .flatten()
            .any(|connection| connection.fields().nth(1).is_some_and(dangerous_input));
        if dangerous_class(entity.classname().unwrap_or(""))
            || unsafe_property
            || unsafe_connection
            || entity
                .properties
                .get("targetname")
                .is_some_and(|v| v.starts_with("__hammerr_capture_"))
        {
            return Err(invalid(format!(
                "entity:{} cannot participate in private reflection capture: authored scripts or console-command inputs are unsupported",
                entity.id
            )));
        }
    }
    Ok(())
}

pub fn capture_staging_bsp(bytes: &[u8]) -> io::Result<Vec<u8>> {
    capture_staging_bsp_for_channel(bytes, crate::CaptureChannel::Ldr)
}

pub fn capture_staging_bsp_for_channel(
    bytes: &[u8],
    channel: crate::CaptureChannel,
) -> io::Result<Vec<u8>> {
    if bytes.len() > crate::util::MAX_BSP {
        return Err(invalid("Capture BSP exceeds 128 MiB"));
    }
    let mut bsp = Bsp::parse(bytes)?;
    let entities = bsp.entities()?;
    if entities.len() > 4 * 1024 * 1024 {
        return Err(invalid("Capture entity text exceeds 4 MiB"));
    }
    let executable = Regex::new(r#"(?i)"classname"\s*"(?:logic_script|point_servercommand|point_clientcommand|point_broadcastclientcommand)"|\b(?:runscript\w*|consolecommand|callscriptfunction|setthinkfunction)\b|[,\x1b]command[,\x1b]|__hammerr_capture_"#).map_err(io::Error::other)?;
    let script_fields =
        Regex::new(r#"(?i)"(?:vscripts|thinkfunction)"\s*"([^"]*)""#).map_err(io::Error::other)?;
    if executable.is_match(entities)
        || script_fields
            .captures_iter(entities)
            .any(|capture| !capture[1].trim().is_empty())
    {
        return Err(invalid(
            "Compiled entities contain unsupported authored scripts or console commands; inspect the source entity diagnostics",
        ));
    }
    if channel == crate::CaptureChannel::Hdr
        && [
            bsp::lump::LIGHTING_HDR,
            bsp::lump::WORLDLIGHTS_HDR,
            bsp::lump::LEAF_AMBIENT_LIGHTING_HDR,
        ]
        .iter()
        .any(|&index| bsp.lump(index).data.is_empty())
    {
        return Err(invalid(
            "HDR reflection capture requires complete native HDR lighting, world-light and ambient arrays",
        ));
    }
    let mut entities = entities.to_owned();
    entities.push_str(CONTROLLER);
    bsp.set_entities(&entities);
    for index in [
        bsp::lump::FACES,
        bsp::lump::LIGHTING_HDR,
        bsp::lump::LEAF_AMBIENT_LIGHTING_HDR,
    ] {
        bsp.lumps[index].version = 1;
    }
    let result = bsp.to_bytes();
    if result.len() > crate::util::MAX_BSP {
        return Err(invalid("Staged capture BSP exceeds 128 MiB"));
    }
    Ok(result)
}
