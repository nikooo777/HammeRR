use std::io::Read;
use std::path::Path;

use serde_json::{Value, json};
use source_formats::bsp::{Bsp, LUMP_COUNT};

use super::Session;

pub(super) const MAX_BSP_BYTES: u64 = 128 * 1024 * 1024;
const BSP_HEADER_AND_PADDING_BYTES: usize = 12 + LUMP_COUNT * 19 + 4;
const GAME_LUMP_DIRECTORY_BYTES: usize = 16;

pub(super) struct BuildIdentity {
    pub(super) token: u64,
    pub(super) output: Result<compiler::FileIdentity, String>,
    pub(super) source: Result<Option<compiler::FileIdentity>, String>,
}

pub(super) fn capture_output_identity(
    output: &Path,
    expected: Option<&Bsp>,
    maximum_bytes: u64,
    purpose: &str,
) -> Result<compiler::FileIdentity, String> {
    let expected = expected.ok_or("Compiler BSP bytes are unavailable")?;
    let overhead = expected
        .game_lumps
        .len()
        .checked_mul(GAME_LUMP_DIRECTORY_BYTES)
        .and_then(|bytes| bytes.checked_add(BSP_HEADER_AND_PADDING_BYTES));
    let upper_bound = expected
        .lumps
        .iter()
        .map(|lump| lump.data.len())
        .chain(expected.game_lumps.iter().map(|lump| lump.data.len()))
        .try_fold(overhead.unwrap_or(usize::MAX), usize::checked_add);
    if upper_bound.is_none_or(|bytes| bytes as u64 > maximum_bytes)
        || std::fs::metadata(output)
            .map_err(|error| error.to_string())?
            .len()
            > maximum_bytes
    {
        return Err(format!(
            "BSP exceeds the {maximum_bytes}-byte {purpose} budget"
        ));
    }
    let identity = compiler::FileIdentity::capture_bounded(
        output,
        maximum_bytes,
        &compiler::Control::default(),
    )
    .map_err(|error| error.to_string())?;
    identity
        .verify_bytes(&expected.to_bytes())
        .map_err(|error| error.to_string())?;
    identity
        .verify_metadata()
        .map_err(|error| error.to_string())?;
    Ok(identity)
}

impl BuildIdentity {
    pub(super) fn source(
        path: &Path,
        control: &compiler::Control,
    ) -> Result<compiler::FileIdentity, String> {
        let metadata = std::fs::metadata(path)
            .map_err(|error| format!("Saved VMF source is missing or unreadable: {error}"))?;
        if !metadata.is_file() || metadata.len() > MAX_BSP_BYTES {
            return Err(format!(
                "Saved VMF source must be a regular file within the {MAX_BSP_BYTES}-byte inspection budget"
            ));
        }
        compiler::FileIdentity::capture(path, control).map_err(|error| error.to_string())
    }

