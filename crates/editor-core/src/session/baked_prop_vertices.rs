use std::io::{self, Write};

use compiler::prop_lighting_assets::{self, ValidatedPropLighting};
use serde_json::{Value, json};
use source_formats::bsp::{Bsp, PakLimits};
use source_formats::vhv;

use super::{Session, baked_build};

pub use super::baked_lightmaps::Channel;

pub const MAX_PAGE_PROPS: usize = 32;
pub const MAX_PAGE_SECTIONS: usize = 32;
pub const MAX_PAGE_VERTICES: usize = 256;
pub const MAX_REPORT_BYTES: usize = 256 * 1024;
pub const MAX_PAGE_SOURCE_BYTES: usize = 128 * 1024;
pub const MAX_CALL_BYTES: usize = 384 * 1024 * 1024;
const SOURCE_BYTES: usize = 64 * 1024 * 1024;
const METADATA_BYTES: usize = 32 * 1024 * 1024;
const MAX_DIAGNOSTICS: usize = 64;
const MAX_DIAGNOSTIC_BYTES: usize = 512;

#[derive(Debug, Clone)]
pub struct Query {
    pub build_token: Option<String>,
    pub channel: Channel,
    pub prop: Option<u32>,
    pub section: Option<usize>,
    pub prop_offset: usize,
    pub prop_limit: usize,
    pub section_offset: usize,
    pub section_limit: usize,
    pub vertex_offset: usize,
    pub vertex_limit: usize,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            build_token: None,
            channel: Channel::Ldr,
            prop: None,
            section: None,
            prop_offset: 0,
            prop_limit: MAX_PAGE_PROPS,
            section_offset: 0,
            section_limit: MAX_PAGE_SECTIONS,
            vertex_offset: 0,
            vertex_limit: MAX_PAGE_VERTICES,
        }
    }
}

pub struct Inspection {
    pub report: Value,
}

impl Query {
    fn validate(&self) -> Result<(), String> {
        for (offset, limit, maximum, label) in [
            (self.prop_offset, self.prop_limit, MAX_PAGE_PROPS, "Prop"),
            (
                self.section_offset,
                self.section_limit,
                MAX_PAGE_SECTIONS,
                "Section",
            ),
            (
                self.vertex_offset,
                self.vertex_limit,
                MAX_PAGE_VERTICES,
                "Vertex",
            ),
        ] {
            if limit == 0 || limit > maximum {
                return Err(format!("{label} page limit must be 1–{maximum}"));
            }
            offset
                .checked_add(limit)
                .ok_or_else(|| format!("{label} page range overflow"))?;
        }
        if self
            .build_token
            .as_ref()
            .is_some_and(|token| token.len() > 20)
        {
            return Err("Build token exceeds its bounded decimal length".into());
        }
        if self.build_token.is_none()
            && (self.prop.is_some()
                || self.section.is_some()
                || self.prop_offset != 0
                || self.section_offset != 0
                || self.vertex_offset != 0)
        {
            return Err(
                "A retained prop, section or page requires the returned build token".into(),
            );
        }
        Ok(())
    }
}

impl Session {
    pub fn baked_prop_vertex_build_state(&self) -> Value {
        let mut state = baked_build::build_state(self);
        if self.compiled.is_none() {
            state["diagnostics"] = json!([
                "Compile the map successfully in this session before inspecting baked prop vertices"
            ]);
        }
        if let Some(build) = &self.compiled {
            state["bounces"] = json!(build.options.bounces);
            state["bake_static_prop_vertices"] = json!(build.options.bake_static_prop_vertices);
            if let Some(guard) = &build.prop_lighting_guard
                && let Err(error) = guard.verify_metadata()
            {
                if state["status"] != "unavailable" {
                    state["status"] = json!("stale");
                }
                state["current"] = json!(false);
                state["render_context"]["current_for_display"] = json!(false);
                state["diagnostics"]
                    .as_array_mut()
                    .expect("build diagnostics")
                    .push(json!(format!(
                        "Published prop lighting sources changed: {error}"
                    )));
            }
        }
        bound_diagnostics(&mut state);
        state
    }

