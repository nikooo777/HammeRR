#[cfg(test)]
use std::path::Path;

use serde_json::{Value, json};
use source_formats::bsp::{Bsp, Face, Lighting, LightingKind, RgbExp32, TexInfo};
use source_formats::vtf::RgbaImage;

use super::Session;

mod origins;
pub use origins::SourceReference;

pub const MAX_PAGE_FACES: usize = 32;
pub const MAX_PREVIEW_SIDE: usize = 128;
const MAX_SOURCE_FILE_BYTES: usize = 4096;
const RGBEXP_BYTES: usize = size_of::<RgbExp32>();
const DISPLAY_CONVERSION: &str = "Each linear RGB channel uses (v / (1 + v))^(1 / 2.2), v = intensity / 255. Nearest sample reduction, no exposure adaptation or native game tone mapping.";

pub(super) use super::baked_build::BuildIdentity;
use super::baked_build::{MAX_BSP_BYTES, read_owned_bytes};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    #[default]
    Ldr,
    Hdr,
}

impl Channel {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ldr" => Ok(Self::Ldr),
            "hdr" => Ok(Self::Hdr),
            _ => Err("Channel must be ldr or hdr".into()),
        }
    }

    pub(super) fn kind(self) -> LightingKind {
        match self {
            Self::Ldr => LightingKind::Ldr,
            Self::Hdr => LightingKind::Hdr,
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Ldr => "ldr",
            Self::Hdr => "hdr",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Query {
    pub channel: Channel,
    pub offset: usize,
    pub limit: usize,
    pub face: Option<usize>,
    pub source: Option<SourceReference>,
    pub selected_authored_face: bool,
    pub contributor_offset: usize,
    pub style_index: usize,
    pub map_index: usize,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            channel: Channel::Ldr,
            offset: 0,
            limit: MAX_PAGE_FACES,
            face: None,
            source: None,
            selected_authored_face: false,
            contributor_offset: 0,
            style_index: 0,
            map_index: 0,
        }
    }
}

pub struct Inspection {
    pub report: Value,
    pub image: Option<RgbaImage>,
}

impl Session {
    pub fn baked_lightmap_build_state(&self) -> Value {
        super::baked_build::build_state(self)
    }

