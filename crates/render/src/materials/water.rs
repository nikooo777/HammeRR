use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::keyvalues;
use source_formats::vmt::{Material, material_path, texture_path};
use source_formats::vtf::{FLAG_NOMIP, ImageFormat, Vtf};

use crate::normal_maps::validate_static_texture;

pub(crate) const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_MATERIAL_BYTES: usize = 64 * 1024;
const MAX_ASSET_BYTES: usize = 8 * 1024 * 1024;
const MAX_AXIS: u32 = 1024;
const MAX_PATH_BYTES: usize = 240;
const MAX_FOG_DISTANCE: f32 = 32_768.0;
const MAX_REFRACT_AMOUNT: f32 = 16.0;
const TRILINEAR: u32 = 0x0000_0002;
const NORMAL: u32 = 0x0000_0080;
const NOLOD: u32 = 0x0000_0200;
const EIGHT_BIT_ALPHA: u32 = 0x0000_2000;
const ALLOWED_NORMAL_FLAGS: u32 = TRILINEAR | NORMAL | FLAG_NOMIP | NOLOD | EIGHT_BIT_ALPHA;

#[derive(Debug)]
pub(crate) enum Layer {
    Absent,
    Declared(Declaration),
    Ready(Box<Asset>),
    Unavailable(String),
}

#[derive(Debug)]
pub(crate) struct Declaration {
    path: String,
    bottom: String,
    fog_color: [f32; 3],
    fog_inverse_range: f32,
    refract_amount: f32,
}

#[derive(Debug)]
pub(crate) struct Asset {
    pub(crate) normal: Vtf,
    pub(crate) texel_size: [u32; 2],
    pub(crate) fog_color: [f32; 3],
    pub(crate) fog_inverse_range: f32,
    pub(crate) refract_amount: f32,
}

impl Layer {
    fn resolve(material: &Material) -> Self {
        if !requested(material) {
            return Self::Absent;
        }
        match Declaration::new(material) {
            Ok(declaration) => Self::Declared(declaration),
            Err(error) => Self::Unavailable(error),
        }
    }

    pub(crate) fn resolve_from_mount(fs: &GameFs, name: &str, material: &Material) -> Self {
        match Self::resolve(material) {
            Self::Declared(declaration) => match verify_source(fs, name, material) {
                Ok(()) => Self::Declared(declaration),
                Err(error) => Self::Unavailable(error),
            },
            layer => layer,
        }
    }

    pub(crate) fn ready(&self) -> Option<&Asset> {
        match self {
            Self::Ready(asset) => Some(asset),
            _ => None,
        }
    }
}

pub(super) fn requested(material: &Material) -> bool {
    material.shader == "water" || material.compile_flag("%compilewater")
}

pub(super) fn is_receiver(material: &Material, display: Option<&str>) -> bool {
    material.shader == "lightmappedgeneric"
        && material.blocks.is_empty()
        && material.params.iter().all(|(key, _)| {
            matches!(
                key.as_str(),
                "$basetexture"
                    | "$surfaceprop"
                    | "$surfaceprop2"
                    | "$reflectivity"
                    | "$nodecal"
                    | "%keywords"
            )
        })
        && resource_name(material, "$basetexture")
            .is_ok_and(|name| display == Some(texture_path(&name).as_str()))
}

pub(crate) fn verify_receiver(fs: &GameFs, info: &super::MaterialInfo) -> Result<(), String> {
    let material = super::resolved_material_limited(fs, &info.name, MAX_MATERIAL_BYTES)?
        .ok_or_else(|| format!("Water receiver {} was not found", info.name))?;
    if !is_receiver(&material, info.display_texture.as_deref())
        || info.material_identity != Some(super::material_identity(&material))
    {
        return Err(format!(
            "Water receiver {} changed or has an unsupported shader; reload material assets",
            info.name
        ));
    }
    verify_source(fs, &info.name, &material)
}

pub(crate) fn verify_nodraw(fs: &GameFs) -> Result<(), String> {
    let name = "tools/toolsnodraw";
    let material = super::resolved_material_limited(fs, name, MAX_MATERIAL_BYTES)?
        .ok_or("Water refraction needs mounted tools/toolsnodraw")?;
    let compile = material.compile_semantics();
    if compile.draws || compile.surface & source_formats::bsp::surf::NODRAW == 0 {
        return Err(
            "Water refraction needs tools/toolsnodraw to have native NoDraw semantics".into(),
        );
    }
    verify_source(fs, name, &material)
}