    pub fn inspect_baked_prop_vertices(&self, query: &Query) -> Result<Inspection, String> {
        self.inspect_baked_prop_vertices_controlled(query, &compiler::Control::default())
    }

    pub fn inspect_baked_prop_vertices_controlled(
        &self,
        query: &Query,
        control: &compiler::Control,
    ) -> Result<Inspection, String> {
        control.checkpoint()?;
        query.validate()?;
        let mut report = empty_report(query, self.baked_prop_vertex_build_state());
        let Some(build) = &self.compiled else {
            if query.build_token.is_some() {
                return Err("The requested successful build is no longer retained".into());
            }
            return finish_controlled(report, control);
        };
        let token = build.lightmap_identity.token.to_string();
        if query
            .build_token
            .as_ref()
            .is_some_and(|requested| requested != &token)
        {
            return Err(
                "Build token differs from the retained successful artifact; refresh inspection"
                    .into(),
            );
        }
        report["build"]["bounces"] = json!(build.options.bounces);
        report["build"]["bake_static_prop_vertices"] =
            json!(build.options.bake_static_prop_vertices);
        if report["build"]["status"] == "unavailable" {
            return finish_controlled(report, control);
        }
        let identity = build
            .lightmap_identity
            .output
            .as_ref()
            .map_err(Clone::clone)?;
        let bytes = match baked_build::read_owned_bytes_controlled(identity, control) {
            Ok(bytes) => bytes,
            Err(error) => {
                control.checkpoint()?;
                unavailable(&mut report, error);
                return finish_controlled(report, control);
            }
        };
        let decoded = match baked_build::decoded_bsp_bound(&bytes) {
            Ok(bound) => bound,
            Err(error) => {
                unavailable(&mut report, error);
                return finish_controlled(report, control);
            }
        };
        let peak = decoded
            .checked_add(PakLimits::default().max_total_bytes)
            .and_then(|bytes| bytes.checked_add(baked_build::MAX_BSP_BYTES as usize))
            .and_then(|bytes| bytes.checked_add(METADATA_BYTES))
            .ok_or("Baked prop inspection allocation bound overflow")?;
        if peak > MAX_CALL_BYTES {
            return Err("Baked prop inspection exceeds its 384 MiB live allocation bound".into());
        }
        report["limits"]["conservative_live_bytes"] = json!(peak);
        let bsp = match Bsp::parse(&bytes) {
            Ok(bsp) => bsp,
            Err(error) => {
                unavailable(&mut report, error.to_string());
                return finish_controlled(report, control);
            }
        };
        drop(bytes);
        if let Some(error) = baked_build::source_issue_controlled(build, control) {
            control.checkpoint()?;
            stale(&mut report, error);
        }
        let bundle = match prop_lighting_assets::read_validated_prop_lighting(&bsp, control) {
            Ok(bundle) => bundle,
            Err(error) => {
                control.checkpoint()?;
                unavailable(
                    &mut report,
                    format!("Generated prop lighting cannot be validated: {error}"),
                );
                recheck_output(build, identity, control, &mut report);
                control.checkpoint()?;
                return finish_controlled(report, control);
            }
        };
        let mut closure = if bundle.manifest().is_some() {
            "unverifiable"
        } else {
            "not_applicable"
        };
        if bundle.manifest().is_some() {
            match &build.prop_lighting_guard {
                Some(guard) => match bundle.verify_published_sources(guard, self.game(), control) {
                    Ok(()) => closure = "verified",
                    Err(error) => {
                        control.checkpoint()?;
                        closure = "changed";
                        stale(
                            &mut report,
                            format!("Published prop lighting sources changed: {error}"),
                        );
                    }
                },
                None => stale(
                    &mut report,
                    "The original published prop source guard is unavailable".into(),
                ),
            }
            if build.content_epoch != self.content_epoch {
                closure = "changed";
            }
        }
        let source_associations = build
            .static_prop_origins
            .as_ref()
            .map_err(String::as_str)
            .and_then(|origins| {
                origins.associations(
                    &bsp,
                    bundle.static_props().map_or(0, |props| props.props.len()),
                )
            });
        inspect_bundle(&bundle, query, closure, &source_associations, &mut report)?;
        if let Some(error) = baked_build::source_issue_controlled(build, control) {
            control.checkpoint()?;
            stale(&mut report, error);
        }
        if bundle.manifest().is_some()
            && let Some(guard) = &build.prop_lighting_guard
            && let Err(error) = bundle.verify_published_sources(guard, self.game(), control)
        {
            control.checkpoint()?;
            report["bake"]["source_closure"] = json!("changed");
            stale(
                &mut report,
                format!("Published prop lighting sources changed during inspection: {error}"),
            );
        }
        recheck_output(build, identity, control, &mut report);
        control.checkpoint()?;
        report["historical"] = json!(report["build"]["status"] == "stale");
        finish_controlled(report, control)
    }
}

