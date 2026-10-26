use std::io::{self, Write};

use glam::Vec3;
use render::Camera;
use serde::Serialize;
use serde_json::{Value, json};
use source_formats::bsp::{Bsp, MAX_WORLD_LIGHTS, WorldLight, lump, world_light_kind};

use super::{Actor, Session, baked_build};

pub use super::baked_lightmaps::Channel;

pub const MAX_PAGE_LIGHTS: usize = 64;
pub const MAX_REPORT_BYTES: usize = 256 * 1024;
pub const MAX_CALL_BYTES: usize = 384 * 1024 * 1024;
pub const MAX_METADATA_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_DIAGNOSTICS: usize = 64;
pub const MAX_DIAGNOSTIC_BYTES: usize = baked_build::MAX_DIAGNOSTIC_BYTES;
const FRAME_HALF_EXTENT: f32 = 16.0;
const SOURCE_MODE: baked_build::SourceVerification =
    baked_build::SourceVerification::OriginalMetadata {
        maximum_bytes: MAX_METADATA_BYTES,
    };
const NOTES: &str = "Native 88-byte world-light records in file order. Positions/radius use Hammer units; intensity is stored native RGB, not display brightness or illuminance. Owner/cluster/texinfo are native integers, not authored identities. Available point/spot source associations are captured at compilation and paired with exact channel bytes; historical IDs describe that captured source only. Instance identity records the outer helper, leaf file, original entity and build-local expanded entity, with an ordered captured occurrence path of authored helper IDs and referenced files, from the root helper to the leaf source. Historical paths describe the captured snapshot, not current files. Channel availability does not establish runtime HDR activation. Currentness covers the committed map, captured settings/content epoch and tracked original source guards, not a complete world-material closure. Changed lookup directories conservatively make source evidence stale. Unapplied drafts are not baked; pending_preview describes rendered geometry drafts only.";

#[derive(Debug, Clone)]
pub struct Query {
    pub build_token: Option<String>,
    pub channel: Channel,
    pub offset: usize,
    pub limit: usize,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            build_token: None,
            channel: Channel::Ldr,
            offset: 0,
            limit: MAX_PAGE_LIGHTS,
        }
    }
}

