use source_formats::gamefs::asset_relative;
use source_formats::keyvalues::Node;
use source_formats::vmt::{Material, texture_path};
use source_formats::vtf::{FLAG_NOMIP, Vtf};

use super::ORDINARY_COLOR_UNSUPPORTED_TEXTURE_FLAGS;

pub(crate) const FLAG: u32 = 256;
pub(crate) const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_MATERIALS: usize = 128;
const MAX_TIME_SECONDS: f64 = 3600.0;
const MAX_ASSET_BYTES: usize = 16 * 1024 * 1024;
const MAX_FRAMES: u16 = 64;
const MAX_AXIS: u32 = 512;
const MAX_RATE: f64 = 120.0;
const DEFAULT_RATE: f32 = 15.0;
const BINDING_BYTES: usize = 256;
const UPLOAD_ROW_ALIGNMENT: usize = 256;

pub fn checked_time(seconds: f64) -> Result<f32, String> {
    if !(0.0..=MAX_TIME_SECONDS).contains(&seconds) {
        return Err(format!(
            "Material time must be finite and between 0 and {MAX_TIME_SECONDS} seconds"
        ));
    }
    Ok(seconds as f32)
}

#[derive(Debug)]
pub(crate) enum Layer {
    Absent,
    Declared(Playback),
    Ready(Playback),
    Unavailable(String),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Playback {
    Fixed(u16),
    Loop { frames: u16, rate: f32 },
}

impl Playback {
    pub(crate) fn frames(self) -> std::ops::Range<u16> {
        match self {
            Self::Fixed(frame) => frame..frame + 1,
            Self::Loop { frames, .. } => 0..frames,
        }
    }

    pub(crate) fn binding_index(self, time: f32) -> usize {
        match self {
            Self::Fixed(_) => 0,
            Self::Loop { frames, rate } => ((time * rate) as usize) % usize::from(frames),
        }
    }

    pub(super) fn payload_bytes(self, texture: &Vtf) -> usize {
        let frame_bytes: usize = (0..texture.mip_count)
            .map(|mip| {
                let (width, height) = texture.mip_size(mip);
                (width as usize * 4).next_multiple_of(UPLOAD_ROW_ALIGNMENT) * height as usize
            })
            .sum();
        (frame_bytes + BINDING_BYTES) * self.frames().len()
    }
}

impl Layer {
    pub(super) fn resolve(
        material: &Material,
        base: Option<&Vtf>,
        display_texture: Option<&str>,
    ) -> Self {
        if !requested(material) && base.is_none_or(|texture| texture.frames == 1) {
            return Self::Absent;
        }
        match resolve(material, base, display_texture) {
            Ok(playback) => Self::Declared(playback),
            Err(error) => Self::Unavailable(error),
        }
    }