fn empty_report(query: &Query, build: Value) -> Value {
    json!({
        "build":build,"historical":false,"channel":query.channel.name(),
        "bake":{"status":"unavailable","manifest_version":null,"static_props_version":null,"static_props_sha256":null,"map_flags":null,"emitted_props":0,"baked_receivers":0,"source_closure":"unverifiable","diagnostics":[]},
        "source_associations":{"status":"unavailable","diagnostics":["No successful native prop source mapping was retained"]},
        "selected_prop":null,"channels":[],"props":page_value(query.prop_offset,query.prop_limit,0,Vec::new()),
        "selected":null,"sections":page_value(query.section_offset,query.section_limit,0,Vec::new()),
        "vertices":page_value(query.vertex_offset,query.vertex_limit,0,Vec::new()),"statistics":null,
        "color_encoding":{"storage":"bgra8","range":[0,255],"alpha":"stored component; current writer emits 255; not an authored opacity measurement","units":"encoded_vertex_color_bytes","explanation":"Raw lossy gamma/overbright encoded vertex-color bytes, not linear RGBExp32 intensity, irradiance, material-modulated screen pixels or exposure-adapted brightness. HDR uses the same byte encoding independently. RGBA is only byte reordering."},
        "limits":{"bsp_bytes":baked_build::MAX_BSP_BYTES,"pak_bytes":PakLimits::default().max_total_bytes,"source_bytes":SOURCE_BYTES,"live_bytes":MAX_CALL_BYTES,"report_bytes":MAX_REPORT_BYTES,"page_source_bytes":MAX_PAGE_SOURCE_BYTES,"page_props":MAX_PAGE_PROPS,"page_sections":MAX_PAGE_SECTIONS,"page_vertices":MAX_PAGE_VERTICES,"diagnostics":MAX_DIAGNOSTICS,"diagnostic_bytes":MAX_DIAGNOSTIC_BYTES,"receiver_sections":prop_lighting_assets::MAX_RECEIVER_MESHES},
        "diagnostics":[]
    })
}

fn source_association(
    associations: &Result<&[compiler::entity_origins::Origin], &str>,
    ordinal: usize,
    bytes: &mut usize,
) -> Result<Value, String> {
    let entries = match associations {
        Ok(entries) => entries,
        Err(reason) => return Ok(json!({"status":"unavailable","reason":reason})),
    };
    let Some(origin) = entries.get(ordinal) else {
        return Ok(
            json!({"status":"unavailable","reason":"No source association was captured for this native prop"}),
        );
    };
    *bytes = bytes
        .checked_add(origin.response_copy_bytes())
        .filter(|bytes| *bytes <= MAX_PAGE_SOURCE_BYTES)
        .ok_or("Prop source paths exceed 128 KiB; request fewer props per page")?;
    let origin = origin.try_clone()?;
    let label = origin.route_label();
    let description = origin.label(false);
    Ok(json!({"status":"available","origin":origin,"label":label,"description":description}))
}

