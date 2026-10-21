use std::collections::{BTreeMap, BTreeSet};
use std::io;

use source_formats::gamefs::asset_relative;
use source_formats::vtf::{ImageFormat, Vtf};

use super::raw::{self, Parameter};
use super::{
    MAX_ASSETS, MAX_DEPTH, MAX_EDGES, MAX_MATERIAL_BYTES, MAX_MATERIAL_READS,
    MAX_MATERIAL_TOTAL_BYTES, MAX_SOURCE_BYTES, MAX_TEXTURE_AXIS, MAX_TEXTURE_BYTES,
    is_water_shader,
};
use crate::Control;

const MAX_NATIVE_NAME_BYTES: usize = 259;
const ENVMAP_FLAG: u32 = 0x4000;
const HDR_CHECK_BYTES: usize = 32 * 1024;

pub(crate) struct StaticWaterClosure {
    pub paths: BTreeSet<String>,
}

#[derive(Clone)]
struct Resolved {
    shader: String,
    parameters: BTreeMap<String, Parameter>,
}

#[derive(Clone, Copy)]
struct TextureLayout {
    version: (u32, u32),
    width: u32,
    height: u32,
    mips: u8,
    cube: bool,
    faces: u8,
    first_frame: u16,
    format: ImageFormat,
}

struct Graph<'a, F> {
    read: &'a mut F,
    control: &'a Control,
    paths: BTreeSet<String>,
    resolved: BTreeMap<String, Resolved>,
    textures: BTreeMap<String, TextureLayout>,
    resolving: BTreeSet<String>,
    walking: BTreeSet<String>,
    completed: BTreeSet<String>,
    material_reads: usize,
    material_bytes: usize,
    source_bytes: usize,
    edges: usize,
}

pub(crate) fn canonical_material_path(path: &str) -> io::Result<()> {
    canonical_path(path)?;
    if !path.starts_with("materials/") || !path.ends_with(".vmt") || path == "materials/.vmt" {
        return Err(invalid("Water material path must be materials/*.vmt"));
    }
    Ok(())
}

fn canonical_path(path: &str) -> io::Result<()> {
    if path.is_empty()
        || path.len() > MAX_NATIVE_NAME_BYTES
        || !path.is_ascii()
        || path.bytes().any(|byte| byte <= b' ' || byte == 127)
        || asset_relative(path)? != path
        || path.contains(['[', ']', '"', '{', '}'])
    {
        return Err(invalid(
            "Water assets require canonical lowercase ASCII paths of at most 259 bytes",
        ));
    }
    Ok(())
}

fn logical_path(reference: &str, material: bool, hdr: bool) -> io::Result<String> {
    canonical_path(reference)?;
    if reference.starts_with("materials/")
        || reference
            .rsplit('/')
            .next()
            .is_none_or(|name| name.contains('.'))
        || reference.starts_with('_')
        || reference.eq_ignore_ascii_case("env_cubemap")
    {
        return Err(invalid(
            "Water dependencies require logical paths without materials/ or a terminal extension; runtime env_cubemap is unsupported",
        ));
    }
    let extension = if material {
        ".vmt"
    } else if hdr {
        ".hdr.vtf"
    } else {
        ".vtf"
    };
    let path = format!("materials/{reference}{extension}");
    canonical_path(&path)?;
    Ok(path)
}

