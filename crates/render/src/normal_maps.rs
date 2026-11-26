use std::collections::BTreeMap;
use std::sync::Arc;

use glam::DVec3;
use serde::Serialize;
use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::vmf::Side;
use source_formats::vmt::{DuplicateOperations, Material, texture_path, texture_transform};
use source_formats::vtf::{FLAG_SSBUMP, ImageFormat, RgbaImage, Vtf};

use crate::materials::uv::IDENTITY_AXIS as IDENTITY_TRANSFORM;

pub const MAX_ASSETS: usize = 128;
pub const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_AXIS: u32 = 1024;
pub const MAX_TERRAIN_VERTICES: usize = 1024 * 1024;
const MAX_MATERIAL_BYTES: usize = 64 * 1024;
const MAX_TEXTURE_BYTES: usize = 16 * 1024 * 1024;
const UNSUPPORTED_TEXTURE_FLAGS: u32 = 0x2201_c84d;

pub(crate) fn ordinary_ldr_format(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Rgb888
            | ImageFormat::Bgr888
            | ImageFormat::Rgba8888
            | ImageFormat::Bgra8888
            | ImageFormat::Dxt1
            | ImageFormat::Dxt3
            | ImageFormat::Dxt5
    )
}

pub(crate) fn validate_static_texture(texture: &Vtf, maximum_axis: u32) -> Result<(), String> {
    if texture.width > maximum_axis
        || texture.height > maximum_axis
        || texture.frames != 1
        || texture.faces != 1
        || texture.depth != 1
        || texture.authored_frames == 0
        || texture.authored_depth == Some(0)
    {
        return Err(format!(
            "Static texture requires one frame, face and depth slice with axes 1–{maximum_axis}"
        ));
    }
    Ok(())
}
pub const LIMITS: &str = "Flat LightmappedGeneric brush polygons and world/func_detail terrain. Terrain also supports WorldVertexTransition with two exact static ordinary repeat-sampled LDR color textures and checked normal inputs. Selected-body and selected-skin authored MDL meshes support opaque Normal mode and plain VertexLitGeneric with one ordinary RGB bump map, identity transforms and checked stored tangents on the displayed pose. Sprites, icons, helpers and reference meshes are excluded. Terrain uses checked packed triangle UV/position tangent frames and prepared border normals; singular frames are excluded. Normal-mapped terrain expands triangle vertices within its explicit shared vertex budget. Static single-frame/face/depth RGB/BGR/RGBA/BGRA8888 or DXT1/3/5 tangent-space normals, axes 1–1024. RGB is linear. Normal images repeat with linear filtering at their base mip. Bump alpha masks specular on supported Phong models in Detail authored lighting. Clamp/border/point-sample, sRGB, procedural and render-target VTF flags are unsupported. Brush and terrain normal Z must be at least 128. Model preview uses the stored normal and tangent with the installed shader’s signed bitangent and zero-vector normalization. Plain opaque single-base LightmappedGeneric also supports flagged SSBump RGB coefficients with literal $ssbump 1, or finite biased linear RGBA16F XYZ with Z above 0.5. SSBump uses fixed shading on terrain and fixed or Detail authored lighting on flat brushes. Signed/swizzled formats are unsupported. Ordinary LightmappedGeneric uses one bump map. WorldVertexTransition uses the first normal for both colors when only one map is declared. Vertex paint blends both normals when a second map is declared. Missing or invalid declared layers reject normals for the terrain receiver or the selected model instance. Proxies, fallback blocks, detail, animated frames and base display overrides are excluded. WorldVertexTransition uses identity texture transforms. LightmappedGeneric uses an identity base transform and supports positive axis scale and translation with zero rotation for bump maps. Authored face UV rotation and signed scales define the tangent frame. Fixed editor shading or admitted unshadowed authored lights in Detail quality. Plain opaque VertexLitGeneric Phong uses a normalized signed bitangent per vertex, a fixed declared exponent, explicit boost and Fresnel ranges. See Authored lighting preview for Phong limits. 128 cached admissions and 64 MiB decoded bytes; reload materials clears successes/failures. Disabled and non-Textured views perform no normal admission.";

#[derive(Serialize)]
pub struct Limits {
    pub assets: usize,
    pub decoded_bytes: usize,
    pub texture_bytes: usize,
    pub material_bytes: usize,
    pub axis: u32,
    pub terrain_vertices: usize,
    pub policy: &'static str,
}

