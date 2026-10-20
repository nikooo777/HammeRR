use source_formats::gamefs::asset_relative;
use source_formats::vmt::{self, Material};

use super::{DimensionSource, parse_reflectivity};

pub(super) const NATIVE_NO_PREVIEW_DIMENSION: i32 = 128;
const MAX_TOOL_PREVIEW_NAME_BYTES: usize = 241;

#[derive(Clone)]
pub(super) struct HeaderProvider {
    pub key: &'static str,
    pub path: String,
}

impl HeaderProvider {
    fn new(material: &Material, key: &'static str) -> Result<Self, String> {
        let value = material.get(key).unwrap_or_default().trim();
        if value.is_empty() {
            return Err(format!("Selected {key} requires a nonblank texture path"));
        }
        if key == "%tooltexture" {
            if value.len() > MAX_TOOL_PREVIEW_NAME_BYTES {
                return Err(format!(
                    "Selected %tooltexture names longer than {MAX_TOOL_PREVIEW_NAME_BYTES} bytes are unsupported for relative preview dimensions"
                ));
            }
            let lower = value.to_ascii_lowercase();
            let unsigned = lower.trim_start_matches(['+', '-']);
            let numeric_root = !value.contains(['/', '\\'])
                && !lower.ends_with(".vtf")
                && (unsigned.starts_with("0x")
                    || unsigned.starts_with("nan(") && unsigned.ends_with(')'));
            if value.parse::<f64>().is_ok() || numeric_root {
                return Err(
                    "Selected %tooltexture numeric or numeric-looking scalar declarations are unsupported; provide a canonical texture path".into(),
                );
            }
        }
        asset_relative(value).map_err(|error| format!("Selected {key} texture path: {error}"))?;
        Ok(Self {
            key,
            path: vmt::texture_path(value),
        })
    }

    fn runtime(&self) -> bool {
        self.path
            .strip_prefix("materials/")
            .is_some_and(|name| name.starts_with("_rt_") && !name.contains('/'))
    }
}

pub(super) enum DimensionProvider {
    CompileDefault,
    Header(HeaderProvider, DimensionSource),
    NativeNoPreview,
}

pub(super) enum ReflectivityProvider {
    CompileDefault,
    Header(HeaderProvider),
    Authored([f32; 3]),
}

pub(super) struct MetricPlan {
    pub dimensions: DimensionProvider,
    pub reflectivity: ReflectivityProvider,
}

impl MetricPlan {
    pub fn new(material: &Material, primary: Option<&'static str>) -> Result<Self, String> {
        let authored = material
            .get("$reflectivity")
            .map(parse_reflectivity)
            .transpose()?;
        let no_preview = match material.get("%notooltexture").map(str::trim_ascii) {
            None => false,
            Some("0" | "1") => true,
            Some(_) => {
                return Err(
                    "%notooltexture declarations require canonical literal 0 or 1; other values are unsupported".into(),
                );
            }
        };
        let tool = if no_preview {
            None
        } else {
            material
                .get("%tooltexture")
                .map(|_| HeaderProvider::new(material, "%tooltexture"))
                .transpose()?
        };
        let diffuse = if (!no_preview && tool.is_none()) || authored.is_none() {
            primary
                .map(|key| HeaderProvider::new(material, key))
                .transpose()?
        } else {
            None
        };
        let dimensions = if no_preview {
            DimensionProvider::NativeNoPreview
        } else {
            match tool {
                Some(provider) => {
                    DimensionProvider::Header(provider, DimensionSource::ToolTextureHeader)
                }
                None => match &diffuse {
                    Some(provider) => DimensionProvider::Header(
                        provider.clone(),
                        DimensionSource::DiffuseTextureHeader,
                    ),
                    None => DimensionProvider::CompileDefault,
                },
            }
        };
        let reflectivity = match authored {
            Some(value) => ReflectivityProvider::Authored(value),
            None => match diffuse {
                Some(provider) => ReflectivityProvider::Header(provider),
                None => ReflectivityProvider::CompileDefault,
            },
        };
        Ok(Self {
            dimensions,
            reflectivity,
        })
    }

    pub fn headers(&self) -> [Option<&HeaderProvider>; 2] {
        [
            match &self.dimensions {
                DimensionProvider::Header(provider, _) => Some(provider),
                DimensionProvider::CompileDefault | DimensionProvider::NativeNoPreview => None,
            },
            match &self.reflectivity {
                ReflectivityProvider::Header(provider) => Some(provider),
                ReflectivityProvider::CompileDefault | ReflectivityProvider::Authored(_) => None,
            },
        ]
    }

    pub fn unavailable(&self) -> Option<String> {
        if matches!(self.dimensions, DimensionProvider::CompileDefault)
            && matches!(self.reflectivity, ReflectivityProvider::Authored(_))
        {
            return Some(
                "metrics are unavailable: authored $reflectivity requires selected texture dimensions; no eligible base texture is declared".into(),
            );
        }
        self.headers()
            .into_iter()
            .flatten()
            .find(|provider| provider.runtime())
            .map(|provider| {
                format!(
                    "selected {} {} uses a runtime-generated texture; its required {} metrics are unsupported",
                    provider.key,
                    provider.path,
                    self.demands(&provider.path),
                )
            })
    }

    pub fn demands(&self, path: &str) -> &'static str {
        let dimensions = matches!(
            &self.dimensions,
            DimensionProvider::Header(provider, _) if provider.path == path
        );
        let reflectivity = matches!(
            &self.reflectivity,
            ReflectivityProvider::Header(provider) if provider.path == path
        );
        match (dimensions, reflectivity) {
            (true, true) => "dimensions and reflectivity",
            (true, false) => "dimensions",
            _ => "reflectivity",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct TextureHeader {
    pub width: i32,
    pub height: i32,
    pub reflectivity: [f32; 3],
}