    pub fn inspect_baked_lightmaps(&self, query: &Query) -> Result<Inspection, String> {
        if usize::from(query.face.is_some())
            + usize::from(query.source.is_some())
            + usize::from(query.selected_authored_face)
            > 1
        {
            return Err(
                "Choose only one compiled face, source reference or selected authored face".into(),
            );
        }
        if let Some(SourceReference::Instance { file, .. }) = &query.source
            && file.len() > MAX_SOURCE_FILE_BYTES
        {
            return Err("Instance source reference exceeds the bounded filename length".into());
        }
        let source = if query.selected_authored_face {
            Some(self.selected_baked_lightmap_source()?)
        } else {
            query.source.clone()
        };
        if query.limit == 0 || query.limit > MAX_PAGE_FACES {
            return Err(format!("Face page limit must be 1–{MAX_PAGE_FACES}"));
        }
        if query.style_index >= 4 || query.map_index >= 4 {
            return Err("Style and map indices must be 0–3".into());
        }
        let mut report = json!({
            "build":self.baked_lightmap_build_state(), "channel":query.channel.name(),
            "face_identity":"Zero-based compiled BSP face index in the selected channel; authored contributors come only from the exact retained compiler emission mapping.",
            "mapping":origins::unavailable("No successful owned emission mapping is available"),
            "intensity_units":"Decoded linear RGBExp32 intensities; independent of the display image and not native screen brightness.",
            "channels":[], "faces":[], "selected":null, "diagnostics":[],
            "limits":{"bsp_bytes":MAX_BSP_BYTES,"page_faces":MAX_PAGE_FACES,"preview_side":MAX_PREVIEW_SIDE,"mapping_faces":compiler::face_origins::MAX_FACES,"mapping_references":compiler::face_origins::MAX_REFERENCES,"mapping_bytes":compiler::face_origins::MAX_BYTES},
        });
        let Some(build) = &self.compiled else {
            return Ok(Inspection {
                report,
                image: None,
            });
        };
        if report["build"]["status"] == "unavailable" {
            return Ok(Inspection {
                report,
                image: None,
            });
        }
        let identity = build
            .lightmap_identity
            .output
            .as_ref()
            .map_err(Clone::clone)?;
        let result = read_owned_bytes(identity)
            .and_then(|bytes| Bsp::parse(&bytes).map_err(|error| error.to_string()));
        let bsp = match result {
            Ok(bsp) => bsp,
            Err(error) => {
                mark_unavailable(
                    &mut report,
                    format!("Compiled BSP cannot be inspected: {error}"),
                );
                return Ok(Inspection {
                    report,
                    image: None,
                });
            }
        };
        if let Some(error) = super::baked_build::source_issue(build) {
            mark_stale(&mut report, error);
        }
        let mut effective = query.clone();
        let mapping = build
            .face_origins
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|origins| {
                origins.validate_channel(&bsp, query.channel.kind())?;
                Ok(origins)
            });
        match &mapping {
            Ok(origins) => {
                report["mapping"] = json!({"status":"available","retained_faces":origins.len(),"metadata_bytes":origins.bytes(),"contributors":[],"matches":[],"diagnostics":[]});
                if let Some(source) = &source {
                    let (matches, face) =
                        origins::matches(origins, source, query.offset, query.limit);
                    merge(&mut report["mapping"], matches);
                    effective.face = face;
                    effective.offset = 0;
                    if face.is_none() {
                        report["channels"] = json!([
                            channel_summary(&bsp, Channel::Ldr),
                            channel_summary(&bsp, Channel::Hdr)
                        ]);
                        recheck(build, &mut report);
                        return Ok(Inspection {
                            report,
                            image: None,
                        });
                    }
                }
            }
            Err(error) => {
                report["mapping"] = origins::unavailable(error);
                if source.is_some() {
                    report["channels"] = json!([
                        channel_summary(&bsp, Channel::Ldr),
                        channel_summary(&bsp, Channel::Hdr)
                    ]);
                    recheck(build, &mut report);
                    return Ok(Inspection {
                        report,
                        image: None,
                    });
                }
            }
        }
        let inspected = inspect_bsp(&bsp, &effective);
        match inspected {
            Ok((details, image)) => {
                for (key, value) in details.as_object().expect("inspection object") {
                    report[key] = value.clone();
                }
                if let Ok(origins) = mapping
                    && let Some(face) = report["selected"]["face"].as_u64()
                {
                    merge(
                        &mut report["mapping"],
                        origins::selected(
                            origins,
                            &bsp,
                            query.channel,
                            face as usize,
                            query.contributor_offset,
                            query.limit,
                        ),
                    );
                }
                recheck(build, &mut report);
                let image = if report["build"]["status"] == "unavailable" {
                    None
                } else {
                    image
                };
                Ok(Inspection { report, image })
            }
            Err(error) => {
                report["channels"] = json!([
                    channel_summary(&bsp, Channel::Ldr),
                    channel_summary(&bsp, Channel::Hdr)
                ]);
                report["diagnostics"] = json!([error]);
                recheck(build, &mut report);
                Ok(Inspection {
                    report,
                    image: None,
                })
            }
        }
    }
}

fn merge(target: &mut Value, value: Value) {
    for (key, value) in value.as_object().expect("report object") {
        target[key] = value.clone();
    }
}

fn mark_stale(report: &mut Value, error: String) {
    report["build"]["current"] = json!(false);
    report["build"]["status"] = json!("stale");
    report["build"]["render_context"]["current_for_display"] = json!(false);
    report["build"]["diagnostics"]
        .as_array_mut()
        .expect("build diagnostics")
        .push(json!(error));
}

fn mark_unavailable(report: &mut Value, error: String) {
    report["build"]["status"] = json!("unavailable");
    report["build"]["current"] = json!(false);
    report["build"]["render_context"]["current_for_display"] = json!(false);
    report["selected"] = Value::Null;
    report["faces"] = json!([]);
    report["mapping"] = origins::unavailable(&error);
    report["diagnostics"] = json!([error]);
}