fn inspect_bundle(
    bundle: &ValidatedPropLighting,
    query: &Query,
    closure: &str,
    source_associations: &Result<&[compiler::entity_origins::Origin], &str>,
    report: &mut Value,
) -> Result<(), String> {
    let manifest = bundle.manifest();
    let props = bundle.static_props();
    let total = props.map_or(0, |props| props.props.len());
    page_range(query.prop_offset, total, "Prop")?;
    if let Some(ordinal) = query.prop
        && ordinal as usize >= total
    {
        return Err("Compiled prop ordinal exceeds the final native table".into());
    }
    report["bake"] = json!({"status":if manifest.is_some(){"available"}else{"not_baked"},"manifest_version":manifest.map(|manifest|manifest.version),"static_props_version":props.map(|props|props.version),"static_props_sha256":manifest.map(|manifest|&manifest.static_props_sha256),"map_flags":bundle.map_flags(),"emitted_props":total,"baked_receivers":manifest.map_or(0,|manifest|manifest.receivers.len()),"source_closure":closure,"diagnostics":[]});
    report["channels"] = json!([prop_lighting_assets::Channel::Ldr,prop_lighting_assets::Channel::Hdr].map(|channel|{
        let bytes = manifest.map_or(0, |manifest| manifest.assets.iter().filter(|asset|asset.channel == channel).map(|asset|asset.bytes).sum::<usize>());
        json!({"channel":channel,"available":manifest.is_some_and(|manifest|manifest.channels.contains(&channel)),"bytes":bytes})
    }));
    report["source_associations"] = match source_associations {
        Ok(_) => json!({"status":"available","diagnostics":[]}),
        Err(reason) => json!({"status":"unavailable","diagnostics":[reason]}),
    };
    let mut source_bytes = 0usize;
    let mut rows = Vec::new();
    if let Some(props) = props {
        for (ordinal, prop) in props
            .props
            .iter()
            .enumerate()
            .skip(query.prop_offset)
            .take(query.prop_limit)
        {
            let receiver = manifest.and_then(|manifest| {
                manifest
                    .receivers
                    .binary_search_by_key(&(ordinal as u32), |receiver| receiver.ordinal)
                    .ok()
                    .map(|index| &manifest.receivers[index])
            });
            let source_association =
                source_association(source_associations, ordinal, &mut source_bytes)?;
            rows.push(json!({"source_association":source_association,"ordinal":ordinal,"model":props.model_name(prop.model as usize).map_err(|error|error.to_string())?,"origin":prop.origin,"skin":prop.skin,"flags":prop.flags,"status":if prop.flags & 0x40 != 0{"disabled_by_prop"}else if receiver.is_some(){"baked"}else{"not_baked"},"checksum":receiver.map(|receiver|receiver.checksum),"section_count":receiver.map_or(0,|receiver|receiver.meshes.len()),"vertex_count":receiver.map_or(0,|receiver|receiver.meshes.iter().map(|mesh|mesh.vertex_count as usize).sum::<usize>()),"channels":if receiver.is_some(){manifest.map(|manifest|manifest.channels.as_slice()).unwrap_or_default()}else{&[]}}));
        }
    }
    report["props"] = page_value(query.prop_offset, query.prop_limit, total, rows);
    let channel = match query.channel {
        Channel::Ldr => prop_lighting_assets::Channel::Ldr,
        Channel::Hdr => prop_lighting_assets::Channel::Hdr,
    };
    let ordinal = query.prop.or_else(|| {
        manifest.and_then(|manifest| {
            manifest
                .assets
                .iter()
                .find(|asset| asset.channel == channel)
                .map(|asset| asset.ordinal)
        })
    });
    let native_ordinal = ordinal.or_else(|| (total > 0).then_some(0));
    if let Some(ordinal) = native_ordinal
        && let Some(props) = props
    {
        let association =
            source_association(source_associations, ordinal as usize, &mut source_bytes)?;
        report["selected_prop"] = json!({"ordinal":ordinal,"model":props.model_name(props.props[ordinal as usize].model as usize).map_err(|error|error.to_string())?,"source_association":association});
    }
    let selected = ordinal.and_then(|ordinal| bundle.asset(ordinal, channel));
    let Some((asset, data)) = selected else {
        page_range(query.section_offset, 0, "Section")?;
        page_range(query.vertex_offset, 0, "Vertex")?;
        report["diagnostics"] = json!([if manifest.is_none() {
            "This successful build did not bake static-prop vertex files"
        } else {
            "The selected native prop or requested channel has no baked vertex file"
        }]);
        return Ok(());
    };
    let view = vhv::View::parse(data, vhv::Limits::default()).map_err(|error| error.to_string())?;
    let metadata = view.metadata();
    page_range(query.section_offset, metadata.mesh_count, "Section")?;
    let section_index = query.section.unwrap_or(0);
    let section = view
        .section(section_index)
        .map_err(|error| error.to_string())?;
    let count = section.layout.vertex_count as usize;
    page_range(query.vertex_offset, count, "Vertex")?;
    let colors = section
        .page(
            query.vertex_offset,
            query.vertex_limit.min(count - query.vertex_offset),
        )
        .map_err(|error| error.to_string())?;
    let props = props.ok_or("Validated receiver has no final native table")?;
    let prop = &props.props[asset.ordinal as usize];
    report["selected"] = json!({"ordinal":asset.ordinal,"model":props.model_name(prop.model as usize).map_err(|error|error.to_string())?,"checksum":metadata.checksum,"channel":channel,"path":asset.path,"sha256":asset.sha256,"bytes":asset.bytes,"version":vhv::VERSION,"flags":vhv::VERTEX_COLOR,"stride":vhv::VERTEX_BYTES,"section_count":metadata.mesh_count,"vertex_count":metadata.vertex_count});
    let mut sections = Vec::new();
    for index in query.section_offset
        ..metadata
            .mesh_count
            .min(query.section_offset + query.section_limit)
    {
        let section = view.section(index).map_err(|error| error.to_string())?;
        sections.push(json!({"index":index,"lod":section.layout.lod,"vertex_count":section.layout.vertex_count,"hardware_start":section.hardware_start,"payload_offset":section.payload_offset}));
    }
    report["sections"] = page_value(
        query.section_offset,
        query.section_limit,
        metadata.mesh_count,
        sections,
    );
    let rows = colors.iter().enumerate().map(|(offset,color)|{
        let index = query.vertex_offset+offset;
        json!({"index":index,"hardware_index":section.hardware_start+index,"bgra8":[color.blue,color.green,color.red,color.alpha],"rgba8":[color.red,color.green,color.blue,color.alpha]})
    }).collect();
    report["vertices"] = page_value(query.vertex_offset, query.vertex_limit, count, rows);
    report["vertices"]["section"] = json!(section_index);
    report["vertices"]["lod"] = json!(section.layout.lod);
    if !colors.is_empty() {
        let mut minimum = [255; 3];
        let mut maximum = [0; 3];
        let mut sum = [0u64; 3];
        for color in colors {
            for (axis, value) in [color.red, color.green, color.blue].into_iter().enumerate() {
                minimum[axis] = minimum[axis].min(value);
                maximum[axis] = maximum[axis].max(value);
                sum[axis] += u64::from(value);
            }
        }
        report["statistics"] = json!({"scope":"returned_vertex_page","count":colors.len(),"encoded_rgb8":{"minimum":minimum,"maximum":maximum,"mean":sum.map(|value|value as f64/colors.len() as f64)}});
    }
    Ok(())
}

