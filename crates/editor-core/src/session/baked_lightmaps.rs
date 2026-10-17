use std::io::Read;
use std::path::Path;

use serde_json::{Value, json};
use source_formats::bsp::{Bsp, Face, LUMP_COUNT, Lighting, LightingKind, RgbExp32, TexInfo};
use source_formats::vtf::RgbaImage;

use super::Session;

pub const MAX_PAGE_FACES: usize = 32;
pub const MAX_PREVIEW_SIDE: usize = 128;
const MAX_BSP_BYTES: u64 = 128 * 1024 * 1024;
const RGBEXP_BYTES: usize = size_of::<RgbExp32>();
const BSP_HEADER_AND_PADDING_BYTES: usize = 12 + LUMP_COUNT * 19 + 4;
const GAME_LUMP_DIRECTORY_BYTES: usize = 16;
const DISPLAY_CONVERSION: &str = "Each linear RGB channel uses (v / (1 + v))^(1 / 2.2), v = intensity / 255. Nearest sample reduction, no exposure adaptation or native game tone mapping.";

pub(super) struct BuildIdentity {
    token: u64,
    output: Result<compiler::FileIdentity, String>,
    source: Result<Option<compiler::FileIdentity>, String>,
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
        let identity = (|| {
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
            if upper_bound.is_none_or(|bytes| bytes as u64 > MAX_BSP_BYTES)
                || std::fs::metadata(output)
                    .map_err(|error| error.to_string())?
                    .len()
                    > MAX_BSP_BYTES
            {
                return Err(format!(
                    "BSP exceeds the {MAX_BSP_BYTES}-byte inspection budget"
                ));
            }
            let identity = compiler::FileIdentity::capture(output, &compiler::Control::default())
                .map_err(|error| error.to_string())?;
            identity
                .verify_bytes(&expected.to_bytes())
                .map_err(|error| error.to_string())?;
            identity
                .verify_metadata()
                .map_err(|error| error.to_string())?;
            Ok(identity)
        })();
        Self {
            token,
            output: identity,
            source,
        }
    }
}

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

    fn kind(self) -> LightingKind {
        match self {
            Self::Ldr => LightingKind::Ldr,
            Self::Hdr => LightingKind::Hdr,
        }
    }

    fn name(self) -> &'static str {
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
        let Some(build) = &self.compiled else {
            return json!({"status":"unavailable", "current":false, "diagnostics":["Compile the map successfully in this session before inspecting baked lightmaps"]});
        };
        let identity = &build.lightmap_identity;
        let mut diagnostics = Vec::new();
        if !self.current_snapshot(&build.snapshot) {
            diagnostics.push("The map changed since this successful build".to_owned());
        }
        if build.options != self.compile_options() {
            diagnostics.push("Build settings changed since this successful build".to_owned());
        }
        if build.content_epoch != self.content_epoch {
            diagnostics.push(
                "Mounted content or capture configuration changed since this build".to_owned(),
            );
        }
        if let Some(error) = self.instance_dependency_issue(
            build.instance_dependencies.as_ref(),
            &build.instance_source_path,
        ) {
            diagnostics.push(format!(
                "Instance source/dependencies changed or cannot be verified: {error}"
            ));
        }
        if self.compiling {
            diagnostics
                .push("A new build is running; this is the previous successful build".to_owned());
        }
        let output_issue = match &identity.output {
            Ok(output) => output
                .verify_metadata()
                .err()
                .map(|error| error.to_string()),
            Err(error) => Some(error.clone()),
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
                if self
                    .path
                    .as_ref()
                    .and_then(|path| path.canonicalize().ok())
                    .as_deref()
                    != Some(source.path())
                {
                    diagnostics.push("The source map path changed since this build".to_owned());
                }
            }
            Err(error) => {
                diagnostics.push(format!("Saved VMF source identity unavailable: {error}"))
            }
            Ok(None) if self.path.is_some() => diagnostics
                .push("The map was saved to a new source path after this build".to_owned()),
            Ok(None) => {}
        }
        json!({
            "status":if output_issue.is_some() {"unavailable"} else if diagnostics.is_empty() {"current"} else {"stale"},
            "current":diagnostics.is_empty(), "token":identity.token.to_string(),
            "bsp":build.path, "generation":build.snapshot.generation, "revision":build.snapshot.revision,
            "lighting":build.options.lighting.name(), "diagnostics":diagnostics,
            "source":identity.source.as_ref().ok().and_then(|source| source.as_ref()).map(compiler::FileIdentity::path),
        })
    }

    pub fn inspect_baked_lightmaps(&self, query: &Query) -> Result<Inspection, String> {
        if query.limit == 0 || query.limit > MAX_PAGE_FACES {
            return Err(format!("Face page limit must be 1–{MAX_PAGE_FACES}"));
        }
        if query.style_index >= 4 || query.map_index >= 4 {
            return Err("Style and map indices must be 0–3".into());
        }
        let mut report = json!({
            "build":self.baked_lightmap_build_state(), "channel":query.channel.name(),
            "face_identity":"Zero-based compiled BSP face index in the selected channel. No authored VMF face mapping is asserted.",
            "intensity_units":"Decoded linear RGBExp32 intensities; independent of the display image and not native screen brightness.",
            "channels":[], "faces":[], "selected":null, "diagnostics":[],
            "limits":{"bsp_bytes":MAX_BSP_BYTES,"page_faces":MAX_PAGE_FACES,"preview_side":MAX_PREVIEW_SIDE},
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
                report["build"]["status"] = json!("unavailable");
                report["build"]["current"] = json!(false);
                report["diagnostics"] =
                    json!([format!("Compiled BSP cannot be inspected: {error}")]);
                return Ok(Inspection {
                    report,
                    image: None,
                });
            }
        };
        if let Ok(Some(source)) = &build.lightmap_identity.source
            && let Err(error) = read_owned_bytes(source)
        {
            report["build"]["status"] = json!("stale");
            report["build"]["current"] = json!(false);
            report["build"]["diagnostics"]
                .as_array_mut()
                .expect("build diagnostics")
                .push(json!(format!("Saved VMF source changed: {error}")));
        }
        let inspected = inspect_bsp(&bsp, query);
        match inspected {
            Ok((details, image)) => {
                for (key, value) in details.as_object().expect("inspection object") {
                    report[key] = value.clone();
                }
                Ok(Inspection { report, image })
            }
            Err(error) => {
                report["channels"] = json!([
                    channel_summary(&bsp, Channel::Ldr),
                    channel_summary(&bsp, Channel::Hdr)
                ]);
                report["diagnostics"] = json!([error]);
                Ok(Inspection {
                    report,
                    image: None,
                })
            }
        }
    }
}

fn read_owned_bytes(identity: &compiler::FileIdentity) -> Result<Vec<u8>, String> {
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
    let file = std::fs::File::open(identity.path()).map_err(|error| error.to_string())?;
    if file.metadata().map_err(|error| error.to_string())?.len() > MAX_BSP_BYTES {
        return Err(format!(
            "BSP exceeds the {MAX_BSP_BYTES}-byte inspection limit"
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BSP_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_BSP_BYTES {
        return Err("BSP grew beyond the inspection limit".into());
    }
    identity
        .verify_bytes(&bytes)
        .map_err(|error| error.to_string())?;
    identity
        .verify_metadata()
        .map_err(|error| error.to_string())?;
    Ok(bytes)
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
    let Some(lighting) = bsp.lighting(kind).map_err(|error| error.to_string())? else {
        report["diagnostics"] = json!([
            "No baked samples in the requested channel. Compile with Direct LDR or Direct LDR + HDR; Fullbright has no baked lightmaps."
        ]);
        return Ok((report, None));
    };
    if query.offset > faces.len() {
        return Err("Face page offset exceeds the compiled face count".into());
    }
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