fn recheck(build: &super::CompiledMap, report: &mut Value) {
    let observed = super::baked_build::recheck(build);
    for error in observed.stale {
        mark_stale(report, error);
    }
    if let Some(error) = observed.unavailable {
        mark_unavailable(report, error);
    }
}

fn channel_summary(bsp: &Bsp, channel: Channel) -> Value {
    let kind = channel.kind();
    let mut diagnostics = Vec::new();
    let faces = match bsp.lighting_faces(kind) {
        Ok(faces) => Some(faces),
        Err(error) => {
            diagnostics.push(error.to_string());
            None
        }
    };
    let lighting = match bsp.lighting(kind) {
        Ok(lighting) => lighting,
        Err(error) => {
            diagnostics.push(error.to_string());
            None
        }
    };
    let present = lighting.is_some() && faces.as_ref().is_some_and(|faces| !faces.is_empty());
    if !present && diagnostics.is_empty() {
        diagnostics.push("No baked face lightmaps in this channel; Fullbright builds and absent HDR channels have no samples".into());
    }
    json!({"channel":channel.name(), "available":present,
        "face_count":faces.as_ref().map_or(0, Vec::len),
        "lit_face_count":faces.as_ref().map_or(0, |faces| faces.iter().filter(|face| face.light_offset >= 0).count()),
        "lighting_bytes":lighting.map_or(0, |lighting| lighting.samples().len() * RGBEXP_BYTES),
        "diagnostics":diagnostics,
    })
}

fn face_view<'a>(
    lighting: Lighting<'a>,
    face: &Face,
    infos: &[TexInfo],
) -> Result<Option<source_formats::bsp::FaceLighting<'a>>, String> {
    if face.light_offset == -1 {
        return Ok(None);
    }
    let info = usize::try_from(face.texinfo)
        .ok()
        .and_then(|index| infos.get(index))
        .ok_or("Compiled face references invalid texture information")?;
    lighting.face(face, info).map_err(|error| error.to_string())
}

fn face_row(lighting: Lighting<'_>, index: usize, face: &Face, infos: &[TexInfo]) -> Value {
    match face_view(lighting, face, infos) {
        Ok(Some(view)) => {
            json!({"face":index,"status":"available","dimensions":view.dimensions(),"styles":view.styles(),"maps":view.maps(),
            "sample_bytes":view.dimensions().iter().product::<usize>() * view.styles().len() * view.maps() * RGBEXP_BYTES,
            "average_bytes":view.styles().len() * RGBEXP_BYTES,"light_offset":face.light_offset})
        }
        Ok(None) => {
            json!({"face":index,"status":"unlit","diagnostics":["Face has light_offset -1; no baked samples"]})
        }
        Err(error) => json!({"face":index,"status":"invalid","diagnostics":[error]}),
    }
}