fn page_range(offset: usize, total: usize, label: &str) -> Result<(), String> {
    if offset > total {
        return Err(format!("{label} offset exceeds its total"));
    }
    Ok(())
}

fn page_value(offset: usize, limit: usize, total: usize, rows: Vec<Value>) -> Value {
    let end = offset + rows.len();
    json!({"offset":offset,"limit":limit,"returned":rows.len(),"total":total,"next_offset":if end<total{Some(end)}else{None},"rows":rows})
}

fn stale(report: &mut Value, error: String) {
    if report["build"]["status"] != "unavailable" {
        report["build"]["status"] = json!("stale");
    }
    report["build"]["current"] = json!(false);
    report["build"]["render_context"]["current_for_display"] = json!(false);
    report["build"]["diagnostics"]
        .as_array_mut()
        .expect("build diagnostics")
        .push(json!(error));
}

fn unavailable(report: &mut Value, error: String) {
    report["build"]["status"] = json!("unavailable");
    report["build"]["current"] = json!(false);
    report["build"]["render_context"]["current_for_display"] = json!(false);
    report["bake"]["status"] = json!("unavailable");
    report["source_associations"] = json!({"status":"unavailable","diagnostics":["The owned native prop table is unavailable"]});
    report["selected_prop"] = Value::Null;
    report["selected"] = Value::Null;
    report["statistics"] = Value::Null;
    for key in ["props", "sections", "vertices"] {
        report[key]["rows"] = json!([]);
        report[key]["returned"] = json!(0);
        report[key]["next_offset"] = Value::Null;
    }
    report["channels"] = json!([]);
    report["historical"] = json!(false);
    report["diagnostics"] = json!([error]);
}