    pub(crate) fn ready(&self) -> Option<Playback> {
        match self {
            Self::Ready(playback) => Some(*playback),
            _ => None,
        }
    }
}

pub(super) fn requested(material: &Material) -> bool {
    material
        .get("$frame")
        .is_some_and(|value| value.trim().parse::<i32>() != Ok(0))
        || material.blocks.iter().any(|block| {
            block.key.eq_ignore_ascii_case("proxies")
                && block.children().is_some_and(|children| {
                    children
                        .iter()
                        .any(|proxy| proxy.key.eq_ignore_ascii_case("AnimatedTexture"))
                })
        })
}

fn resolve(
    material: &Material,
    base: Option<&Vtf>,
    display_texture: Option<&str>,
) -> Result<Playback, String> {
    if !matches!(
        material.shader.as_str(),
        "lightmappedgeneric" | "unlitgeneric"
    ) || material.is_tool()
    {
        return Err(
            "Base frame preview requires non-tool LightmappedGeneric or UnlitGeneric".into(),
        );
    }
    for (key, value) in &material.params {
        match key.as_str() {
            "$basetexture" | "$frame" | "$surfaceprop" | "$surfaceprop2" | "$nodecal"
            | "%keywords" | "%detailtype" => {}
            "$alphatest" | "$translucent" | "$additive" | "$selfillum" | "$vertexalpha"
            | "$vertexcolor"
                if value.trim().parse::<i32>() == Ok(0) => {}
            "$alpha" if value.trim().parse::<f64>() == Ok(1.0) => {}
            _ => {
                let key: String = key.chars().take(128).collect();
                return Err(format!(
                    "Base frame preview excludes material parameter {key}"
                ));
            }
        }
    }
    let path = material
        .get("$basetexture")
        .map(str::trim)
        .filter(|value| !value.is_empty() && asset_relative(value).is_ok())
        .map(texture_path);
    let texture = base
        .filter(|_| path.is_some() && path.as_deref() == display_texture)
        .ok_or("Base frame preview requires the exact declared base VTF")?;
    if texture.width > MAX_AXIS
        || texture.height > MAX_AXIS
        || texture.authored_frames == 0
        || texture.frames > MAX_FRAMES
        || texture.faces != 1
        || texture.depth != 1
        || texture.authored_depth == Some(0)
        || texture.first_frame != 0
        || texture.flags & ORDINARY_COLOR_UNSUPPORTED_TEXTURE_FLAGS != 0
        || !crate::normal_maps::ordinary_ldr_format(texture.format)
    {
        return Err(format!(
            "Base frame preview needs an ordinary repeat-sampled 2D LDR VTF with 1–{MAX_FRAMES} frames and axes 1–{MAX_AXIS}"
        ));
    }
    if texture.flags & FLAG_NOMIP != 0 && texture.mip_count != 1 {
        return Err("Base frame preview requires one stored mip with the No mip flag".into());
    }
    texture
        .validate_native_mips()
        .map_err(|error| error.to_string())?;
    let initial = match material.get("$frame") {
        None => 0,
        Some(value) => value
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|frame| *frame < texture.frames)
            .ok_or("$frame selects an unavailable base VTF frame")?,
    };
    let playback = if material.blocks.is_empty() {
        Playback::Fixed(initial)
    } else {
        let [proxies] = material.blocks.as_slice() else {
            return Err(
                "Base frame preview requires exactly one Proxies block and no fallbacks".into(),
            );
        };
        let Some([proxy]) = proxies
            .children()
            .filter(|_| proxies.key.eq_ignore_ascii_case("proxies"))
        else {
            return Err("Base frame preview supports one AnimatedTexture proxy".into());
        };
        Playback::Loop {
            frames: texture.frames,
            rate: proxy_rate(proxy)?,
        }
    };
    if playback.payload_bytes(texture).max(texture.encoded_size()) > MAX_ASSET_BYTES {
        return Err("Base frame preview exceeds the 16 MiB per-material frame budget".into());
    }
    Ok(playback)
}

fn proxy_rate(proxy: &Node) -> Result<f32, String> {
    let children = proxy
        .children()
        .filter(|_| proxy.key.eq_ignore_ascii_case("AnimatedTexture"))
        .ok_or("Base frame preview supports one AnimatedTexture proxy")?;
    let mut fields = [None; 4];
    let keys = [
        "animatedTextureVar",
        "animatedTextureFrameNumVar",
        "animatedTextureFrameRate",
        "animationNoWrap",
    ];
    for child in children {
        let index = keys
            .iter()
            .position(|key| child.key.eq_ignore_ascii_case(key))
            .ok_or("AnimatedTexture has an unsupported parameter")?;
        let value = child
            .as_str()
            .ok_or("AnimatedTexture parameters must be scalar")?
            .trim();
        if fields[index].replace(value).is_some() {
            return Err(format!("AnimatedTexture has a duplicate {}", keys[index]));
        }
    }
    if fields[0].is_none_or(|value| !value.eq_ignore_ascii_case("$basetexture"))
        || fields[1].is_none_or(|value| !value.eq_ignore_ascii_case("$frame"))
    {
        return Err("AnimatedTexture must target $basetexture and $frame".into());
    }
    if fields[3].is_some_and(|value| value.parse::<i32>() != Ok(0)) {
        return Err("AnimatedTexture preview requires a looping animation".into());
    }
    match fields[2] {
        None => Ok(DEFAULT_RATE),
        Some(value) => value
            .parse::<f64>()
            .ok()
            .filter(|rate| *rate > 0.0 && *rate <= MAX_RATE)
            .map(|rate| rate as f32)
            .filter(|rate| rate.is_normal())
            .ok_or_else(|| {
                format!("AnimatedTexture requires a positive finite rate up to {MAX_RATE} FPS")
            }),
    }
}