pub fn limits() -> Limits {
    Limits {
        assets: MAX_ASSETS,
        decoded_bytes: MAX_CACHE_BYTES,
        texture_bytes: MAX_TEXTURE_BYTES,
        material_bytes: MAX_MATERIAL_BYTES,
        axis: MAX_AXIS,
        terrain_vertices: MAX_TERRAIN_VERTICES,
        policy: LIMITS,
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AssetReport {
    pub material: String,
    pub state: &'static str,
    pub texture: Option<String>,
    pub source: Option<crate::icons::Source>,
    pub texture_source: Option<crate::icons::Source>,
    pub dimensions: Option<[u32; 2]>,
    pub encoding: Option<Encoding>,
    pub transform: [f32; 4],
    pub secondary_texture: Option<String>,
    pub secondary_texture_source: Option<crate::icons::Source>,
    pub secondary_dimensions: Option<[u32; 2]>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    TangentRgb,
    TangentRgba16f,
    SsBump,
}

#[derive(Debug)]
pub enum Image {
    Byte(RgbaImage),
    Half {
        dimensions: [u32; 2],
        pixels: Vec<u8>,
    },
}

impl Image {
    pub(crate) fn dimensions(&self) -> [u32; 2] {
        match self {
            Self::Byte(image) => [image.width, image.height],
            Self::Half { dimensions, .. } => *dimensions,
        }
    }

    pub(crate) fn pixels(&self) -> &[u8] {
        match self {
            Self::Byte(image) => &image.pixels,
            Self::Half { pixels, .. } => pixels,
        }
    }
}

#[derive(Debug)]
pub struct Asset {
    pub retained: bool,
    pub report: AssetReport,
    pub image: Option<Image>,
    pub secondary_image: Option<Image>,
}

impl Asset {
    fn decoded_bytes(&self) -> usize {
        self.image
            .iter()
            .chain(&self.secondary_image)
            .map(|image| image.pixels().len())
            .sum()
    }
}

#[derive(Debug, Default)]
pub struct Library {
    entries: BTreeMap<String, Arc<Asset>>,
    bytes: usize,
}

impl Library {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub(crate) fn invalidate(&mut self, name: &str) {
        if let Some(asset) = self.entries.remove(name) {
            self.bytes -= asset.decoded_bytes();
        }
    }

    pub fn load(&mut self, fs: &GameFs, name: &str) -> Arc<Asset> {
        self.load_matching(fs, name, None)
    }

    pub fn load_matching(&mut self, fs: &GameFs, name: &str, expected: Option<u64>) -> Arc<Asset> {
        self.load_cached(fs, name, expected, None)
    }

    pub(crate) fn load_display(
        &mut self,
        fs: &GameFs,
        display: &crate::materials::MaterialInfo,
    ) -> Arc<Asset> {
        self.load_cached(fs, &display.name, display.source_identity(), Some(display))
    }

    fn load_cached(
        &mut self,
        fs: &GameFs,
        name: &str,
        expected: Option<u64>,
        display: Option<&crate::materials::MaterialInfo>,
    ) -> Arc<Asset> {
        if let Some(asset) = self.entries.get(name) {
            return asset.clone();
        }
        let mut report = AssetReport {
            material: name.chars().take(256).collect(),
            state: "unsupported",
            texture: None,
            source: None,
            texture_source: None,
            dimensions: None,
            encoding: None,
            transform: IDENTITY_TRANSFORM,
            secondary_texture: None,
            secondary_texture_source: None,
            secondary_dimensions: None,
            error: None,
        };
        let retained = name.len() <= 256 && self.entries.len() < MAX_ASSETS;
        let (image, secondary_image) = if retained {
            match load(
                fs,
                name,
                MAX_CACHE_BYTES.saturating_sub(self.bytes),
                expected,
                display,
                &mut report,
            ) {
                Ok(Some(images)) => {
                    report.state = "ready";
                    (Some(images.primary), images.secondary)
                }
                Ok(None) => {
                    report.state = "absent";
                    (None, None)
                }
                Err(error) => {
                    report.error = Some(error.chars().take(512).collect());
                    (None, None)
                }
            }
        } else {
            report.error = Some(
                if name.len() > 256 {
                    "Normal-map material path exceeds 256 bytes"
                } else {
                    "Normal-map admission cache reached 128 entries; reload material assets to retry"
                }
                .into(),
            );
            (None, None)
        };
        let asset = Arc::new(Asset {
            retained,
            report,
            image,
            secondary_image,
        });
        if retained {
            self.bytes += asset.decoded_bytes();
            self.entries.insert(name.into(), asset.clone());
        }
        asset
    }
}

fn transform(value: Option<&str>) -> Result<[f32; 4], String> {
    let transform = texture_transform::<f32>(value, DuplicateOperations::Reject)
        .map_err(|_| "Normal preview requires a finite bounded axis-scale/translation transform with zero rotation and no duplicate operations".to_owned())?;
    if transform[..2].iter().any(|value| *value < 1.0 / 1024.0) {
        return Err(
            "Normal preview requires positive bump transform scales from 1/1024 to 1024".into(),
        );
    }
    Ok(transform)
}

pub(crate) fn declares_normals(material: &Material) -> bool {
    material.params.iter().any(|(key, _)| {
        key.starts_with("$bump") || key.starts_with("$normalmap") || key.starts_with("$ssbump")
    })
}

struct Images {
    primary: Image,
    secondary: Option<Image>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImageProfile {
    Brush,
    SsBump,
    TerrainBlend,
    Model,
}

fn load(
    fs: &GameFs,
    name: &str,
    budget: usize,
    expected: Option<u64>,
    display: Option<&crate::materials::MaterialInfo>,
    report: &mut AssetReport,
) -> Result<Option<Images>, String> {
    if name.len() > 256 {
        return Err("Normal-map material path exceeds 256 bytes".into());
    }
    let name = asset_relative(name).map_err(|error| error.to_string())?;
    let material =
        Material::load_with_reader(&source_formats::vmt::material_path(&name), &mut |path| {
            let asset = fs.read_asset_limited(path, MAX_MATERIAL_BYTES)?;
            if report.source.is_none() {
                report.source = asset.as_ref().map(|asset| asset.origin.clone().into());
            }
            Ok(asset.map(|asset| asset.data))
        })
        .map_err(|error| error.to_string())?
        .ok_or("Normal-preview material was not found")?;
    if expected.is_some_and(|expected| expected != crate::materials::material_identity(&material)) {
        return Err("Material settings changed since its base display was cached; Reload material assets before normal preview".into());
    }
    if !declares_normals(&material) && display.is_none_or(|display| !display.model_bumped) {
        return Ok(None);
    }
    let terrain_only = material.shader == "worldvertextransition";
    let model_only = material.shader == "vertexlitgeneric";
    let ssbump = match material.get("$ssbump").map(str::trim) {
        None | Some("0") => false,
        Some("1") if material.shader == "lightmappedgeneric" => true,
        Some("1") => return Err("SSBump preview requires single-base LightmappedGeneric".into()),
        _ => return Err("SSBump preview requires literal $ssbump 0 or 1".into()),
    };
    if terrain_only || model_only {
        let display = display.ok_or("Normal preview requires the exact base display textures")?;
        if let Some(error) = &display.error {
            return Err(error.clone());
        }
        if !display.matches_material(&material) {
            return Err("Normal preview requires the exact base display textures".into());
        }
        let error = if terrain_only {
            &display.terrain_lighting_error
        } else {
            &display.model_normal_error
        };
        if let Some(error) = error {
            return Err(error.clone());
        }
    } else {
        single_layer_material(&material)?;
    }
    let bump = material
        .get("$bumpmap")
        .filter(|value| !value.trim().is_empty())
        .ok_or("Normal preview requires one nonempty $bumpmap")?;
    if material
        .get("$basetexture")
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err("Normal preview requires an explicit base texture".into());
    }
    if transform(material.get("$basetexturetransform"))? != IDENTITY_TRANSFORM {
        return Err("Normal preview requires an identity base texture transform".into());
    }
    report.transform = transform(material.get("$bumptransform"))?;
    if model_only && report.transform != IDENTITY_TRANSFORM {
        return Err("VertexLitGeneric normal preview requires identity $bumptransform".into());
    }
    if terrain_only {
        if report.transform != IDENTITY_TRANSFORM {
            return Err(
                "WorldVertexTransition normal preview requires identity $bumptransform".into(),
            );
        }
        let second_transform = transform(material.get("$bumptransform2"))
            .map_err(|error| format!("WorldVertexTransition $bumptransform2: {error}"))?;
        if second_transform != IDENTITY_TRANSFORM {
            return Err(
                "WorldVertexTransition normal preview requires identity $bumptransform2".into(),
            );
        }
    }
    let path = normal_path(bump, terrain_only || model_only)?;
    report.texture = Some(path.clone());
    let profile = if terrain_only {
        ImageProfile::TerrainBlend
    } else if model_only {
        ImageProfile::Model
    } else if ssbump {
        ImageProfile::SsBump
    } else {
        ImageProfile::Brush
    };
    let (image, encoding) = read_normal(fs, &path, budget, profile, &mut report.texture_source)?;
    if encoding != Encoding::TangentRgb {
        special_normal_material(&material, display)?;
    }
    report.dimensions = Some(image.dimensions());
    report.encoding = Some(encoding);
    let secondary = if terrain_only && let Some(bump) = material.get("$bumpmap2") {
        let path =
            normal_path(bump, true).map_err(|error| format!("Second normal layer: {error}"))?;
        report.secondary_texture = Some(path.clone());
        let (image, _) = read_normal(
            fs,
            &path,
            budget - image.pixels().len(),
            ImageProfile::TerrainBlend,
            &mut report.secondary_texture_source,
        )
        .map_err(|error| format!("Second normal layer {path}: {error}"))?;
        report.secondary_dimensions = Some(image.dimensions());
        Some(image)
    } else {
        None
    };
    Ok(Some(Images {
        primary: image,
        secondary,
    }))
}

fn single_layer_material(material: &Material) -> Result<(), String> {
    if material.shader != "lightmappedgeneric" {
        return Err(format!(
            "Normal preview does not support shader {}",
            material.shader
        ));
    }
    if !material.blocks.is_empty() {
        return Err(
            "Normal preview does not support material proxies or nested fallback/effect blocks"
                .into(),
        );
    }
    for (key, _) in &material.params {
        if (key.starts_with("$bump") && !["$bumpmap", "$bumptransform"].contains(&key.as_str()))
            || (key.starts_with("$ssbump") && key != "$ssbump")
            || key.starts_with("$detail")
            || [
                "$normalmap",
                "$normalmap2",
                "$basetexture2",
                "%tooltexture",
                "$frame",
                "$forcebump",
            ]
            .contains(&key.as_str())
        {
            return Err(format!(
                "Normal preview does not support material parameter {key}"
            ));
        }
    }
    Ok(())
}

fn special_normal_material(
    material: &Material,
    display: Option<&crate::materials::MaterialInfo>,
) -> Result<(), String> {
    if material.is_tool() || material.translucent() || material.alpha_test() {
        return Err("Special normal encodings require an opaque non-tool surface".into());
    }
    if let Some((key, _)) = material.params.iter().find(|(key, _)| {
        !matches!(
            key.as_str(),
            "$basetexture"
                | "$basetexturetransform"
                | "$bumpmap"
                | "$bumptransform"
                | "$ssbump"
                | "$surfaceprop"
                | "$surfaceprop2"
                | "$nodecal"
                | "%keywords"
        )
    }) {
        let key: String = key.chars().take(128).collect();
        return Err(format!(
            "Special normal encodings do not support material parameter {key}"
        ));
    }
    let display = display.ok_or("Special normal encodings need the exact base VTF")?;
    if let Some(error) = &display.error {
        return Err(error.clone());
    }
    let path = material
        .get("$basetexture")
        .map(str::trim)
        .filter(|path| !path.is_empty() && asset_relative(path).is_ok())
        .map(texture_path);
    let base = display
        .base
        .as_ref()
        .filter(|_| path.is_some() && path == display.display_texture)
        .ok_or("Special normal encodings need the exact base VTF")?;
    crate::materials::validate_ordinary_color_texture(base, u32::from(u16::MAX))
        .map_err(|error| format!("Special normal encoding base: {error}"))
}

fn normal_path(value: &str, require_relative: bool) -> Result<String, String> {
    if require_relative {
        asset_relative(value.trim()).map_err(|error| error.to_string())?;
    }
    let path = asset_relative(&texture_path(value)).map_err(|error| error.to_string())?;
    if path.len() > 256 || value.trim().is_empty() {
        return Err("Normal texture requires a bounded nonempty mounted VTF path".into());
    }
    Ok(path)
}

fn read_normal(
    fs: &GameFs,
    path: &str,
    budget: usize,
    profile: ImageProfile,
    source: &mut Option<crate::icons::Source>,
) -> Result<(Image, Encoding), String> {
    let asset = fs
        .read_asset_limited(path, MAX_TEXTURE_BYTES)
        .map_err(|error| error.to_string())?
        .ok_or("Normal texture was not found")?;
    *source = Some(asset.origin.into());
    let texture = Vtf::parse(asset.data).map_err(|error| error.to_string())?;
    validate_static_texture(&texture, MAX_AXIS).map_err(|_| {
        "Normal preview requires a static single-frame/face/depth VTF with axes 1–1024".to_owned()
    })?;
    let ssbump = profile == ImageProfile::SsBump;
    if (texture.flags & FLAG_SSBUMP != 0) != ssbump {
        return Err(
            "SSBump requires both literal $ssbump 1 and a coefficient VTF with its SSBump flag"
                .into(),
        );
    }
    let half = texture.format == ImageFormat::Rgba16161616F && profile == ImageProfile::Brush;
    if !half && !ordinary_ldr_format(texture.format) {
        return Err(
            "Normal preview needs an ordinary RGB texture or a supported single-base RGBA16F input"
                .into(),
        );
    }
    if (half || ssbump) && texture.first_frame != 0 {
        return Err("Special normal encodings require the static frame-zero texture".into());
    }
    if texture.flags & UNSUPPORTED_TEXTURE_FLAGS != 0 {
        return Err("Normal preview does not support clamp/border/point-sample, sRGB, procedural, environment or render-target VTF flags".into());
    }
    let bytes = texture.width as usize * texture.height as usize * if half { 8 } else { 4 };
    if bytes > budget {
        return Err("Normal-map decoded cache exceeds 64 MiB; reload materials to retry".into());
    }
    if half {
        let pixels = texture
            .image_data(0, 0, 0)
            .map_err(|error| error.to_string())?;
        for pixel in pixels.as_chunks::<8>().0 {
            let values = [0, 2, 4, 6].map(|offset| {
                half::f16::from_bits(u16::from_le_bytes([pixel[offset], pixel[offset + 1]]))
                    .to_f32()
            });
            if !values.iter().all(|value| (0.0..=1.0).contains(value)) || values[2] <= 0.5 {
                return Err(
                    "RGBA16F normals need finite 0–1 components and biased tangent Z above 0.5"
                        .into(),
                );
            }
        }
        return Ok((
            Image::Half {
                dimensions: [texture.width, texture.height],
                pixels: pixels.to_vec(),
            },
            Encoding::TangentRgba16f,
        ));
    }
    let image = texture
        .decode_rgba8(0, 0, 0)
        .map_err(|error| error.to_string())?;
    if profile != ImageProfile::Model
        && !ssbump
        && image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[2] < 128)
    {
        return Err("Normal preview requires RGB normal Z≥128; lower-hemisphere or ambiguous normal encodings are unsupported".into());
    }
    Ok((
        Image::Byte(image),
        if ssbump {
            Encoding::SsBump
        } else {
            Encoding::TangentRgb
        },
    ))
}

pub fn tangent(
    side: &Side,
    normal: DVec3,
    points: &[DVec3],
    dimensions: [u32; 2],
    transform: [f32; 4],
) -> Result<[f32; 4], String> {
    let s = side.u_axis.axis / side.u_axis.scale;
    let t = side.v_axis.axis / side.v_axis.scale;
    let orientation = normal.dot(s.cross(t));
    if !s.is_finite() || !t.is_finite() || !orientation.is_finite() || orientation.abs() < 1e-8 {
        return Err(
            "Normal preview requires a finite nonsingular authored face UV projection".into(),
        );
    }
    validate_uvs(
        points.iter().map(|&point| {
            map_core::texel_uv(side, point) / glam::DVec2::from_array(dimensions.map(f64::from))
        }),
        transform,
    )?;
    let (tangent, _, _) = map_core::surface::texture_tangents(normal, s, orientation < 0.0)?;
    Ok([
        tangent.x as f32,
        tangent.y as f32,
        tangent.z as f32,
        if orientation < 0.0 { -1.0 } else { 1.0 },
    ])
}

pub(crate) fn validate_uvs(
    coordinates: impl IntoIterator<Item = glam::DVec2>,
    transform: [f32; 4],
) -> Result<(), String> {
    for uv in coordinates {
        let bumped = uv * glam::DVec2::new(f64::from(transform[0]), f64::from(transform[1]))
            + glam::DVec2::new(f64::from(transform[2]), f64::from(transform[3]));
        if !uv.is_finite()
            || !bumped.is_finite()
            || uv.abs().max_element().max(bumped.abs().max_element()) > 1_000_000.0
        {
            return Err("Normal-preview UV coordinates must fit finite ±1000000".into());
        }
    }
    Ok(())
}

pub(crate) fn terrain_frames(
    mesh: &map_core::DisplacementMesh,
    normals: &[[f64; 3]],
    coordinates: &[[f32; 2]],
    side: &Side,
    dimensions: [u32; 2],
    transform: [f32; 4],
) -> Result<Vec<[f32; 4]>, String> {
    if let Some(error) = crate::scene::terrain::receiver_error(mesh, normals) {
        return Err(error.into());
    }
    if coordinates.len() != mesh.positions.len() {
        return Err("Terrain normal preview needs complete texture coordinates".into());
    }
    let base_normal = map_core::Plane::from_points(side.plane.0)
        .ok_or("Terrain normal preview needs a valid base plane")?
        .normal;
    tangent(
        side,
        base_normal,
        &mesh.base_positions,
        dimensions,
        transform,
    )?;
    validate_uvs(
        coordinates
            .iter()
            .map(|uv| glam::DVec2::from_array(uv.map(f64::from))),
        transform,
    )?;
    let mut frames = Vec::new();
    for triangle in mesh.triangles() {
        let [a, b, c] = triangle.map(|index| mesh.positions[index].as_vec3().as_dvec3());
        let [ua, ub, uc] =
            triangle.map(|index| glam::DVec2::from_array(coordinates[index].map(f64::from)));
        let du = ub - ua;
        let dv = uc - ua;
        let determinant = du.perp_dot(dv);
        if !determinant.is_finite() || determinant == 0.0 {
            return Err("Terrain normal preview requires nonsingular packed triangle UVs".into());
        }
        let tangent = ((b - a) * dv.y - (c - a) * du.y) / determinant;
        let normal = DVec3::from_array(normals[triangle[0]]);
        let orientation = normal.dot((b - a).cross(c - a)) / determinant;
        if !orientation.is_finite() || orientation == 0.0 {
            return Err("Terrain normal preview needs finite nonzero tangent frames".into());
        }
        let sign = if orientation < 0.0 { -1.0 } else { 1.0 };
        for index in triangle {
            map_core::surface::texture_tangents(
                DVec3::from_array(normals[index]),
                tangent,
                sign < 0.0,
            )?;
        }
        let direction = tangent
            .try_normalize()
            .ok_or("Terrain normal preview needs a finite tangent direction")?
            .as_vec3();
        frames.push([direction.x, direction.y, direction.z, sign]);
    }
    if frames.is_empty() {
        return Err("Terrain normal preview needs drawable triangles".into());
    }
    Ok(frames)
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub requested: bool,
    pub active: bool,
    pub applied_faces: usize,
    pub rejected_faces: usize,
    pub terrain: crate::lighting::TerrainReceivers,
    pub terrain_vertices: usize,
    pub models: crate::lighting::ModelReceivers,
    pub materials: Vec<AssetReport>,
    pub errors: Vec<String>,
    pub errors_truncated: bool,
    pub materials_truncated: bool,
}

impl Report {
    pub(crate) fn material(&mut self, report: &AssetReport) {
        if self
            .materials
            .iter()
            .any(|known| known.material == report.material)
        {
            return;
        }
        if self.materials.len() < MAX_ASSETS {
            self.materials.push(report.clone());
        } else {
            self.materials_truncated = true;
        }
    }

    pub(crate) fn reject_terrain(
        &mut self,
        face: map_core::surface::FaceId,
        material: &str,
        triangles: usize,
        reason: &str,
    ) {
        self.terrain.excluded_surfaces += 1;
        self.terrain.excluded_triangles += triangles;
        self.error(format!(
            "Terrain side {} on brush {} · {}: {reason}",
            face.side, face.solid, material
        ));
    }

    pub(crate) fn error(&mut self, error: String) {
        if self.errors.len() < MAX_ASSETS {
            self.errors.push(error.chars().take(1024).collect());
        } else {
            self.errors_truncated = true;
        }
    }

    pub fn summary(&self) -> String {
        if !self.requested {
            return "Normal map preview is off for this view".into();
        }
        if !self.active {
            return "Normal map preview requires Textured view; no normal assets were read".into();
        }
        let mut summary = format!(
            "Normal maps · {} brush polygons · {} terrain surfaces · {} model instances · {} unavailable",
            self.applied_faces,
            self.terrain.admitted_surfaces,
            self.models.admitted_instances,
            self.rejected_faces + self.terrain.excluded_surfaces + self.models.excluded_instances
        );
        for error in self.errors.iter().take(3) {
            summary.push_str(&format!("\n{error}"));
        }
        summary
    }
}