fn recheck_output(
    build: &super::CompiledMap,
    identity: &compiler::FileIdentity,
    control: &compiler::Control,
    report: &mut Value,
) {
    let observed = baked_build::recheck(build);
    for error in observed.stale {
        stale(report, error);
    }
    if let Some(error) = observed.unavailable {
        unavailable(report, error);
    }
    if let Err(error) = identity.verify(control) {
        unavailable(
            report,
            format!("Compiled BSP changed during inspection: {error}"),
        );
    }
}

fn bound_diagnostics(value: &mut Value) {
    let mut remaining = MAX_DIAGNOSTICS;
    bound_diagnostics_remaining(value, &mut remaining);
}

fn bound_diagnostics_remaining(value: &mut Value, remaining: &mut usize) {
    match value {
        Value::Object(object) => {
            let previous_total = object
                .get("diagnostics_total")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let previous_returned = object
                .get("diagnostics_returned")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let previous_truncation = object
                .get("diagnostics_truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if let Some(Value::Array(diagnostics)) = object.get_mut("diagnostics") {
                let total = previous_total + diagnostics.len().saturating_sub(previous_returned);
                let returned = diagnostics.len().min(*remaining);
                let mut truncated = previous_truncation || total > returned;
                diagnostics.truncate(returned);
                *remaining -= returned;
                for value in diagnostics {
                    if let Some(message) = value.as_str()
                        && message.len() > MAX_DIAGNOSTIC_BYTES
                    {
                        let mut end = MAX_DIAGNOSTIC_BYTES;
                        while !message.is_char_boundary(end) {
                            end -= 1;
                        }
                        *value = json!(&message[..end]);
                        truncated = true;
                    }
                }
                object.insert("diagnostics_total".into(), json!(total));
                object.insert("diagnostics_returned".into(), json!(returned));
                object.insert("diagnostics_truncated".into(), json!(truncated));
            }
            for value in object.values_mut() {
                bound_diagnostics_remaining(value, remaining);
            }
        }
        Value::Array(values) => {
            for value in values {
                bound_diagnostics_remaining(value, remaining);
            }
        }
        _ => {}
    }
}

struct ReportSize(usize);
impl Write for ReportSize {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .filter(|&size| size <= MAX_REPORT_BYTES)
            .ok_or_else(|| io::Error::other("Baked prop report exceeds 256 KiB"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn finish(mut report: Value) -> Result<Inspection, String> {
    bound_diagnostics(&mut report);
    serde_json::to_writer(ReportSize(0), &report).map_err(|error| error.to_string())?;
    Ok(Inspection { report })
}

fn finish_controlled(report: Value, control: &compiler::Control) -> Result<Inspection, String> {
    control.checkpoint()?;
    let inspection = finish(report)?;
    control.checkpoint()?;
    Ok(inspection)
}

#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