impl Declaration {
    fn new(material: &Material) -> Result<Self, String> {
        if material.shader != "water" || material.is_tool() {
            return Err("Water refraction requires a visible Water material".into());
        }
        if !material.blocks.is_empty() {
            return Err(
                "Water refraction requires a static material without proxies or fallback blocks"
                    .into(),
            );
        }
        for (key, value) in &material.params {
            match key.as_str() {
                "%compilewater" | "%tooltexture" | "%keywords" | "$surfaceprop" | "$abovewater"
                | "$forceexpensive" | "$reflecttexture" | "$reflecttint" | "$refracttexture"
                | "$refracttint" | "$refractamount" | "$normalmap" | "$bumptransform"
                | "$fogenable" | "$fogstart" | "$fogend" | "$fogcolor" | "$bottommaterial" => {}
                "$forcecheap" | "$reflectentities" | "$blurrefract" | "$bumpframe"
                    if value.trim() == "0" => {}
                "$reflectamount" if value.trim() == "0" => {}
                _ => {
                    let key: String = key.chars().take(128).collect();
                    return Err(format!(
                        "Water refraction excludes material parameter {key}"
                    ));
                }
            }
        }
        for key in [
            "%compilewater",
            "$abovewater",
            "$forceexpensive",
            "$fogenable",
        ] {
            if material.get(key).map(str::trim) != Some("1") {
                return Err(format!("Water refraction requires {key} 1"));
            }
        }
        for (key, target) in [
            ("$reflecttexture", "_rt_WaterReflection"),
            ("$refracttexture", "_rt_WaterRefraction"),
        ] {
            if !material
                .get(key)
                .is_some_and(|value| value.trim().eq_ignore_ascii_case(target))
            {
                return Err(format!("Water refraction requires {key} {target}"));
            }
        }
        if color(material, "$reflecttint")? != [0.0; 3] {
            return Err("Water refraction requires zero $reflecttint".into());
        }
        if material.get("$refracttint").is_some() && color(material, "$refracttint")? != [1.0; 3] {
            return Err("Water refraction requires identity $refracttint".into());
        }
        if !super::uv::is_axis_identity(material.get("$bumptransform")) {
            return Err("Water refraction requires an identity $bumptransform".into());
        }
        let bottom = resource_name(material, "$bottommaterial")?;
        let path = texture_path(&resource_name(material, "$normalmap")?);
        let tool = texture_path(&resource_name(material, "%tooltexture")?);
        if tool != path {
            return Err("Water refraction requires %tooltexture to use the $normalmap for surface UV dimensions".into());
        }
        let fog_color = color(material, "$fogcolor")?;
        if fog_color
            .iter()
            .any(|&channel| channel != 0.0 && channel != 1.0)
        {
            return Err("Water refraction requires endpoint $fogcolor channels (0 or 1)".into());
        }
        let fog_start = scalar(material, "$fogstart", 0.0, MAX_FOG_DISTANCE)?;
        let fog_end = scalar(material, "$fogend", 0.0, MAX_FOG_DISTANCE)?;
        let range = fog_end - fog_start;
        if range < 1.0 {
            return Err(
                "Water refraction requires $fogend at least one unit beyond $fogstart".into(),
            );
        }
        let refract_amount = scalar(
            material,
            "$refractamount",
            -MAX_REFRACT_AMOUNT,
            MAX_REFRACT_AMOUNT,
        )?;
        Ok(Self {
            path,
            bottom,
            fog_color,
            fog_inverse_range: range.recip(),
            refract_amount,
        })
    }

    pub(crate) fn load(self, fs: &GameFs, remaining_budget: usize) -> Result<Asset, String> {
        if remaining_budget == 0 {
            return Err(cache_limit());
        }
        let bottom = Material::load_limited(fs, &self.bottom, MAX_MATERIAL_BYTES)
            .map_err(|error| format!("Water bottom {}: {error}", self.bottom))?
            .ok_or_else(|| format!("Water bottom {} was not found", self.bottom))?;
        if bottom.shader != "water" || bottom.get("$abovewater").map(str::trim) != Some("0") {
            return Err(format!(
                "Water bottom {} needs Water with $abovewater 0",
                self.bottom
            ));
        }
        let normal = super::read_vtf(fs, &self.path, Some(MAX_ASSET_BYTES.min(remaining_budget)))
            .map_err(|error| format!("Water normal {error}"))?
            .ok_or_else(|| format!("Water normal {} was not found", self.path))?;
        validate_normal(&normal).map_err(|error| format!("Water normal {}: {error}", self.path))?;
        let asset = Asset {
            texel_size: [normal.width, normal.height],
            normal,
            fog_color: self.fog_color,
            fog_inverse_range: self.fog_inverse_range,
            refract_amount: self.refract_amount,
        };
        if asset.cache_bytes() > remaining_budget {
            return Err(cache_limit());
        }
        Ok(asset)
    }
}