pub(crate) fn logical_material_path(reference: &str) -> io::Result<String> {
    logical_path(reference, true, false)
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub(crate) fn collect_static_water_closure<F>(
    root: &str,
    control: &Control,
    read: &mut F,
) -> io::Result<StaticWaterClosure>
where
    F: FnMut(&str, bool, usize) -> io::Result<Vec<u8>>,
{
    canonical_material_path(root)?;
    let mut graph = Graph {
        read,
        control,
        paths: BTreeSet::new(),
        resolved: BTreeMap::new(),
        textures: BTreeMap::new(),
        resolving: BTreeSet::new(),
        walking: BTreeSet::new(),
        completed: BTreeSet::new(),
        material_reads: 0,
        material_bytes: 0,
        source_bytes: 0,
        edges: 0,
    };
    graph.walk(root, 0)?;
    Ok(StaticWaterClosure { paths: graph.paths })
}

impl<F> Graph<'_, F>
where
    F: FnMut(&str, bool, usize) -> io::Result<Vec<u8>>,
{
    fn checkpoint(&self) -> io::Result<()> {
        self.control.checkpoint().map_err(io::Error::other)
    }

    fn edge(&mut self) -> io::Result<()> {
        self.checkpoint()?;
        if self.edges >= MAX_EDGES {
            return Err(invalid("Water closure exceeds 128 dependency edges"));
        }
        self.edges += 1;
        Ok(())
    }

    fn read(&mut self, path: &str, material: bool) -> io::Result<Vec<u8>> {
        self.checkpoint()?;
        if self.paths.len() >= MAX_ASSETS {
            return Err(invalid("Water closure exceeds 64 source assets"));
        }
        let mut maximum = MAX_TEXTURE_BYTES.min(MAX_SOURCE_BYTES.saturating_sub(self.source_bytes));
        if material {
            if self.material_reads >= MAX_MATERIAL_READS {
                return Err(invalid("Water closure exceeds nine material reads"));
            }
            maximum = maximum
                .min(MAX_MATERIAL_BYTES)
                .min(MAX_MATERIAL_TOTAL_BYTES.saturating_sub(self.material_bytes));
        }
        if maximum == 0 {
            return Err(invalid(
                "Water closure exceeds its material or source byte budget",
            ));
        }
        let bytes = (self.read)(path, material, maximum)?;
        if bytes.len() > maximum {
            return Err(invalid(format!(
                "Water source exceeds its byte budget: {path}"
            )));
        }
        self.source_bytes += bytes.len();
        if material {
            self.material_reads += 1;
            self.material_bytes += bytes.len();
        }
        self.paths.insert(path.into());
        Ok(bytes)
    }

    fn resolve(&mut self, path: &str, depth: usize) -> io::Result<Resolved> {
        self.checkpoint()?;
        if depth > MAX_DEPTH {
            return Err(invalid("Water material/Patch depth exceeds eight"));
        }
        if self.resolving.contains(path) {
            return Err(invalid(format!("Cyclic Water Patch include: {path}")));
        }
        if let Some(resolved) = self.resolved.get(path) {
            return Ok(resolved.clone());
        }
        self.resolving.insert(path.into());
        let bytes = self.read(path, true)?;
        let raw = raw::parse(&bytes).map_err(|failure| {
            invalid(format!(
                "Unsupported Water material {path}: {} ({})",
                failure.message, failure.code
            ))
        })?;
        let resolved = if let Some(include) = raw.include {
            canonical_material_path(&include.value)?;
            self.edge()?;
            let mut resolved = self.resolve(&include.value, depth + 1)?;
            for (replace, values) in raw.edits {
                for (key, value) in values {
                    let existing = resolved.parameters.get(&key);
                    if replace && existing.is_none()
                        || !replace && existing.is_some_and(|old| old.value != value.value)
                    {
                        return Err(invalid(format!(
                            "Conflicting Water Patch edit in {path}: {}",
                            value.key
                        )));
                    }
                    resolved.parameters.insert(key, value);
                }
            }
            resolved
        } else {
            Resolved {
                shader: raw.shader,
                parameters: raw.parameters,
            }
        };
        self.resolving.remove(path);
        self.resolved.insert(path.into(), resolved.clone());
        Ok(resolved)
    }

    fn walk(&mut self, path: &str, depth: usize) -> io::Result<()> {
        self.checkpoint()?;
        if depth > MAX_DEPTH {
            return Err(invalid("Water material dependency depth exceeds eight"));
        }
        if self.walking.contains(path) {
            return Err(invalid(format!("Cyclic Water material dependency: {path}")));
        }
        if self.completed.contains(path) {
            return Ok(());
        }
        let resolved = self.resolve(path, depth)?;
        if !is_water_shader(&resolved.shader) {
            return Err(invalid(format!(
                "Water dependency uses unsupported shader {}: {path}",
                resolved.shader
            )));
        }
        self.walking.insert(path.into());
        for (key, default) in [
            ("$normalmap", Some("dev/water_normal")),
            ("$basetexture", None),
            ("$reflecttexture", Some("_rt_WaterReflection")),
            ("$refracttexture", Some("_rt_WaterRefraction")),
            ("$envmap", Some("env_cubemap")),
            ("%tooltexture", None),
            ("$bottommaterial", None),
            ("$underwateroverlay", None),
        ] {
            let Some(reference) = resolved
                .parameters
                .get(key)
                .map(|value| value.value.as_str())
                .or(default)
            else {
                continue;
            };
            self.edge()?;
            if key == "$reflecttexture" && reference.eq_ignore_ascii_case("_rt_WaterReflection")
                || key == "$refracttexture" && reference.eq_ignore_ascii_case("_rt_WaterRefraction")
                || key == "$underwateroverlay" && reference == "0"
            {
                continue;
            }
            if key == "$envmap" && reference.eq_ignore_ascii_case("env_cubemap") {
                return Err(invalid(format!(
                    "Portable Water requires an explicit static $envmap in every reachable Water material: {path}"
                )));
            }
            let material = matches!(key, "$bottommaterial" | "$underwateroverlay");
            let target = logical_path(reference, material, false)?;
            if material {
                self.walk(&target, depth + 1)?;
            } else if key == "$envmap" {
                let ldr = self.texture(&target, true, false)?;
                let hdr_path = logical_path(reference, false, true)?;
                let hdr = self.texture(&hdr_path, true, true)?;
                if (ldr.width, ldr.height, ldr.mips) != (hdr.width, hdr.height, hdr.mips) {
                    return Err(invalid(format!(
                        "Water LDR/HDR cubemap layouts differ: {target} and {hdr_path}"
                    )));
                }
            } else {
                self.texture(&target, false, false)?;
            }
        }
        self.walking.remove(path);
        self.completed.insert(path.into());
        Ok(())
    }

    fn texture(&mut self, path: &str, cube: bool, hdr: bool) -> io::Result<TextureLayout> {
        let layout = if let Some(layout) = self.textures.get(path) {
            *layout
        } else {
            let bytes = self.read(path, false)?;
            let texture = Vtf::parse(bytes)
                .map_err(|error| invalid(format!("Unsupported Water VTF {path}: {error}")))?;
            if texture.version.1 > 4 {
                return Err(invalid(format!(
                    "Water VTF version is unsupported by CS:S; requires 7.0 through 7.4: {path}"
                )));
            }
            if texture.width > MAX_TEXTURE_AXIS
                || texture.height > MAX_TEXTURE_AXIS
                || texture.depth != 1
                || texture.authored_depth.is_some_and(|depth| depth != 1)
                || texture.authored_frames != 1
            {
                return Err(invalid(format!(
                    "Water texture is outside the static one-frame depth-one subset: {path}"
                )));
            }
            if texture
                .reflectivity
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
            {
                return Err(invalid(format!(
                    "Water texture reflectivity must be finite and nonnegative: {path}"
                )));
            }
            if texture.format == ImageFormat::Rgba16161616F {
                for mip in 0..texture.mip_count {
                    for face in 0..texture.faces {
                        let image = texture.image_data(mip, 0, face).map_err(|error| {
                            invalid(format!("Water HDR image is unavailable: {path}: {error}"))
                        })?;
                        for chunk in image.chunks(HDR_CHECK_BYTES) {
                            self.checkpoint()?;
                            for channel in chunk.as_chunks::<2>().0 {
                                let bits = u16::from_le_bytes(*channel);
                                if bits & 0x7c00 == 0x7c00
                                    || bits & 0x8000 != 0 && bits & 0x7fff != 0
                                {
                                    return Err(invalid(format!(
                                        "Water HDR channels must be finite and nonnegative: {path}"
                                    )));
                                }
                            }
                        }
                    }
                }
            }
            let layout = TextureLayout {
                version: texture.version,
                width: texture.width,
                height: texture.height,
                mips: texture.mip_count,
                cube: texture.flags & ENVMAP_FLAG != 0,
                faces: texture.faces,
                first_frame: texture.first_frame,
                format: texture.format,
            };
            self.textures.insert(path.into(), layout);
            layout
        };
        if cube {
            // CS:S reads seven stored faces for every VTF 7.4 cubemap.
            if layout.version != (7, 4)
                || !layout.cube
                || layout.width != layout.height
                || layout.faces != 7
                || layout.first_frame != 0
            {
                return Err(invalid(format!(
                    "Water cubemap requires VTF 7.4, seven stored square faces and first_frame 0: {path}"
                )));
            }
            let supported = if hdr {
                layout.format == ImageFormat::Rgba16161616F
            } else {
                matches!(
                    layout.format,
                    ImageFormat::Rgba8888
                        | ImageFormat::Bgra8888
                        | ImageFormat::Bgrx8888
                        | ImageFormat::Rgb888
                        | ImageFormat::Bgr888
                        | ImageFormat::Dxt1
                        | ImageFormat::Dxt3
                        | ImageFormat::Dxt5
                )
            };
            if !supported {
                return Err(invalid(format!(
                    "Water cubemap uses an unsupported {} color format: {path}",
                    if hdr { "HDR" } else { "LDR" }
                )));
            }
        } else if layout.cube || layout.faces != 1 {
            return Err(invalid(format!(
                "Water 2D texture has a cubemap layout: {path}"
            )));
        }
        Ok(layout)
    }
}