fn inspect_bsp(bsp: &Bsp, query: &Query) -> Result<(Value, Option<RgbaImage>), String> {
    let channels = [Channel::Ldr, Channel::Hdr].map(|channel| channel_summary(bsp, channel));
    let mut report = json!({"channels":channels,"faces":[],"selected":null});
    let kind = query.channel.kind();
    let faces = bsp
        .lighting_faces(kind)
        .map_err(|error| error.to_string())?;
    report["page"] = json!({"offset":query.offset,"limit":query.limit,"total":faces.len(),"next_offset":query.offset.checked_add(query.limit).filter(|&offset| offset < faces.len())});
    if query.offset > faces.len() {
        return Err("Face page offset exceeds the compiled face count".into());
    }
    if query.face.is_some_and(|face| face >= faces.len()) {
        return Err("Compiled face index exceeds this channel's face count".into());
    }
    let Some(lighting) = bsp.lighting(kind).map_err(|error| error.to_string())? else {
        if let Some(index) = query.face.or((!faces.is_empty()).then_some(0))
            && index < faces.len()
        {
            report["selected"] = json!({"face":index,"status":"unlit","diagnostics":["No baked samples in this channel"]});
        }
        report["diagnostics"] = json!([
            "No baked samples in the requested channel. Compile with Direct LDR or Direct LDR + HDR; Fullbright has no baked lightmaps."
        ]);
        return Ok((report, None));
    };
    let infos = bsp
        .records::<TexInfo>()
        .map_err(|error| error.to_string())?;
    report["faces"] = json!(
        faces
            .iter()
            .enumerate()
            .skip(query.offset)
            .take(query.limit)
            .map(|(index, face)| face_row(lighting, index, face, &infos))
            .collect::<Vec<_>>()
    );
    let selected = query.face.or_else(|| {
        faces
            .iter()
            .position(|face| face_view(lighting, face, &infos).is_ok_and(|view| view.is_some()))
    });
    let Some(index) = selected else {
        report["diagnostics"] = json!(["No valid lit face is available in this channel"]);
        return Ok((report, None));
    };
    let face = faces
        .get(index)
        .ok_or("Compiled face index exceeds this channel's face count")?;
    report["selected"] = face_row(lighting, index, face, &infos);
    let view = match face_view(lighting, face, &infos) {
        Ok(Some(view)) => view,
        _ => return Ok((report, None)),
    };
    let samples = view
        .samples(query.style_index, query.map_index)
        .ok_or("Selected style or bump-map index is absent from this compiled face")?;
    let stored_average = view
        .average(query.style_index)
        .expect("checked style index");
    let mut minimum = [f64::MAX; 3];
    let mut maximum = [0.0_f64; 3];
    let mut mean = [0.0; 3];
    for sample in samples {
        for (axis, value) in sample.linear().into_iter().enumerate() {
            minimum[axis] = minimum[axis].min(value);
            maximum[axis] = maximum[axis].max(value);
            mean[axis] += value / samples.len() as f64;
        }
    }
    let dimensions = view.dimensions();
    let image = preview(samples, dimensions);
    let selected = &mut report["selected"];
    selected["style_index"] = json!(query.style_index);
    selected["style"] = json!(view.styles()[query.style_index]);
    selected["map_index"] = json!(query.map_index);
    selected["map_kind"] = json!(match query.map_index {
        0 => "base",
        1 => "bump_1",
        2 => "bump_2",
        _ => "bump_3",
    });
    selected["sample_count"] = json!(samples.len());
    selected["selected_sample_bytes"] = json!(samples.len() * RGBEXP_BYTES);
    selected["selected_sample_offset"] = json!(
        face.light_offset as usize
            + (query.style_index * view.maps() + query.map_index) * samples.len() * RGBEXP_BYTES
    );
    selected["linear_rgb"] = json!({"minimum":minimum,"mean":mean,"maximum":maximum,
        "stored_base_average":stored_average.linear(),
        "stored_base_average_rgbexp":{"rgb":stored_average.rgb,"exponent":stored_average.exponent}});
    selected["preview"] = json!({"width":image.width,"height":image.height,"conversion":DISPLAY_CONVERSION,"source_dimensions":dimensions});
    Ok((report, Some(image)))
}

fn preview(samples: &[RgbExp32], dimensions: [usize; 2]) -> RgbaImage {
    let largest = dimensions[0].max(dimensions[1]);
    let width =
        (dimensions[0] * MAX_PREVIEW_SIDE / largest).clamp(1, dimensions[0].min(MAX_PREVIEW_SIDE));
    let height =
        (dimensions[1] * MAX_PREVIEW_SIDE / largest).clamp(1, dimensions[1].min(MAX_PREVIEW_SIDE));
    let mut pixels = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let sample =
                samples[(y * dimensions[1] / height) * dimensions[0] + x * dimensions[0] / width];
            pixels.extend(sample.linear().map(|intensity| {
                let value = intensity / 255.0;
                ((value / (1.0 + value)).powf(1.0 / 2.2) * 255.0).round() as u8
            }));
            pixels.push(255);
        }
    }
    RgbaImage {
        width: width as u32,
        height: height as u32,
        pixels,
    }
}

#[cfg(test)]
mod tests;