impl Asset {
    pub(crate) fn cache_bytes(&self) -> usize {
        let gpu_bytes = (0..self.normal.mip_count)
            .map(|mip| {
                let (width, height) = self.normal.mip_size(mip);
                width as usize * height as usize * 4
            })
            .sum::<usize>();
        self.normal.encoded_size() + gpu_bytes
    }
}

fn verify_source(fs: &GameFs, name: &str, material: &Material) -> Result<(), String> {
    let path = material_path(name);
    let bytes = fs
        .read_limited(&path, MAX_MATERIAL_BYTES)
        .map_err(|error| format!("Water material {path}: {error}"))?
        .ok_or_else(|| format!("Water material {path} was not found"))?;
    let text =
        std::str::from_utf8(&bytes).map_err(|error| format!("Water material {path}: {error}"))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let roots = keyvalues::parse_static_material_bounded(text, 2, 128)
        .map_err(|error| format!("Water material {path}: {error}"))?;
    let [root] = roots.as_slice() else {
        return Err(format!(
            "Water refraction requires one direct {} block in {path}",
            material.shader
        ));
    };
    let children = root
        .children()
        .filter(|_| root.key.eq_ignore_ascii_case(&material.shader))
        .ok_or_else(|| {
            format!(
                "Water refraction requires a direct {} block in {path}",
                material.shader
            )
        })?;
    if children.len() != material.params.len()
        || children
            .iter()
            .zip(&material.params)
            .any(|(node, (key, value))| {
                !node.key.eq_ignore_ascii_case(key) || node.as_str() != Some(value)
            })
    {
        return Err(format!(
            "Water refraction requires unchanged, unique scalar parameters in {path}"
        ));
    }
    Ok(())
}

fn resource_name(material: &Material, key: &str) -> Result<String, String> {
    let value = material.get(key).unwrap_or_default().trim();
    let normalized = asset_relative(value).ok();
    normalized
        .filter(|name| {
            !name.is_empty()
                && name.len() <= MAX_PATH_BYTES
                && name.eq_ignore_ascii_case(value)
                && !name.starts_with("materials/")
                && !name.starts_with("_rt_")
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'/'))
                && name.parse::<f32>().is_err()
        })
        .ok_or_else(|| {
            format!("Water refraction requires a canonical mounted asset name for {key}")
        })
}

fn color(material: &Material, key: &str) -> Result<[f32; 3], String> {
    let value = material
        .get(key)
        .ok_or_else(|| format!("Water refraction requires explicit {key}"))?;
    super::emission::tint(value, key)
}

fn scalar(material: &Material, key: &str, low: f32, high: f32) -> Result<f32, String> {
    material
        .get(key)
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| (low..=high).contains(value) && !value.is_subnormal())
        .ok_or_else(|| format!("Water refraction requires {key} in {low}..={high}"))
}

fn validate_normal(normal: &Vtf) -> Result<(), String> {
    validate_static_texture(normal, MAX_AXIS)?;
    if !matches!(normal.format, ImageFormat::Rgba8888 | ImageFormat::Bgra8888)
        || normal.flags & !ALLOWED_NORMAL_FLAGS != 0
        || normal.flags & (NORMAL | EIGHT_BIT_ALPHA) != NORMAL | EIGHT_BIT_ALPHA
        || normal.first_frame != 0
        || !normal.width.is_power_of_two()
        || !normal.height.is_power_of_two()
    {
        return Err("Requires a static linear repeat-sampled RGBA8888 or BGRA8888 normal with eight-bit alpha and power-of-two dimensions".into());
    }
    let expected_mips = if normal.flags & FLAG_NOMIP != 0 {
        1
    } else {
        if normal.flags & TRILINEAR == 0 {
            return Err("Water normal with mips requires trilinear filtering".into());
        }
        normal.width.max(normal.height).ilog2() + 1
    };
    if u32::from(normal.mip_count) != expected_mips {
        return Err("Water normal requires its complete declared mip chain".into());
    }
    normal
        .validate_native_mips()
        .map_err(|error| error.to_string())?;
    for mip in 0..normal.mip_count {
        normal
            .image_data(mip, 0, 0)
            .map_err(|error| format!("Invalid water normal mip {mip}: {error}"))?;
    }
    Ok(())
}

fn cache_limit() -> String {
    "Water normal payload exceeds the 32 MiB cache budget".into()
}
