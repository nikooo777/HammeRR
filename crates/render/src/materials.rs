//! CPU-side material loading: VMT parameters plus the textures to display.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::vmt::{Material, material_path, texture_path};
use source_formats::vtf::Vtf;

pub(crate) mod animation;
pub(crate) mod detail;
mod emission;
pub(crate) mod environment;
mod phong;
pub(crate) mod uv;
pub(crate) mod water;

/// Texture size assumed for UV mapping when a material can't be loaded.
const FALLBACK_SIZE: u32 = 128;
pub const MAX_RELOAD_FAILURES: usize = 100;
// Point/clamp sampling, normal/procedural/cube/target/vertex textures, SSBump and border flags.
const ORDINARY_COLOR_UNSUPPORTED_TEXTURE_FLAGS: u32 = 0x2e01_c88d;

pub(crate) fn validate_ordinary_color_texture(
    texture: &Vtf,
    maximum_axis: u32,
) -> Result<(), String> {
    crate::normal_maps::validate_static_texture(texture, maximum_axis)?;
    if !crate::normal_maps::ordinary_ldr_format(texture.format)
        || texture.flags & ORDINARY_COLOR_UNSUPPORTED_TEXTURE_FLAGS != 0
        || texture.first_frame != 0
    {
        return Err("Requires an ordinary static repeat-sampled LDR texture".into());
    }
    for mip in 0..texture.mip_count {
        texture
            .image_data(mip, 0, 0)
            .map_err(|error| format!("Invalid texture mip {mip}: {error}"))?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub struct Emission {
    pub tint: [f32; 3],
    pub base_alpha: bool,
}

struct DisplayTextures {
    base: Option<Vtf>,
    blend: Option<Vtf>,
    base_path: Option<String>,
}

#[derive(Debug)]
pub struct MaterialInfo {
    pub(crate) reference: bool,
    pub(crate) detail: detail::Layer,
    pub(crate) environment: environment::Layer,
    pub(crate) animation: animation::Layer,
    pub(crate) water: water::Layer,
    pub(crate) water_receiver: bool,
    pub emission: Option<Emission>,
    emission_error: Option<String>,
    pub(crate) emission_model_only: bool,
    pub(crate) base_transform: Result<Option<uv::BaseTransform>, String>,
    pub lighting_error: Option<String>,
    pub terrain_lighting_error: Option<String>,
    pub(crate) terrain_blend_bumped: bool,
    pub(crate) declares_normals: bool,
    pub(crate) model_bumped: bool,
    pub(crate) model_normal_error: Option<String>,
    pub(crate) model_phong: Option<phong::ModelPhong>,
    pub model_lighting_error: Option<String>,
    pub normal: Option<Arc<crate::normal_maps::Asset>>,
    pub display_texture: Option<String>,
    pub blend_texture: Option<String>,
    material_identity: Option<u64>,
    /// Material name as used in the map, lowercase (`de_cbble/cobbleroad01`).
    pub name: String,
    pub base: Option<Vtf>,
    /// Second layer of a blend material (`$basetexture2`), blended by
    /// displacement vertex alpha.
    pub blend: Option<Vtf>,
    /// Size of the displayed texture, used to normalize texture coordinates.
    pub width: u32,
    pub height: u32,
    pub alpha_test: bool,
    pub translucent: bool,
    pub tool: bool,
    pub sky: bool,
    pub sky3d_eligible: bool,
    /// No usable display texture is available.
    pub missing: bool,
    pub error: Option<String>,
}

impl MaterialInfo {
    pub(crate) fn emission_for(&self, model: bool) -> Result<bool, &str> {
        if let Some(error) = &self.emission_error {
            return Err(error);
        }
        if self.emission.is_some_and(|emission| emission.base_alpha)
            && self.emission_model_only != model
        {
            return Err("Self-illumination requires the matching brush or model shader");
        }
        Ok(self.emission.is_some())
    }

    pub fn source_identity(&self) -> Option<u64> {
        self.material_identity
    }

    pub fn matches_material(&self, material: &Material) -> bool {
        self.material_identity == Some(material_identity(material))
    }

    pub fn load(fs: &GameFs, name: &str) -> Self {
        let name = name.to_ascii_lowercase();
        match Self::load_checked(fs, &name, None) {
            Ok(material) => material,
            Err(error) => Self::failed(&name, error, false),
        }
    }

    fn load_checked(fs: &GameFs, name: &str, limits: Option<[usize; 2]>) -> Result<Self, String> {
        let material = match limits {
            Some([maximum, _]) => resolved_material_limited(fs, name, maximum)?,
            None => {
                Material::load(fs, name).map_err(|error| format!("Material {name}: {error}"))?
            }
        };
        match Self::display_textures(fs, name, material.as_ref(), limits.map(|limits| limits[1])) {
            Ok(textures) => {
                let mut info = Self::from_resolved(
                    name,
                    material.as_ref(),
                    textures.base,
                    textures.blend,
                    textures.base_path,
                );
                if let Some(material) = &material {
                    info.water = water::Layer::resolve_from_mount(fs, name, material);
                }
                Ok(info)
            }
            Err(error) if limits.is_none() => {
                let resolved = material.as_ref();
                let mut failed = Self::failed(
                    name,
                    error.clone(),
                    resolved.is_some_and(emission::requested),
                );
                if resolved.is_some_and(|material| material.get("$basetexturetransform").is_some())
                {
                    failed.base_transform = Err(format!("Base texture transform: {error}"));
                }
                if resolved.is_some_and(environment::requested) {
                    failed.environment = environment::Layer::Unavailable(error.clone());
                }
                if resolved.is_some_and(detail::requested) {
                    failed.detail = detail::Layer::Unavailable(error.clone());
                }
                if resolved.is_some_and(water::requested) {
                    failed.water = water::Layer::Unavailable(error.clone());
                }
                if resolved.is_some_and(|material| {
                    material.get("$frame").is_some() || animation::requested(material)
                }) {
                    failed.animation = animation::Layer::Unavailable(error);
                }
                Ok(failed)
            }
            Err(error) => Err(error),
        }
    }

    fn failed(name: &str, error: String, emission_requested: bool) -> Self {
        let mut material = Self::untextured(name);
        material.width = FALLBACK_SIZE;
        material.height = FALLBACK_SIZE;
        material.missing = true;
        material.emission_error = emission_requested.then(|| error.clone());
        material.error = Some(error);
        material
    }

    fn display_textures(
        fs: &GameFs,
        name: &str,
        material: Option<&Material>,
        maximum: Option<usize>,
    ) -> Result<DisplayTextures, String> {
        let mut base = None;
        let mut base_path = None;
        if let Some(material) = material {
            for path in material
                .editor_textures()
                .map_err(|error| format!("Material {name}: {error}"))?
            {
                if let Some(texture) = read_vtf(fs, &path, maximum)? {
                    base = Some(texture);
                    base_path = Some(path);
                    break;
                }
            }
            if base.is_none() && material.primary_base_texture_key()? == Some("$albedo") {
                return Err(format!(
                    "Material {name}: selected $albedo display texture was not found"
                ));
            }
        }
        let blend = material
            .filter(|m| m.shader == "worldvertextransition")
            .and_then(|m| m.get("$basetexture2"))
            .map(|texture| {
                let path = texture_path(texture);
                read_vtf(fs, &path, maximum)?
                    .ok_or_else(|| format!("Blend texture {path} was not found"))
            })
            .transpose()?;
        Ok(DisplayTextures {
            base,
            blend,
            base_path,
        })
    }

    pub(crate) fn from_resolved(
        name: &str,
        material: Option<&Material>,
        base: Option<Vtf>,
        blend: Option<Vtf>,
        display_texture: Option<String>,
    ) -> Self {
        let (width, height) = base
            .as_ref()
            .map_or((FALLBACK_SIZE, FALLBACK_SIZE), |v| (v.width, v.height));
        let sky = material.is_some_and(|material| {
            material.compile_semantics().surface & source_formats::bsp::surf::SKY != 0
        }) || matches!(name, "tools/toolsskybox" | "tools/toolsskybox2d");
        let sky3d_eligible = material.is_some_and(|material| {
            let flags = material.compile_semantics().surface;
            flags & source_formats::bsp::surf::SKY != 0
                && flags & source_formats::bsp::surf::SKY2D == 0
        });
        let declares_normals = material.is_some_and(crate::normal_maps::declares_normals);
        let phong_requested = material.is_some_and(phong::requested);
        let model_bumped = (declares_normals || phong_requested)
            && material.is_some_and(|material| material.shader == "vertexlitgeneric");
        let (model_phong, phong_error) = match material
            .filter(|_| phong_requested)
            .map(phong::resolve)
            .transpose()
        {
            Ok(profile) => (profile, None),
            Err(error) => (None, Some(error)),
        };
        let model_normal_error = phong_error.or_else(|| {
            material.filter(|_| model_bumped).and_then(|material| {
                model_lighting_error(
                    material,
                    base.as_ref(),
                    display_texture.as_deref(),
                    true,
                    false,
                    model_phong.is_some(),
                )
            })
        });
        let animation = material.map_or(animation::Layer::Absent, |material| {
            animation::Layer::resolve(material, base.as_ref(), display_texture.as_deref())
        });
        let animated = matches!(animation, animation::Layer::Declared(_));
        let unlit_animation =
            animated && material.is_some_and(|material| material.shader == "unlitgeneric");
        let (emission, emission_error) = if unlit_animation {
            (
                Some(Emission {
                    tint: [1.0; 3],
                    base_alpha: false,
                }),
                None,
            )
        } else {
            match material.map_or(Ok(None), |material| {
                emission::resolve(material, base.as_ref(), display_texture.as_deref())
            }) {
                Ok(emission) => (emission, None),
                Err(error) => (None, Some(error)),
            }
        };
        let masked_emission = emission.is_some_and(|emission| emission.base_alpha);
        let base_transform = material.map_or(Ok(None), |material| {
            uv::resolve(material, base.as_ref(), display_texture.as_deref())
        });
        let detail = material.map_or(detail::Layer::Absent, |material| {
            detail::Layer::resolve(material, base.as_ref(), display_texture.as_deref())
        });
        let static_detail = matches!(detail, detail::Layer::Declared(_));
        let environment = material.map_or(environment::Layer::Absent, |material| {
            environment::Layer::resolve(material, base.as_ref(), display_texture.as_deref())
        });

        Self {
            reference: false,
            detail,
            environment,
            animation,
            water: water::Layer::Absent,
            water_receiver: material
                .is_some_and(|material| water::is_receiver(material, display_texture.as_deref())),
            emission,
            emission_error,
            emission_model_only: material
                .is_some_and(|material| material.shader == "vertexlitgeneric"),
            material_identity: material.map(material_identity),
            lighting_error: material.map_or_else(
                || Some("No resolved material".into()),
                |material| {
                    if static_detail || animated && material.shader == "lightmappedgeneric" {
                        None
                    } else {
                        lighting_error(material, masked_emission)
                    }
                },
            ),
            terrain_lighting_error: material.map_or_else(
                || Some("No resolved material".into()),
                |material| {
                    if static_detail || animated && material.shader == "lightmappedgeneric" {
                        None
                    } else {
                        terrain_lighting_error(
                            material,
                            base.as_ref(),
                            blend.as_ref(),
                            display_texture.as_deref(),
                            masked_emission,
                        )
                    }
                },
            ),
            terrain_blend_bumped: material.is_some_and(|material| {
                material.shader == "worldvertextransition" && declares_normals
            }),
            declares_normals: declares_normals || phong_requested,
            model_bumped,
            model_normal_error: model_normal_error.clone(),
            model_phong: model_phong.filter(|_| model_normal_error.is_none()),
            model_lighting_error: if phong_requested {
                model_normal_error.or_else(|| {
                    Some(
                        "Model Phong requires Normal map preview and Detail authored lighting"
                            .into(),
                    )
                })
            } else {
                material.map_or_else(
                    || Some("No resolved material".into()),
                    |material| {
                        base_transform.as_ref().err().cloned().or_else(|| {
                            model_lighting_error(
                                material,
                                base.as_ref(),
                                display_texture.as_deref(),
                                false,
                                masked_emission,
                                false,
                            )
                        })
                    },
                )
            },
            base_transform,
            display_texture,
            blend_texture: material
                .filter(|material| material.shader == "worldvertextransition")
                .and_then(|material| material.get("$basetexture2"))
                .map(texture_path),
            normal: None,
            error: None,
            missing: base.is_none(),
            alpha_test: material.is_some_and(Material::alpha_test),
            translucent: material.is_some_and(Material::translucent),
            tool: sky || material.is_some_and(Material::is_tool),
            sky,
            sky3d_eligible,
            name: name.to_owned(),
            base,
            blend,
            width,
            height,
        }
    }

    /// A flat-colored material for editor helpers such as entity boxes.
    pub fn untextured(name: &str) -> Self {
        Self {
            reference: false,
            detail: detail::Layer::Absent,
            environment: environment::Layer::Absent,
            animation: animation::Layer::Absent,
            water: water::Layer::Absent,
            water_receiver: false,
            emission: None,
            emission_error: None,
            emission_model_only: false,
            base_transform: Ok(None),
            material_identity: None,
            lighting_error: Some("Editor helper or unavailable material".into()),
            terrain_lighting_error: Some("Editor helper or unavailable material".into()),
            terrain_blend_bumped: false,
            declares_normals: false,
            model_bumped: false,
            model_normal_error: None,
            model_phong: None,
            model_lighting_error: Some("Editor helper or unavailable material".into()),
            display_texture: None,
            blend_texture: None,
            normal: None,
            name: name.to_owned(),
            base: None,
            blend: None,
            width: 1,
            height: 1,
            alpha_test: false,
            translucent: false,
            tool: false,
            sky: false,
            sky3d_eligible: false,
            missing: false,
            error: None,
        }
    }
}

fn lighting_error(material: &Material, masked_emission: bool) -> Option<String> {
    if material.shader != "lightmappedgeneric" {
        return Some("Authored lighting requires LightmappedGeneric".into());
    }
    diffuse_lighting_error(material, masked_emission)
}

fn diffuse_lighting_error(material: &Material, masked_emission: bool) -> Option<String> {
    let reason = if !material.blocks.is_empty() {
        "Authored lighting excludes proxies and nested fallback/effect blocks"
    } else if material.translucent() || material.alpha_test() {
        "Authored lighting excludes translucent and alpha-tested materials"
    } else if material.params.iter().any(|(key, _)| {
        !masked_emission && key.starts_with("$selfillum")
            || ["%tooltexture", "$additive"].contains(&key.as_str())
    }) {
        "Authored lighting excludes self-illumination, additive and base display overrides"
    } else {
        return None;
    };
    Some(reason.into())
}

fn model_lighting_error(
    material: &Material,
    base: Option<&Vtf>,
    display_texture: Option<&str>,
    normal_preview: bool,
    masked_emission: bool,
    phong: bool,
) -> Option<String> {
    if material.shader != "vertexlitgeneric" {
        return Some("Model authored lighting requires VertexLitGeneric".into());
    }
    if let Some(reason) = diffuse_lighting_error(material, masked_emission) {
        return Some(reason);
    }
    if let Some((key, _)) = material.params.iter().find(|(key, value)| {
        !(matches!(
            key.as_str(),
            "$basetexture"
                | "$basetexturetransform"
                | "$model"
                | "$surfaceprop"
                | "$surfaceprop2"
                | "$nodecal"
                | "%keywords"
        ) || phong::allowed_parameter(key, value, phong)
            || normal_preview && matches!(key.as_str(), "$bumpmap" | "$bumptransform")
            || masked_emission && matches!(key.as_str(), "$selfillum" | "$selfillumtint"))
    }) {
        let key: String = key.chars().take(256).collect();
        let mode = if phong {
            "Phong"
        } else if normal_preview {
            "normal preview"
        } else {
            "authored lighting"
        };
        return Some(format!("Model {mode} excludes unsupported parameter {key}"));
    }
    let declared = material
        .get("$basetexture")
        .filter(|value| {
            !normal_preview || !value.trim().is_empty() && asset_relative(value.trim()).is_ok()
        })
        .map(texture_path);
    if declared.as_deref() != display_texture || display_texture.is_none() {
        return Some("Model authored lighting needs its declared base display texture".into());
    }
    let Some(base) = base else {
        return Some("Model authored lighting needs an available base display texture".into());
    };
    if normal_preview {
        if let Err(error) = crate::normal_maps::validate_static_texture(base, u32::from(u16::MAX)) {
            return Some(format!("Model normal preview base texture: {error}"));
        }
        if base.flags & ORDINARY_COLOR_UNSUPPORTED_TEXTURE_FLAGS != 0 {
            return Some(
                "Model normal preview needs an ordinary repeat-sampled base texture".into(),
            );
        }
    }
    if base.frames != 1 || base.faces != 1 || base.depth != 1 {
        return Some(
            "Model authored lighting requires a static single-frame, single-face 2D base".into(),
        );
    }
    if !(crate::normal_maps::ordinary_ldr_format(base.format)
        || masked_emission && base.format == source_formats::vtf::ImageFormat::Dxt1OneBitAlpha)
    {
        return Some("Model authored lighting requires an ordinary LDR RGB base format".into());
    }
    None
}

fn terrain_blend_lighting_error(
    material: &Material,
    base: Option<&Vtf>,
    blend: Option<&Vtf>,
    display_texture: Option<&str>,
) -> Option<String> {
    let bumped = crate::normal_maps::declares_normals(material);
    if let Some(reason) = diffuse_lighting_error(material, false) {
        return Some(reason);
    }
    if let Some((key, _)) = material.params.iter().find(|(key, _)| {
        !(matches!(
            key.as_str(),
            "$basetexture"
                | "$basetexture2"
                | "$surfaceprop"
                | "$surfaceprop2"
                | "$nodecal"
                | "%keywords"
        ) || bumped
            && matches!(
                key.as_str(),
                "$basetexturetransform"
                    | "$bumpmap"
                    | "$bumpmap2"
                    | "$bumptransform"
                    | "$bumptransform2"
            ))
    }) {
        let key: String = key.chars().take(256).collect();
        return Some(format!(
            "Terrain blend authored lighting excludes unsupported parameter {key}"
        ));
    }
    let declared = material
        .get("$basetexture")
        .filter(|value| {
            !value.trim().is_empty() && (!bumped || asset_relative(value.trim()).is_ok())
        })
        .map(texture_path);
    if declared.as_deref() != display_texture || declared.is_none() {
        return Some(
            "Terrain blend authored lighting requires its exact declared first base texture".into(),
        );
    }
    if material.get("$basetexture2").is_none_or(|value| {
        value.trim().is_empty() || bumped && asset_relative(value.trim()).is_err()
    }) {
        return Some(
            "Terrain blend authored lighting requires a declared second base texture".into(),
        );
    }
    for (label, texture) in [("first", base), ("second", blend)] {
        let Some(texture) = texture else {
            return Some(format!(
                "Terrain blend authored lighting requires its available {label} base texture"
            ));
        };
        if let Err(error) =
            crate::normal_maps::validate_static_texture(texture, u32::from(u16::MAX))
        {
            return Some(format!("Terrain blend {label} base: {error}"));
        }
        if !crate::normal_maps::ordinary_ldr_format(texture.format)
            || texture.flags & ORDINARY_COLOR_UNSUPPORTED_TEXTURE_FLAGS != 0
        {
            return Some(format!(
                "Terrain blend {label} base requires an ordinary LDR repeat-sampled color texture"
            ));
        }
    }
    None
}

fn terrain_lighting_error(
    material: &Material,
    base: Option<&Vtf>,
    blend: Option<&Vtf>,
    display_texture: Option<&str>,
    masked_emission: bool,
) -> Option<String> {
    if material.shader == "worldvertextransition" {
        return terrain_blend_lighting_error(material, base, blend, display_texture);
    }
    lighting_error(material, masked_emission).or_else(|| {
        material.params.iter().find_map(|(key, value)| {
            if key == "$ssbump" && value.trim() == "1" {
                Some("SSBump terrain uses fixed editor shading".into())
            } else if (key.starts_with("$bump") && !["$bumpmap", "$bumptransform"].contains(&key.as_str()))
                || key.starts_with("$normal") || (key.starts_with("$ssbump") && (key != "$ssbump" || value.trim() != "0"))
                || key == "$forcebump" {
                Some("Terrain authored lighting excludes declared bump and normal maps".into())
            } else if key.starts_with("$detail")
                || key.starts_with("$blend")
                || key.starts_with("$basetexture2")
                || key.starts_with("$texture2")
                || key.starts_with("$envmap")
                || key.starts_with("$phong")
                || key == "$lightwarptexture"
            {
                Some("Terrain authored lighting requires a single base without extra, blend or detail layers".into())
            } else {
                None
            }
        })
    })
}

fn hash_nodes(
    nodes: &[source_formats::keyvalues::Node],
    hash: &mut std::collections::hash_map::DefaultHasher,
) {
    nodes.len().hash(hash);
    for node in nodes {
        node.key.hash(hash);
        match &node.value {
            source_formats::keyvalues::Value::String(value) => {
                0u8.hash(hash);
                value.hash(hash);
            }
            source_formats::keyvalues::Value::Block(nodes) => {
                1u8.hash(hash);
                hash_nodes(nodes, hash);
            }
        }
    }
}

pub fn material_identity(material: &Material) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    material.shader.hash(&mut hash);
    material.params.hash(&mut hash);
    hash_nodes(&material.blocks, &mut hash);
    hash.finish()
}

pub fn resolved_material_limited(
    fs: &GameFs,
    name: &str,
    maximum: usize,
) -> Result<Option<Material>, String> {
    Material::load_with_reader(&material_path(name), &mut |path| {
        fs.read_limited(&asset_relative(path)?, maximum)
    })
    .map_err(|error| format!("Material {name}: {error}"))
}

fn read_vtf(fs: &GameFs, path: &str, maximum: Option<usize>) -> Result<Option<Vtf>, String> {
    let bytes = match maximum {
        Some(maximum) => fs
            .read_asset_limited(path, maximum)
            .map(|asset| asset.map(|asset| asset.data)),
        None => fs.read(path),
    };
    bytes
        .map_err(|error| format!("{path}: {error}"))?
        .map(|bytes| Vtf::parse(bytes).map_err(|error| format!("{path}: {error}")))
        .transpose()
}

/// Index of a material in a [`MaterialLibrary`].
pub type MaterialId = usize;

/// Materials loaded so far, kept across scene rebuilds so edits only
/// rebuild geometry. Ids are stable for the library's lifetime.
#[derive(Debug)]
pub struct MaterialLibrary {
    normals: crate::normal_maps::Library,
    detail_bytes: usize,
    water_bytes: usize,
    environment_budget: environment::Budget,
    animation_bytes: usize,
    animated_materials: usize,
    pub icons: crate::icons::Library,
    hdr_skybox: Option<Arc<crate::sky::Skybox>>,
    generation: u64,
    materials: Vec<MaterialInfo>,
    by_name: HashMap<String, MaterialId>,
    unavailable_model: Option<MaterialId>,
    skyboxes: HashMap<String, Arc<crate::sky::Skybox>>,
}

impl Default for MaterialLibrary {
    fn default() -> Self {
        Self {
            normals: crate::normal_maps::Library::default(),
            detail_bytes: 0,
            water_bytes: 0,
            environment_budget: environment::Budget::default(),
            animation_bytes: 0,
            animated_materials: 0,
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
            icons: crate::icons::Library::default(),
            hdr_skybox: None,
            materials: vec![MaterialInfo::untextured(Self::EDITOR_NAME)],
            by_name: HashMap::from([(Self::EDITOR_NAME.to_owned(), Self::EDITOR)]),
            unavailable_model: None,
            skyboxes: HashMap::new(),
        }
    }
}

#[derive(Debug, Default)]
pub struct ReloadReport {
    pub reloaded: usize,
    pub failed: usize,
    pub failures: Vec<(String, String)>,
}

impl ReloadReport {
    pub fn record_failure(&mut self, name: String, error: String) {
        self.failed += 1;
        if self.failures.len() < MAX_RELOAD_FAILURES {
            self.failures.push((name, error));
        }
    }
}

impl MaterialLibrary {
    pub fn flat(names: impl IntoIterator<Item = String>) -> Self {
        let mut library = Self::default();
        for name in names {
            let key = name.to_ascii_lowercase();
            if !library.by_name.contains_key(&key) {
                let id = library.materials.len();
                library.materials.push(MaterialInfo::untextured(&key));
                library.by_name.insert(key, id);
            }
        }
        library
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn reload(&mut self, fs: &GameFs) -> ReloadReport {
        let mut report = ReloadReport::default();
        self.normals.clear();
        self.detail_bytes = 0;
        self.water_bytes = 0;
        self.environment_budget = environment::Budget::default();
        self.animation_bytes = 0;
        self.animated_materials = 0;
        self.icons.clear();
        self.hdr_skybox = None;
        for (id, material) in self.materials.iter_mut().enumerate().skip(1) {
            if material.reference || Some(id) == self.unavailable_model {
                continue;
            }
            *material = MaterialInfo::load(fs, &material.name);
            report.reloaded += 1;
            if let Some(error) = &material.error {
                report.record_failure(material.name.clone(), error.clone());
            } else if material.missing {
                report.record_failure(
                    material.name.clone(),
                    "No usable display texture was found".into(),
                );
            }
        }
        let mut skies = self.skyboxes.keys().cloned().collect::<Vec<_>>();
        skies.sort();
        self.skyboxes.clear();
        for name in skies {
            let sky = self.skybox(fs, &name);
            for face in &sky.faces {
                if let Some(error) = &face.issue {
                    report.record_failure(face.material.clone(), error.clone());
                }
            }
        }
        self.generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        report
    }

    pub fn reload_with_names(
        &mut self,
        fs: &GameFs,
        names: impl IntoIterator<Item = String>,
    ) -> ReloadReport {
        for name in names {
            let key = name.to_ascii_lowercase();
            if !self.by_name.contains_key(&key) {
                let id = self.materials.len();
                self.materials.push(MaterialInfo::untextured(&key));
                self.by_name.insert(key, id);
            }
        }
        self.reload(fs)
    }

    pub(crate) fn unavailable_model(&mut self) -> MaterialId {
        if let Some(id) = self.unavailable_model {
            return id;
        }
        let mut material = MaterialInfo::untextured("Unavailable model materials");
        material.missing = true;
        let id = self.materials.len();
        self.materials.push(material);
        self.unavailable_model = Some(id);
        id
    }

    pub fn clear_hdr_skybox(&mut self) {
        self.hdr_skybox = None;
    }

    pub fn cached_hdr_skybox(&self, name: &str) -> Option<Arc<crate::sky::Skybox>> {
        self.hdr_skybox
            .as_ref()
            .filter(|sky| sky.name == name)
            .cloned()
    }
    pub fn cache_hdr_skybox(&mut self, sky: crate::sky::Skybox) -> Arc<crate::sky::Skybox> {
        let sky = Arc::new(sky);
        self.hdr_skybox = Some(sky.clone());
        sky
    }

    pub fn skybox(&mut self, fs: &GameFs, name: &str) -> Arc<crate::sky::Skybox> {
        let name = name.trim().to_ascii_lowercase();
        self.skyboxes
            .entry(name.clone())
            .or_insert_with(|| Arc::new(crate::sky::Skybox::load(fs, &name)))
            .clone()
    }

    /// Flat white, tinted by vertex color: entity boxes and other helpers.
    pub const EDITOR: MaterialId = 0;
    const EDITOR_NAME: &str = "__editor/flat";

    pub(crate) fn reference_geometry(&mut self) -> MaterialId {
        const NAME: &str = "__editor/instance-reference";
        if let Some(id) = self
            .materials
            .iter()
            .position(|material| material.reference)
        {
            return id;
        }
        let mut material = MaterialInfo::untextured(NAME);
        material.reference = true;
        let id = self.materials.len();
        self.materials.push(material);
        id
    }

    pub fn load(&mut self, fs: &GameFs, name: &str) -> MaterialId {
        let key = name.to_ascii_lowercase();
        if let Some(&id) = self.by_name.get(&key) {
            return id;
        }
        self.materials.push(MaterialInfo::load(fs, &key));
        let id = self.materials.len() - 1;
        self.by_name.insert(key, id);
        id
    }

    pub fn load_limited(
        &mut self,
        fs: &GameFs,
        name: &str,
        limits: [usize; 2],
    ) -> Result<MaterialId, String> {
        self.load_validated(fs, name, limits, |material| {
            if material.missing {
                if let Some(error) = &material.error {
                    return Err(error.clone());
                }
                return Err(format!(
                    "Material {} has no usable base texture",
                    material.name
                ));
            }
            Ok(())
        })
    }

    pub(crate) fn load_validated(
        &mut self,
        fs: &GameFs,
        name: &str,
        limits: [usize; 2],
        validate: impl FnOnce(&MaterialInfo) -> Result<(), String>,
    ) -> Result<MaterialId, String> {
        let key = name.to_ascii_lowercase();
        if let Some(&id) = self.by_name.get(&key) {
            validate(&self.materials[id])?;
            return Ok(id);
        }
        let material = MaterialInfo::load_checked(fs, &key, Some(limits))?;
        validate(&material)?;
        let id = self.materials.len();
        self.materials.push(material);
        self.by_name.insert(key, id);
        Ok(id)
    }

    pub fn reload_limited(
        &mut self,
        fs: &GameFs,
        name: &str,
        limits: [usize; 2],
    ) -> Result<MaterialId, String> {
        let key = name.to_ascii_lowercase();
        let material = MaterialInfo::load_checked(fs, &key, Some(limits))?;
        if material.missing {
            return Err(format!("Material {key} has no usable base texture"));
        }
        let id = if let Some(&id) = self.by_name.get(&key) {
            self.normals.invalidate(&key);
            if let Some(detail) = self.materials[id].detail.ready() {
                self.detail_bytes -= detail.cache_bytes();
            }
            if let Some(asset) = self.materials[id].water.ready() {
                self.water_bytes -= asset.cache_bytes();
            }
            if let Some(asset) = self.materials[id].environment.ready() {
                self.environment_budget.release(asset);
            }
            let info = &self.materials[id];
            if let (Some(playback), Some(texture)) = (info.animation.ready(), info.base.as_ref()) {
                self.animation_bytes -= playback.payload_bytes(texture);
                self.animated_materials -= 1;
            }
            self.materials[id] = material;
            id
        } else {
            let id = self.materials.len();
            self.materials.push(material);
            self.by_name.insert(key, id);
            id
        };
        self.generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        Ok(id)
    }

    pub fn insert_base_texture(
        &mut self,
        name: &str,
        material: &Material,
        base: Vtf,
        display_texture: String,
    ) -> MaterialId {
        let key = name.to_ascii_lowercase();
        if let Some(&id) = self.by_name.get(&key) {
            return id;
        }
        let id = self.materials.len();
        self.materials.push(MaterialInfo::from_resolved(
            &key,
            Some(material),
            Some(base),
            None,
            Some(display_texture),
        ));
        self.by_name.insert(key, id);
        id
    }

    pub fn find(&self, name: &str) -> Option<&MaterialInfo> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .map(|id| &self.materials[*id])
    }

    pub fn get(&self, id: MaterialId) -> &MaterialInfo {
        &self.materials[id]
    }

    pub(crate) fn prepare_animation(&mut self, id: MaterialId) {
        let info = &mut self.materials[id];
        if let animation::Layer::Declared(playback) = info.animation {
            let Some(texture) = &info.base else {
                info.animation = animation::Layer::Unavailable("Base frames are missing".into());
                return;
            };
            let bytes = playback.payload_bytes(texture);
            if self.animation_bytes + bytes > animation::MAX_CACHE_BYTES
                || self.animated_materials >= animation::MAX_MATERIALS
            {
                info.animation = animation::Layer::Unavailable(
                    "Base frame cache is full (128 materials or 64 MiB)".into(),
                );
                return;
            }
            info.animation = animation::Layer::Ready(playback);
            self.animation_bytes += bytes;
            self.animated_materials += 1;
            self.generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn load_water(&mut self, fs: &GameFs, id: MaterialId) {
        use water::Layer;

        let layer = &mut self.materials[id].water;
        if matches!(layer, Layer::Declared(_))
            && let Layer::Declared(declaration) = std::mem::replace(layer, Layer::Absent)
        {
            *layer = match declaration.load(fs, water::MAX_CACHE_BYTES - self.water_bytes) {
                Ok(asset) => {
                    self.water_bytes += asset.cache_bytes();
                    Layer::Ready(Box::new(asset))
                }
                Err(error) => Layer::Unavailable(error),
            };
        }
    }

    pub(crate) fn load_detail(&mut self, fs: &GameFs, id: MaterialId) {
        let layer = &mut self.materials[id].detail;
        if !matches!(layer, detail::Layer::Declared(_)) {
            return;
        }
        if let detail::Layer::Declared(parameters) = std::mem::replace(layer, detail::Layer::Absent)
        {
            *layer = parameters.load(fs, detail::MAX_CACHE_BYTES - self.detail_bytes);
        }
        if let Some(asset) = layer.ready() {
            self.detail_bytes += asset.cache_bytes();
            self.generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn load_environment(&mut self, fs: &GameFs, id: MaterialId) {
        use environment::Layer;

        let layer = &mut self.materials[id].environment;
        if !matches!(layer, Layer::Declared(_)) {
            return;
        }
        if let Layer::Declared(declaration) = std::mem::replace(layer, Layer::Absent) {
            *layer = self.environment_budget.load(fs, declaration);
        }
        if layer.ready().is_some() {
            self.generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn normal_map(&mut self, fs: &GameFs, id: MaterialId) -> Arc<crate::normal_maps::Asset> {
        if let Some(asset) = &self.materials[id].normal {
            return asset.clone();
        }
        let asset = self.normals.load_display(fs, &self.materials[id]);
        if asset.retained {
            self.materials[id].normal = Some(asset.clone());
            if asset.image.is_some() {
                self.generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
            }
        }
        asset
    }

    pub fn len(&self) -> usize {
        self.materials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }
}