    pub(super) fn capture(
        token: u64,
        output: &Path,
        source: Result<Option<compiler::FileIdentity>, String>,
        expected: Option<&Bsp>,
    ) -> Self {
        let identity = capture_output_identity(output, expected, MAX_BSP_BYTES, "inspection");
        Self {
            token,
            output: identity,
            source,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum SourceVerification {
    Resolved,
    OriginalMetadata { maximum_bytes: usize },
}

impl SourceVerification {
    fn error(self, error: &str) -> String {
        match self {
            Self::Resolved => error.to_owned(),
            Self::OriginalMetadata { .. } => bounded_message(error),
        }
    }
}

pub(super) const MAX_DIAGNOSTIC_BYTES: usize = 512;
const TRUNCATION_SUFFIX: &str = " [truncated]";

pub(super) fn bounded_message(message: &str) -> String {
    if message.len() <= MAX_DIAGNOSTIC_BYTES {
        return message.to_owned();
    }
    let mut end = MAX_DIAGNOSTIC_BYTES - TRUNCATION_SUFFIX.len();
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATION_SUFFIX}", &message[..end])
}

pub(super) fn build_state(session: &Session) -> Value {
    build_state_with_mode(
        session,
        SourceVerification::Resolved,
        &compiler::Control::default(),
    )
}

pub(super) fn build_state_controlled(
    session: &Session,
    mode: SourceVerification,
    control: &compiler::Control,
) -> Result<Value, String> {
    control.checkpoint()?;
    let state = build_state_with_mode(session, mode, control);
    control.checkpoint()?;
    Ok(state)
}

fn build_state_with_mode(
    session: &Session,
    mode: SourceVerification,
    control: &compiler::Control,
) -> Value {
    let Some(build) = &session.compiled else {
        return json!({"status":"unavailable", "current":false, "diagnostics":["Compile the map successfully in this session before inspecting baked lightmaps"]});
    };
    let identity = &build.lightmap_identity;
    let pending_preview = !std::ptr::eq(session.render_document(), session.document());
    let mut diagnostics = Vec::new();
    if !session.current_snapshot(&build.snapshot) {
        diagnostics.push("The map changed since this successful build".to_owned());
    }
    if build.options != session.compile_options() {
        diagnostics.push("Build settings changed since this successful build".to_owned());
    }
    if build.content_epoch != session.content_epoch {
        diagnostics
            .push("Mounted content or capture configuration changed since this build".to_owned());
    }
    if let Some(error) = session.instance_dependency_issue(
        build.instance_dependencies.as_ref(),
        &build.instance_source_path,
    ) {
        diagnostics.push(format!(
            "Instance source/dependencies changed or cannot be verified: {error}"
        ));
    }
    let asset_issue = match mode {
        SourceVerification::Resolved => session
            .static_prop_source_issue(build.static_prop_sources.as_ref())
            .or_else(|| material_source_issue(build.material_sources.as_ref(), control)),
        SourceVerification::OriginalMetadata { .. } => {
            if build
                .static_prop_sources
                .as_ref()
                .is_some_and(|sources| sources.content_epoch != session.content_epoch)
            {
                Some("Content changed since static-prop compilation; compile again".into())
            } else {
                asset_source_issue(build, mode, control)
            }
        }
    };
    if let Some(error) = asset_issue {
        diagnostics.push(error);
    }
    if session.compiling() {
        diagnostics
            .push("A new build is running; this is the previous successful build".to_owned());
    }
    let output_issue = match &identity.output {
        Ok(output) => output
            .verify_metadata()
            .err()
            .map(|error| error.to_string()),
        Err(error) => Some(mode.error(error)),
    };
    if let Some(issue) = &output_issue {
        diagnostics.push(format!(
            "Compiled BSP changed, is missing, or cannot be verified: {issue}"
        ));
    }
    match &identity.source {
        Ok(Some(source)) => {
            if let Err(error) = source.verify_metadata() {
                diagnostics.push(format!("Saved VMF source changed or is missing: {error}"));
            }
            if session
                .path
                .as_ref()
                .and_then(|path| path.canonicalize().ok())
                .as_deref()
                != Some(source.path())
            {
                diagnostics.push("The source map path changed since this build".to_owned());
            }
        }
        Err(error) => diagnostics.push(format!(
            "Saved VMF source identity unavailable: {}",
            mode.error(error)
        )),
        Ok(None) if session.path.is_some() => {
            diagnostics.push("The map was saved to a new source path after this build".to_owned())
        }
        Ok(None) => {}
    }
    let source = identity
        .source
        .as_ref()
        .ok()
        .and_then(|source| source.as_ref())
        .map(compiler::FileIdentity::path);
    let paths_admitted = std::iter::once(build.path.as_path())
        .chain(source)
        .all(|path| crate::report_path::checked(path).is_ok());
    if !paths_admitted {
        diagnostics.push("Build/source path cannot be represented as UTF-8 JSON or exceeds the inspection metadata path budget".into());
    }
    if matches!(mode, SourceVerification::OriginalMetadata { .. }) {
        for diagnostic in &mut diagnostics {
            *diagnostic = bounded_message(diagnostic);
        }
    }
    json!({
        "status":if output_issue.is_some() {"unavailable"} else if diagnostics.is_empty() {"current"} else {"stale"},
        "current":diagnostics.is_empty(), "token":identity.token.to_string(),
        "render_context":{"pending_preview":pending_preview,"current_for_display":diagnostics.is_empty() && !pending_preview,"diagnostics":if pending_preview {vec!["The displayed geometry/face draft was not baked; this inspection uses the committed build snapshot"]} else {Vec::<&str>::new()}},
        "bsp":if paths_admitted {Some(&build.path)} else {None}, "generation":build.snapshot.generation, "revision":build.snapshot.revision,
        "lighting":build.options.lighting.name(), "diagnostics":diagnostics,
        "source":if paths_admitted {source} else {None},
    })
}

pub(super) fn read_owned_bytes(identity: &compiler::FileIdentity) -> Result<Vec<u8>, String> {
    read_owned_bytes_controlled(identity, &compiler::Control::default())
}

pub(super) fn read_owned_bytes_controlled(
    identity: &compiler::FileIdentity,
    control: &compiler::Control,
) -> Result<Vec<u8>, String> {
    control.checkpoint()?;
    identity
        .verify_metadata()
        .map_err(|error| error.to_string())?;
    if std::fs::metadata(identity.path())
        .map_err(|error| error.to_string())?
        .len()
        > MAX_BSP_BYTES
    {
        return Err(format!(
            "BSP exceeds the {MAX_BSP_BYTES}-byte inspection limit"
        ));
    }
    let mut file = std::fs::File::open(identity.path()).map_err(|error| error.to_string())?;
    let length = file.metadata().map_err(|error| error.to_string())?.len();
    if length > MAX_BSP_BYTES {
        return Err(format!(
            "BSP exceeds the {MAX_BSP_BYTES}-byte inspection limit"
        ));
    }
    let mut bytes = Vec::new();
    let length =
        usize::try_from(length).map_err(|_| "BSP inspection length exceeds the address space")?;
    bytes
        .try_reserve_exact(length)
        .map_err(|_| "BSP inspection buffer cannot be allocated")?;
    bytes.resize(length, 0);
    for chunk in bytes.chunks_mut(64 * 1024) {
        control.checkpoint()?;
        file.read_exact(chunk).map_err(|error| error.to_string())?;
    }
    if file
        .read(&mut [0u8; 1])
        .map_err(|error| error.to_string())?
        != 0
    {
        return Err("BSP grew beyond the inspection limit".into());
    }
    identity
        .verify_bytes(&bytes)
        .map_err(|error| error.to_string())?;
    identity
        .verify_metadata()
        .map_err(|error| error.to_string())?;
    control.checkpoint()?;
    Ok(bytes)
}

fn asset_source_issue(
    build: &super::CompiledMap,
    mode: SourceVerification,
    control: &compiler::Control,
) -> Option<String> {
    match mode {
        SourceVerification::Resolved => build
            .static_prop_sources
            .as_ref()
            .and_then(|sources| sources.verify(control).err())
            .or_else(|| material_source_issue(build.material_sources.as_ref(), control)),
        SourceVerification::OriginalMetadata { maximum_bytes } => {
            for guard in build
                .static_prop_sources
                .as_ref()
                .map(|sources| &sources.guard)
                .into_iter()
                .chain(build.prop_lighting_guard.as_ref())
                .chain(
                    build
                        .material_sources
                        .as_ref()
                        .map(|sources| &sources.guard),
                )
            {
                if let Err(error) = guard.verify_original_metadata(control, maximum_bytes) {
                    return Some(format!(
                        "Original tracked asset metadata changed or cannot be verified: {error}"
                    ));
                }
            }
            None
        }
    }
}

pub(super) fn source_issue_controlled(
    build: &super::CompiledMap,
    control: &compiler::Control,
) -> Option<String> {
    source_issue_with_mode(build, SourceVerification::Resolved, control)
}

pub(super) fn source_issue_with_mode(
    build: &super::CompiledMap,
    mode: SourceVerification,
    control: &compiler::Control,
) -> Option<String> {
    if let Some(error) = asset_source_issue(build, mode, control) {
        return Some(error);
    }
    if let Some(dependencies) = &build.instance_dependencies {
        dependencies
            .verify(control)
            .err()
            .map(|error| format!("Instance source/dependencies changed: {error}"))
    } else if let Ok(Some(source)) = &build.lightmap_identity.source {
        source
            .verify(control)
            .err()
            .map(|error| format!("Saved VMF source changed: {error}"))
    } else {
        None
    }
}

pub(super) fn source_issue(build: &super::CompiledMap) -> Option<String> {
    if let Some(sources) = &build.static_prop_sources
        && let Err(error) = sources.verify(&compiler::Control::default())
    {
        return Some(error);
    }
    if let Some(error) = material_source_issue(
        build.material_sources.as_ref(),
        &compiler::Control::default(),
    ) {
        return Some(error);
    }
    if let Some(dependencies) = &build.instance_dependencies {
        dependencies
            .verify(&compiler::Control::default())
            .err()
            .map(|error| format!("Instance source/dependencies changed: {error}"))
    } else if let Ok(Some(source)) = &build.lightmap_identity.source {
        read_owned_bytes(source)
            .err()
            .map(|error| format!("Saved VMF source changed: {error}"))
    } else {
        None
    }
}

pub(super) fn material_source_issue(
    sources: Option<&super::MaterialSources>,
    control: &compiler::Control,
) -> Option<String> {
    sources.and_then(|sources| sources.verify(control).err())
}

pub(super) struct Recheck {
    pub stale: Vec<String>,
    pub unavailable: Option<String>,
}

pub(super) fn recheck(build: &super::CompiledMap) -> Recheck {
    recheck_with_mode(
        build,
        SourceVerification::Resolved,
        &compiler::Control::default(),
    )
}

pub(super) fn recheck_with_mode(
    build: &super::CompiledMap,
    mode: SourceVerification,
    control: &compiler::Control,
) -> Recheck {
    let mut stale = Vec::new();
    if let Some(error) = asset_source_issue(build, mode, control) {
        stale.push(error);
    }
    if let Some(error) = build
        .instance_dependencies
        .as_ref()
        .and_then(|dependencies| dependencies.verify_metadata().err())
    {
        stale.push(format!(
            "Instance source/dependencies changed during inspection: {error}"
        ));
    }
    if let Ok(Some(source)) = &build.lightmap_identity.source
        && let Err(error) = source.verify_metadata()
    {
        stale.push(format!(
            "Saved VMF source changed during inspection: {error}"
        ));
    }
    let unavailable = build
        .lightmap_identity
        .output
        .as_ref()
        .ok()
        .and_then(|output| output.verify_metadata().err())
        .map(|error| format!("Compiled BSP changed during inspection: {error}"));
    Recheck { stale, unavailable }
}

pub(super) fn decoded_bsp_bound(bytes: &[u8]) -> Result<usize, String> {
    fn word(bytes: &[u8], offset: usize) -> Result<usize, String> {
        let raw: [u8; 4] = bytes
            .get(offset..offset + 4)
            .ok_or("BSP inspection header is truncated")?
            .try_into()
            .unwrap();
        usize::try_from(i32::from_le_bytes(raw))
            .map_err(|_| "BSP inspection range is negative".into())
    }
    if bytes.len() as u64 > MAX_BSP_BYTES || bytes.len() < 1036 || &bytes[..4] != b"VBSP" {
        return Err("BSP inspection header or file size is unsupported".into());
    }
    let mut decoded = size_of::<Bsp>() + LUMP_COUNT * size_of::<source_formats::bsp::Lump>();
    for index in 0..LUMP_COUNT {
        let base = 8 + index * 16;
        let offset = word(bytes, base)?;
        let size = word(bytes, base + 4)?;
        let end = offset
            .checked_add(size)
            .filter(|&end| end <= bytes.len())
            .ok_or("BSP inspection lump range is invalid")?;
        decoded = decoded
            .checked_add(size)
            .ok_or("BSP inspection allocation overflow")?;
        if index == source_formats::bsp::lump::GAME_LUMP && size != 0 {
            let count = word(bytes, offset)?;
            if count > 4096
                || count
                    .checked_mul(16)
                    .and_then(|size| size.checked_add(offset + 4))
                    .is_none_or(|directory_end| directory_end > end)
            {
                return Err("BSP inspection game-lump directory exceeds its bounds".into());
            }
            decoded = decoded
                .checked_add(count * size_of::<source_formats::bsp::GameLump>())
                .ok_or("BSP inspection game-lump allocation overflow")?;
            for entry in 0..count {
                let base = offset + 4 + entry * 16;
                let data_offset = word(bytes, base + 8)?;
                let data_size = word(bytes, base + 12)?;
                if data_offset
                    .checked_add(data_size)
                    .is_none_or(|end| end > bytes.len())
                {
                    return Err("BSP inspection game-lump range is invalid".into());
                }
                decoded = decoded
                    .checked_add(data_size)
                    .ok_or("BSP inspection allocation overflow")?;
            }
        }
    }
    if decoded as u64 > MAX_BSP_BYTES {
        return Err("BSP decoded inspection allocation exceeds 128 MiB".into());
    }
    Ok(decoded)
}