impl Query {
    fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_PAGE_LIGHTS).contains(&self.limit) {
            return Err(format!("Light page limit must be 1–{MAX_PAGE_LIGHTS}"));
        }
        self.offset
            .checked_add(self.limit)
            .ok_or("Light page range overflow")?;
        if let Some(token) = &self.build_token {
            if token.is_empty()
                || token.len() > 20
                || !token.bytes().all(|byte| byte.is_ascii_digit())
                || (token.len() > 1 && token.starts_with('0'))
                || token.parse::<u64>().is_err()
            {
                return Err("Build token must be the returned opaque decimal string".into());
            }
        } else if self.offset != 0 {
            return Err("A retained light page requires the returned build token".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Inspection {
    pub build: Value,
    pub historical: bool,
    pub channel: &'static str,
    pub channels: [ChannelSummary; 2],
    pub page: Page,
    pub diagnostics: Vec<String>,
    pub diagnostics_total: usize,
    pub diagnostics_truncated: bool,
    pub limits: Limits,
    pub notes: &'static str,
}

impl Inspection {
    pub fn frame_origin(&self, ordinal: usize) -> Result<Vec3, String> {
        if self.build["current"] != true || self.build["status"] != "current" {
            return Err("Framing requires a current successful owned build; refresh inspection or compile again".into());
        }
        if !self
            .channels
            .iter()
            .any(|channel| channel.channel == self.channel && channel.status == "available")
        {
            return Err("The requested compiled-light channel was not baked".into());
        }
        let light = self
            .page
            .rows
            .iter()
            .find(|light| light.ordinal == ordinal)
            .ok_or("Compiled light ordinal is outside the native table")?;
        if !matches!(light.kind, world_light_kind::POINT | world_light_kind::SPOT) {
            return Err("Only compiled point and spot light origins can be framed".into());
        }
        let origin = Vec3::from_array(light.origin);
        if !origin.is_finite()
            || origin.abs().max_element() > map_core::checks::SOURCE_COORDINATE_LIMIT as f32
        {
            return Err("Compiled light origin must be finite and inside Source bounds".into());
        }
        Ok(origin)
    }
}

#[derive(Debug, Clone)]
pub struct FramedLight {
    pub inspection: Inspection,
    pub bounds: [[f32; 3]; 2],
    pub camera: Camera,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelSummary {
    pub channel: &'static str,
    pub lump: usize,
    pub status: &'static str,
    pub bytes: Option<usize>,
    pub count: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Page {
    pub offset: usize,
    pub limit: usize,
    pub returned: usize,
    pub total: Option<usize>,
    pub next_offset: Option<usize>,
    pub rows: Vec<WorldLightRow>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SourceAssociation {
    Available {
        origin: compiler::world_light_origins::Origin,
    },
    Unavailable {
        reason: &'static str,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct WorldLightRow {
    pub source_association: SourceAssociation,
    pub ordinal: usize,
    pub kind_name: &'static str,
    pub origin: [f32; 3],
    pub intensity: [f32; 3],
    pub normal: [f32; 3],
    pub cluster: i32,
    pub kind: i32,
    pub style: i32,
    pub stop_dot: f32,
    pub stop_dot2: f32,
    pub exponent: f32,
    pub radius: f32,
    pub constant_attenuation: f32,
    pub linear_attenuation: f32,
    pub quadratic_attenuation: f32,
    pub flags: i32,
    pub texinfo: i32,
    pub owner: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Limits {
    pub bsp_bytes: u64,
    pub decoded_bsp_bytes: u64,
    pub call_bytes: usize,
    pub metadata_bytes: usize,
    pub report_bytes: usize,
    pub source_association_bytes: usize,
    pub page_source_bytes: usize,
    pub occurrence_bytes: usize,
    pub occurrence_depth: usize,
    pub page_lights: usize,
    pub lights_per_channel: usize,
    pub diagnostics: usize,
    pub diagnostic_bytes: usize,
    pub conservative_live_bytes: usize,
}

impl Session {
    pub fn frame_compiled_world_light(
        &mut self,
        actor: Actor,
        build_token: String,
        channel: Channel,
        ordinal: usize,
        aspect: f32,
    ) -> Result<FramedLight, String> {
        self.ensure_not_closing()?;
        if !aspect.is_finite() || !(0.01..=100.0).contains(&aspect) {
            return Err("Viewport aspect must be finite and between 0.01 and 100".into());
        }
        if self.viewport_pick_conflict()
            || self.measurement_picking()
            || self.linked_path_picking()
            || self.block_preview.is_some()
            || self.clip_drag.is_some()
        {
            return Err("Finish or cancel the active viewport gesture or block preview before framing a compiled light".into());
        }
        let inspection = self.inspect_compiled_world_lights(&Query {
            build_token: Some(build_token),
            channel,
            offset: ordinal,
            limit: 1,
        })?;
        let origin = inspection.frame_origin(ordinal)?;
        let mins = origin - Vec3::splat(FRAME_HALF_EXTENT);
        let maxs = origin + Vec3::splat(FRAME_HALF_EXTENT);
        let camera = self.camera.frame_bounds(mins, maxs, aspect);
        if !camera.position.is_finite()
            || !camera.view_projection(aspect).is_finite()
            || !camera.yaw_degrees.is_finite()
            || !camera.pitch_degrees.is_finite()
            || !camera.fov_y_degrees.is_finite()
            || !camera.ortho_height.is_finite()
        {
            return Err("Compiled light cannot be framed with the current camera".into());
        }
        self.set_camera(camera);
        self.log(
            actor,
            format!("framed compiled {} light record {ordinal}", channel.name()),
        );
        Ok(FramedLight {
            inspection,
            bounds: [mins.to_array(), maxs.to_array()],
            camera,
        })
    }

    /// A no-I/O invalidation key; source and output freshness require an inspection.
    pub fn compiled_world_light_observation(&self) -> Value {
        let build = self.compiled.as_ref();
        let path_omitted = self
            .path
            .as_ref()
            .is_some_and(|path| crate::report_path::checked(path).is_err());
        json!({
            "token":build.map(|build| build.lightmap_identity.token.to_string()),
            "generation":self.document_generation,
            "revision":self.document.revision(),
            "content_epoch":self.content_epoch,
            "snapshot_matches":build.is_some_and(|build| self.current_snapshot(&build.snapshot)),
            "settings_match":build.is_some_and(|build| build.options == self.compile_options()),
            "settings_observation":format!("{:?}", self.compile_options()),
            "content_matches":build.is_some_and(|build| build.content_epoch == self.content_epoch),
            "path":if path_omitted {None} else {self.path.as_ref()},
            "path_omitted":path_omitted,
            "compiling":self.compiling(),
            "pending_preview":!std::ptr::eq(self.render_document(), self.document()),
            "render_revision":self.render_document().revision(),
        })
    }

    pub fn compiled_world_light_build_state(&self) -> Value {
        inspection_build_state(self, &compiler::Control::default())
            .unwrap_or_else(|error| json!({"status":"unavailable","current":false,"token":null,"diagnostics":[error]}))
    }

    pub fn inspect_compiled_world_lights(&self, query: &Query) -> Result<Inspection, String> {
        self.inspect_compiled_world_lights_controlled(query, &compiler::Control::default())
    }

    pub fn inspect_compiled_world_lights_controlled(
        &self,
        query: &Query,
        control: &compiler::Control,
    ) -> Result<Inspection, String> {
        control.checkpoint()?;
        query.validate()?;
        let live_bytes = call_bound(baked_build::MAX_BSP_BYTES as usize)?;
        match (&self.compiled, &query.build_token) {
            (None, Some(_)) => {
                return Err("The requested successful build is no longer retained".into());
            }
            (Some(build), Some(token)) if token != &build.lightmap_identity.token.to_string() => {
                return Err(
                    "Build token differs from the retained successful artifact; refresh inspection"
                        .into(),
                );
            }
            _ => {}
        }
        let mut report = empty_report(query, inspection_build_state(self, control)?, live_bytes);
        let Some(build) = &self.compiled else {
            return finish(report, control);
        };
        if report.build["status"] == "unavailable" {
            return finish(report, control);
        }
        let identity = build
            .lightmap_identity
            .output
            .as_ref()
            .map_err(|error| baked_build::bounded_message(error))?;
        let decoded = read_tables(identity, build.options.lighting, control);
        let (bsp, tables) = match decoded {
            Ok(decoded) => decoded,
            Err(error) => {
                control.checkpoint()?;
                unavailable(&mut report, &error);
                return finish(report, control);
            }
        };
        if let Some(error) = baked_build::source_issue_with_mode(build, SOURCE_MODE, control) {
            control.checkpoint()?;
            stale(&mut report, &error);
        }
        for (index, (channel, lump)) in [
            (Channel::Ldr, lump::WORLDLIGHTS),
            (Channel::Hdr, lump::WORLDLIGHTS_HDR),
        ]
        .into_iter()
        .enumerate()
        {
            report.channels[index] = ChannelSummary {
                channel: channel.name(),
                lump,
                status: if channel_baked(build.options.lighting, channel) {
                    "available"
                } else {
                    "not_baked"
                },
                bytes: Some(bsp.lumps[lump].data.len()),
                count: Some(tables[index].len()),
            };
        }
        let table = &tables[usize::from(query.channel == Channel::Hdr)];
        control.checkpoint()?;
        let associations = build
            .world_light_origins
            .as_ref()
            .ok_or("Source associations were not retained for this build")
            .and_then(|origins| origins.associations(&bsp, query.channel.kind(), table.len()));
        control.checkpoint()?;
        if query.offset > table.len() {
            return Err("Light page offset exceeds the native table count".into());
        }
        let end = (query.offset + query.limit).min(table.len());
        report
            .page
            .rows
            .try_reserve_exact(end - query.offset)
            .map_err(|_| "Light page cannot be allocated")?;
        let mut source_bytes = 0usize;
        for (ordinal, light) in table.iter().enumerate().take(end).skip(query.offset) {
            control.checkpoint()?;
            let association = source_association(&associations, ordinal, light, &mut source_bytes)?;
            report
                .page
                .rows
                .push(WorldLightRow::from_native(ordinal, light, association));
        }
        report.page.total = Some(table.len());
        report.page.returned = report.page.rows.len();
        report.page.next_offset = (end < table.len()).then_some(end);
        drop(tables);
        drop(bsp);
        if let Some(error) = baked_build::source_issue_with_mode(build, SOURCE_MODE, control) {
            control.checkpoint()?;
            stale(&mut report, &error);
        }
        report = finish(report, control)?;
        if let Err(error) = identity.verify(control) {
            control.checkpoint()?;
            unavailable(
                &mut report,
                &format!("Compiled BSP changed during inspection: {error}"),
            );
        }
        let observed = baked_build::recheck_with_mode(build, SOURCE_MODE, control);
        control.checkpoint()?;
        for error in observed.stale {
            stale(&mut report, &error);
        }
        if let Some(error) = observed.unavailable {
            unavailable(&mut report, &error);
        }
        finish(report, control)
    }
}

fn inspection_build_state(session: &Session, control: &compiler::Control) -> Result<Value, String> {
    let mut state = baked_build::build_state_controlled(session, SOURCE_MODE, control)?;
    if session.compiled.is_none() {
        state["diagnostics"] = json!([
            "Compile the map successfully in this session before inspecting compiled lights"
        ]);
    }
    if state["status"] == "unavailable" {
        state["token"] = Value::Null;
    }
    bound_build_diagnostics(&mut state);
    Ok(state)
}

fn call_bound(decoded: usize) -> Result<usize, String> {
    if decoded > baked_build::MAX_BSP_BYTES as usize {
        return Err("BSP decoded inspection allocation exceeds 128 MiB".into());
    }
    let bytes = decoded
        .checked_add(baked_build::MAX_BSP_BYTES as usize)
        .and_then(|bytes| bytes.checked_add(MAX_METADATA_BYTES))
        .and_then(|bytes| bytes.checked_add(2 * MAX_WORLD_LIGHTS * size_of::<WorldLight>()))
        .and_then(|bytes| bytes.checked_add(MAX_PAGE_LIGHTS * size_of::<WorldLightRow>()))
        .and_then(|bytes| bytes.checked_add(MAX_REPORT_BYTES))
        .and_then(|bytes| bytes.checked_add(4 * MAX_REPORT_BYTES))
        .ok_or("Compiled light inspection allocation bound overflow")?;
    if bytes > MAX_CALL_BYTES {
        return Err("Compiled light inspection exceeds its 384 MiB call storage budget".into());
    }
    Ok(bytes)
}

fn read_tables(
    identity: &compiler::FileIdentity,
    lighting: compiler::LightingMode,
    control: &compiler::Control,
) -> Result<(Bsp, [Vec<WorldLight>; 2]), String> {
    let bytes = baked_build::read_owned_bytes_controlled(identity, control)?;
    call_bound(baked_build::decoded_bsp_bound(&bytes)?)?;
    control.checkpoint()?;
    let bsp = Bsp::parse(&bytes).map_err(|error| error.to_string())?;
    drop(bytes);
    control.checkpoint()?;
    let mut tables = [Vec::new(), Vec::new()];
    for (index, (channel, lump)) in [
        (Channel::Ldr, lump::WORLDLIGHTS),
        (Channel::Hdr, lump::WORLDLIGHTS_HDR),
    ]
    .into_iter()
    .enumerate()
    {
        control.checkpoint()?;
        if bsp.lumps[lump].fourcc != [0; 4] {
            return Err(format!(
                "{} world-light lump has unsupported fourcc",
                channel.name()
            ));
        }
        let lights = bsp
            .world_lights(channel.kind())
            .map_err(|error| format!("{} world-light table: {error}", channel.name()))?;
        if !channel_baked(lighting, channel) && !lights.is_empty() {
            return Err(format!(
                "{} world-light table is nonempty in a channel that was not baked",
                channel.name()
            ));
        }
        for (ordinal, light) in lights.iter().enumerate() {
            control.checkpoint()?;
            if !light
                .origin
                .iter()
                .chain(&light.intensity)
                .chain(&light.normal)
                .copied()
                .chain([
                    light.stop_dot,
                    light.stop_dot2,
                    light.exponent,
                    light.radius,
                    light.constant_attenuation,
                    light.linear_attenuation,
                    light.quadratic_attenuation,
                ])
                .all(f32::is_finite)
            {
                return Err(format!(
                    "{} world-light record {ordinal} contains a nonfinite scalar",
                    channel.name()
                ));
            }
        }
        tables[index] = lights;
    }
    Ok((bsp, tables))
}

fn channel_baked(lighting: compiler::LightingMode, channel: Channel) -> bool {
    match lighting {
        compiler::LightingMode::Fullbright => false,
        compiler::LightingMode::DirectLdr => channel == Channel::Ldr,
        compiler::LightingMode::DirectLdrHdr => true,
    }
}

fn source_association(
    associations: &Result<&[Option<compiler::world_light_origins::Origin>], &'static str>,
    ordinal: usize,
    light: &WorldLight,
    bytes: &mut usize,
) -> Result<SourceAssociation, String> {
    use compiler::world_light_origins::Origin;
    let unavailable = |reason| SourceAssociation::Unavailable { reason };
    if !matches!(light.kind, world_light_kind::POINT | world_light_kind::SPOT) {
        return Ok(unavailable(
            "Source associations are available only for emitted point and spot lights",
        ));
    }
    let entries = match associations {
        Ok(entries) => entries,
        Err(reason) => return Ok(unavailable(reason)),
    };
    let Some(origin) = entries.get(ordinal).and_then(Option::as_ref) else {
        return Ok(unavailable(
            "No source association was captured for this native record",
        ));
    };
    let origin = match origin {
        Origin::Root { entity } => Origin::Root { entity: *entity },
        Origin::Instance {
            instance,
            file,
            entity,
            expanded_entity,
            occurrence,
        } => {
            *bytes = bytes
                .checked_add(file.len())
                .and_then(|bytes| bytes.checked_add(occurrence.response_bytes()))
                .filter(|&bytes| bytes <= MAX_REPORT_BYTES)
                .ok_or("Compiled light source paths exceed the page storage budget")?;
            let mut path = String::new();
            path.try_reserve_exact(file.len())
                .map_err(|_| "Compiled light source path cannot be allocated")?;
            path.push_str(file);
            Origin::Instance {
                instance: *instance,
                file: path,
                entity: *entity,
                expanded_entity: *expanded_entity,
                occurrence: std::sync::Arc::clone(occurrence),
            }
        }
    };
    Ok(SourceAssociation::Available { origin })
}

impl WorldLightRow {
    fn from_native(
        ordinal: usize,
        light: &WorldLight,
        source_association: SourceAssociation,
    ) -> Self {
        Self {
            source_association,
            ordinal,
            kind_name: match light.kind {
                world_light_kind::SURFACE => "surface",
                world_light_kind::POINT => "point",
                world_light_kind::SPOT => "spot",
                world_light_kind::SKY => "sky",
                world_light_kind::QUAKE => "quake",
                world_light_kind::SKY_AMBIENT => "sky_ambient",
                _ => "unknown",
            },
            origin: light.origin,
            intensity: light.intensity,
            normal: light.normal,
            cluster: light.cluster,
            kind: light.kind,
            style: light.style,
            stop_dot: light.stop_dot,
            stop_dot2: light.stop_dot2,
            exponent: light.exponent,
            radius: light.radius,
            constant_attenuation: light.constant_attenuation,
            linear_attenuation: light.linear_attenuation,
            quadratic_attenuation: light.quadratic_attenuation,
            flags: light.flags,
            texinfo: light.texinfo,
            owner: light.owner,
        }
    }
}

fn empty_report(query: &Query, build: Value, live_bytes: usize) -> Inspection {
    Inspection {
        build,
        historical: false,
        channel: query.channel.name(),
        channels: [("ldr", lump::WORLDLIGHTS), ("hdr", lump::WORLDLIGHTS_HDR)].map(
            |(channel, lump)| ChannelSummary {
                channel,
                lump,
                status: "unavailable",
                bytes: None,
                count: None,
            },
        ),
        page: Page {
            offset: query.offset,
            limit: query.limit,
            returned: 0,
            total: None,
            next_offset: None,
            rows: Vec::new(),
        },
        diagnostics: Vec::new(),
        diagnostics_total: 0,
        diagnostics_truncated: false,
        limits: Limits {
            bsp_bytes: baked_build::MAX_BSP_BYTES,
            decoded_bsp_bytes: baked_build::MAX_BSP_BYTES,
            call_bytes: MAX_CALL_BYTES,
            metadata_bytes: MAX_METADATA_BYTES,
            report_bytes: MAX_REPORT_BYTES,
            source_association_bytes: compiler::world_light_origins::MAX_BYTES,
            page_source_bytes: MAX_REPORT_BYTES,
            occurrence_bytes: compiler::instances::MAX_OCCURRENCE_BYTES,
            occurrence_depth: compiler::instances::MAX_EXPANSION_DEPTH,
            page_lights: MAX_PAGE_LIGHTS,
            lights_per_channel: MAX_WORLD_LIGHTS,
            diagnostics: MAX_DIAGNOSTICS,
            diagnostic_bytes: MAX_DIAGNOSTIC_BYTES,
            conservative_live_bytes: live_bytes,
        },
        notes: NOTES,
    }
}

fn diagnostic(report: &mut Inspection, message: &str) {
    report.diagnostics_total += 1;
    let build_diagnostics = report.build["diagnostics"].as_array().map_or(0, Vec::len)
        + report.build["render_context"]["diagnostics"]
            .as_array()
            .map_or(0, Vec::len);
    if report.diagnostics.len() >= MAX_DIAGNOSTICS.saturating_sub(build_diagnostics) {
        report.diagnostics_truncated = true;
        return;
    }
    report.diagnostics_truncated |=
        message.len() > MAX_DIAGNOSTIC_BYTES || message.ends_with(" [truncated]");
    report
        .diagnostics
        .push(baked_build::bounded_message(message));
}

fn stale(report: &mut Inspection, message: &str) {
    if report.build["status"] != "unavailable" {
        report.build["status"] = json!("stale");
    }
    report.build["current"] = json!(false);
    report.build["render_context"]["current_for_display"] = json!(false);
    diagnostic(report, message);
}

fn unavailable(report: &mut Inspection, message: &str) {
    report.build["status"] = json!("unavailable");
    report.build["current"] = json!(false);
    report.build["token"] = Value::Null;
    report.build["render_context"]["current_for_display"] = json!(false);
    report.page.rows.clear();
    report.page.returned = 0;
    report.page.total = None;
    report.page.next_offset = None;
    for channel in &mut report.channels {
        channel.status = "unavailable";
        channel.count = None;
        channel.bytes = None;
    }
    report.historical = false;
    diagnostic(report, message);
}

fn bound_build_diagnostics(state: &mut Value) {
    let maximum = MAX_DIAGNOSTICS.saturating_sub(
        state["render_context"]["diagnostics"]
            .as_array()
            .map_or(0, Vec::len),
    );
    let Some(diagnostics) = state["diagnostics"].as_array_mut() else {
        return;
    };
    let total = diagnostics.len();
    let mut truncated = total > maximum;
    diagnostics.truncate(maximum);
    for diagnostic in diagnostics.iter_mut() {
        if let Some(message) = diagnostic.as_str() {
            truncated |= message.len() > MAX_DIAGNOSTIC_BYTES || message.ends_with(" [truncated]");
            *diagnostic = json!(baked_build::bounded_message(message));
        }
    }
    let returned = diagnostics.len();
    state["diagnostics_total"] = json!(total);
    state["diagnostics_returned"] = json!(returned);
    state["diagnostics_truncated"] = json!(truncated);
}

struct ReportSize<'a> {
    bytes: usize,
    control: &'a compiler::Control,
}

impl Write for ReportSize<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.control.checkpoint().map_err(io::Error::other)?;
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|&bytes| bytes <= MAX_REPORT_BYTES)
            .ok_or_else(|| io::Error::other("Compiled light report exceeds 256 KiB"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn finish(mut report: Inspection, control: &compiler::Control) -> Result<Inspection, String> {
    control.checkpoint()?;
    report.historical = report.build["status"] == "stale";
    serde_json::to_writer(ReportSize { bytes: 0, control }, &report)
        .map_err(|error| error.to_string())?;
    control.checkpoint()?;
    Ok(report)
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn storage_report_and_diagnostics_have_prepaid_exact_boundaries() {
        let admitted = call_bound(baked_build::MAX_BSP_BYTES as usize).unwrap();
        assert!(admitted > 2 * baked_build::MAX_BSP_BYTES as usize + MAX_METADATA_BYTES);
        assert!(admitted < MAX_CALL_BYTES);
        assert!(call_bound(baked_build::MAX_BSP_BYTES as usize + 1).is_err());
        assert!(call_bound(usize::MAX).is_err());
        let control = compiler::Control::default();
        let mut size = ReportSize {
            bytes: MAX_REPORT_BYTES - 3,
            control: &control,
        };
        size.write_all(b"abc").unwrap();
        assert!(size.write_all(b"x").is_err());
        let mut report = empty_report(
            &Query::default(),
            json!({"status":"current","diagnostics":[],"render_context":{"diagnostics":[]}}),
            admitted,
        );
        let message = "é".repeat(MAX_DIAGNOSTIC_BYTES);
        for _ in 0..MAX_DIAGNOSTICS + 1 {
            diagnostic(&mut report, &message);
        }
        assert_eq!(report.diagnostics_total, MAX_DIAGNOSTICS + 1);
        assert_eq!(report.diagnostics.len(), MAX_DIAGNOSTICS);
        assert!(report.diagnostics_truncated);
        assert!(
            report
                .diagnostics
                .iter()
                .all(|message| message.len() <= MAX_DIAGNOSTIC_BYTES
                    && message.ends_with(" [truncated]"))
        );
        finish(report, &control).unwrap();
        control.cancel().unwrap();
        assert!(
            ReportSize {
                bytes: 0,
                control: &control
            }
            .write_all(b"x")
            .is_err()
        );
    }
}
