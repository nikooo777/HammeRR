use std::io;

use glam::{DVec2, DVec3};
use serde::Serialize;
use source_formats::bsp::{self, Bsp, FaceGeometry, LightingKind, PakArchive, PakEntry, PakLimits};
use source_formats::vmf::parse_vec3;
use source_formats::vtf;

use crate::winding::Winding;
use crate::{CompileContext, Control, map};

const EDGE: usize = 1024;
const MARGIN: usize = 24;
const SPAWN_HEADROOM: f64 = 64.0;
const MAX_TRIANGLES: usize = 524_288;
const MAX_RASTER_WORK: usize = 128_000_000;
const MARKER: &str = "// HammeRR generated radar v1\n";
const OVERVIEWS: &str = "resource/overviews/";
const HIDDEN_SURFACES: i32 = bsp::surf::SKY
    | bsp::surf::SKY2D
    | bsp::surf::NODRAW
    | bsp::surf::TRIGGER
    | bsp::surf::HINT
    | bsp::surf::SKIP;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Off,
    Auto,
    Ceiling(i32),
}

impl Mode {
    pub const fn enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub fn setting(self) -> String {
        match self {
            Self::Off => "off".into(),
            Self::Auto => "auto".into(),
            Self::Ceiling(z) => z.to_string(),
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("off") {
            return Ok(Self::Off);
        }
        if value.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        let height = value
            .parse::<i32>()
            .map_err(|_| "Radar overview must be off, auto or a whole-number ceiling Z")?;
        Self::Ceiling(height).validate()
    }

    pub(crate) fn validate(self) -> Result<Self, String> {
        if let Self::Ceiling(z) = self
            && f64::from(z).abs() > map::MAX_COMPILE_COORD
        {
            return Err(format!(
                "Radar ceiling Z must be within ±{} units",
                map::MAX_COMPILE_COORD
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Serialize)]
pub struct Stats {
    pub file: String,
    pub size: usize,
    pub pos_x: i32,
    pub pos_y: i32,
    pub scale: f32,
    pub ceiling_z: f64,
}

struct Triangle {
    points: [DVec3; 3],
    color: [u8; 3],
}

fn checkpoint(control: &Control) -> io::Result<()> {
    control.checkpoint().map_err(io::Error::other)
}

fn automatic_ceiling(bsp: &Bsp, control: &Control) -> Result<f64, String> {
    let entities = crate::brush_model_pose::read_entities(bsp, &mut || checkpoint(control))
        .map_err(|error| error.to_string())?;
    let mut highest: Option<f64> = None;
    for entity in &entities {
        control.checkpoint()?;
        if !entity.iter().any(|(key, value)| {
            key.eq_ignore_ascii_case("classname")
                && (value.eq_ignore_ascii_case("info_player_terrorist")
                    || value.eq_ignore_ascii_case("info_player_counterterrorist"))
        }) {
            continue;
        }
        let mut origins = entity
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case("origin"));
        let origin = origins
            .next()
            .and_then(|(_, value)| parse_vec3(value))
            .filter(|&origin| map::within_compile_bounds(origin))
            .filter(|_| origins.next().is_none())
            .ok_or("Automatic radar needs one valid origin on each team spawn")?;
        highest = Some(highest.map_or(origin.z, |z| z.max(origin.z)));
    }
    let highest = highest
        .ok_or("Automatic radar needs a team spawn. Set Radar ceiling Z to use a fixed height")?;
    Ok(highest + SPAWN_HEADROOM)
}

fn surface_color(normal: DVec3, water: bool) -> [u8; 3] {
    if water {
        return [66, 137, 161];
    }
    let shade = 0.72 + normal.z * 0.28;
    [178.0, 185.0, 185.0].map(|channel| (channel * shade).round() as u8)
}

fn append_surface(
    triangles: &mut Vec<Triangle>,
    points: impl IntoIterator<Item = DVec3>,
    normal: DVec3,
    ceiling: f64,
    water: bool,
) -> Result<(), String> {
    if normal.z <= 0.0 {
        return Ok(());
    }
    let winding = Winding(points.into_iter().collect());
    let Some(Winding(polygon)) = winding.clip_back(DVec3::Z, ceiling) else {
        return Ok(());
    };
    let count = polygon.len() - 2;
    if triangles.len() + count > MAX_TRIANGLES {
        return Err(format!("Radar exceeds {MAX_TRIANGLES} projected triangles"));
    }
    let color = surface_color(normal, water);
    triangles.extend(polygon[1..].windows(2).map(|edge| Triangle {
        points: [polygon[0], edge[0], edge[1]],
        color,
    }));
    Ok(())
}

fn surfaces(bsp: &Bsp, ceiling: f64, control: &Control) -> Result<Vec<Triangle>, String> {
    let geometry =
        FaceGeometry::new_controlled(bsp, LightingKind::Ldr, &mut || checkpoint(control))
            .map_err(|error| error.to_string())?;
    let infos = bsp
        .records::<bsp::TexInfo>()
        .map_err(|error| error.to_string())?;
    let world = bsp::checked_model_face_range(&geometry.models()[0], geometry.faces().len())
        .map_err(|error| error.to_string())?;
    let mut triangles = Vec::new();
    for ordinal in world {
        control.checkpoint()?;
        let face = &geometry.faces()[ordinal];
        let info = usize::try_from(face.texinfo)
            .ok()
            .and_then(|index| infos.get(index))
            .ok_or_else(|| format!("Compiled radar face {ordinal} has no valid texture info"))?;
        if info.flags & HIDDEN_SURFACES != 0 {
            continue;
        }
        let water = info.flags & bsp::surf::WARP != 0;
        if face.dispinfo >= 0 {
            let terrain = geometry
                .terrain(ordinal)
                .map_err(|error| format!("Radar terrain face {ordinal}: {error}"))?
                .ok_or_else(|| {
                    format!("Radar terrain face {ordinal} is missing displacement geometry")
                })?;
            for indices in terrain.triangles {
                let points =
                    indices.map(|i| DVec3::from_array(terrain.positions[i].map(f64::from)));
                let normal = (points[2] - points[0])
                    .cross(points[1] - points[0])
                    .normalize_or_zero();
                append_surface(&mut triangles, points, normal, ceiling, water)?;
            }
        } else {
            let face = geometry
                .base_winding(ordinal)
                .map_err(|error| format!("Radar face {ordinal}: {error}"))?;
            let normal = DVec3::from_array(face.normal.map(f64::from));
            let points = face
                .positions
                .into_iter()
                .map(|point| DVec3::from_array(point.map(f64::from)));
            append_surface(&mut triangles, points, normal, ceiling, water)?;
        }
    }
    if triangles.is_empty() {
        return Err(format!(
            "Radar has no upward-facing world surfaces at or below Z {ceiling}"
        ));
    }
    Ok(triangles)
}

fn frame(triangles: &[Triangle], file: String, ceiling_z: f64) -> Stats {
    let mut low = DVec2::splat(f64::INFINITY);
    let mut high = DVec2::splat(f64::NEG_INFINITY);
    for point in triangles.iter().flat_map(|triangle| triangle.points) {
        let xy = point.truncate();
        low = low.min(xy);
        high = high.max(xy);
    }
    let span = (high - low).max_element() + 2.0;
    let scale = (span / (EDGE - 2 * MARGIN) as f64) as f32;
    let center = (high + low) * 0.5;
    let half = f64::from(scale) * EDGE as f64 * 0.5;
    Stats {
        file,
        size: EDGE,
        pos_x: (center.x - half).floor() as i32,
        pos_y: (center.y + half).ceil() as i32,
        scale,
        ceiling_z,
    }
}

fn samples(start: f64, end: f64) -> std::ops::Range<usize> {
    let edge = EDGE as f64;
    let index = |coordinate: f64| (coordinate - 0.5).ceil().clamp(0.0, edge) as usize;
    index(start)..index(end)
}

fn rasterize(triangles: &[Triangle], frame: &Stats, control: &Control) -> Result<Vec<u8>, String> {
    const CLAMP_S_T: u32 = 0x0c;
    const NO_LOD: u32 = 0x0200;
    const EIGHT_BIT_ALPHA: u32 = 0x2000;
    let header = vtf::legacy_rgba8888_header(
        [EDGE as u16; 2],
        CLAMP_S_T | vtf::FLAG_NOMIP | NO_LOD | EIGHT_BIT_ALPHA,
        1,
    );
    let mut bytes = header.to_vec();
    bytes.resize(header.len() + EDGE * EDGE * 4, 0);
    let pixels = &mut bytes[header.len()..];
    let mut depth = vec![f32::NEG_INFINITY; EDGE * EDGE];
    let scale = f64::from(frame.scale);
    let mut work = 0;
    for triangle in triangles {
        control.checkpoint()?;
        let p = triangle.points.map(|p| {
            DVec3::new(
                (p.x - f64::from(frame.pos_x)) / scale,
                (f64::from(frame.pos_y) - p.y) / scale,
                p.z,
            )
        });
        let low = p[0].min(p[1]).min(p[2]);
        let high = p[0].max(p[1]).max(p[2]);
        for row in samples(low.y, high.y) {
            let y = row as f64 + 0.5;
            let mut edges = [(p[0], p[1]), (p[1], p[2]), (p[2], p[0])]
                .into_iter()
                .filter_map(|(mut a, mut b)| {
                    if a.y > b.y {
                        std::mem::swap(&mut a, &mut b);
                    }
                    (a.y..b.y)
                        .contains(&y)
                        .then(|| a.lerp(b, (y - a.y) / (b.y - a.y)))
                });
            let (Some(a), Some(b)) = (edges.next(), edges.next()) else {
                continue;
            };
            let (left, right) = if a.x <= b.x { (a, b) } else { (b, a) };
            let columns = samples(left.x, right.x);
            work += 1 + columns.len();
            if work > MAX_RASTER_WORK {
                return Err(format!("Radar exceeds {MAX_RASTER_WORK} raster work units"));
            }
            if columns.is_empty() {
                continue;
            }
            let slope = (right.z - left.z) / (right.x - left.x);
            for column in columns {
                let z = (left.z + slope * (column as f64 + 0.5 - left.x)) as f32;
                let index = row * EDGE + column;
                if z > depth[index] {
                    depth[index] = z;
                    let [r, g, b] = triangle.color;
                    pixels[index * 4..index * 4 + 4].copy_from_slice(&[r, g, b, 255]);
                }
            }
        }
    }
    if !depth.iter().any(|z| z.is_finite()) {
        return Err("Radar surfaces cover no pixels at the generated scale".into());
    }
    for row in 1..EDGE - 1 {
        control.checkpoint()?;
        for column in 1..EDGE - 1 {
            let i = row * EDGE + column;
            if depth[i].is_finite()
                && [i - EDGE, i - 1, i + 1, i + EDGE]
                    .into_iter()
                    .any(|neighbor| depth[i] - depth[neighbor] > 18.0)
            {
                for channel in &mut pixels[i * 4..i * 4 + 3] {
                    *channel /= 2;
                }
            }
        }
    }
    Ok(bytes)
}

fn overview_path(namespace: &str) -> String {
    format!("{OVERVIEWS}{namespace}.txt")
}

fn require_free_path(archive: &PakArchive, path: &str) -> io::Result<()> {
    if archive
        .entries
        .iter()
        .any(|entry| entry.name.eq_ignore_ascii_case(path))
    {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("Radar path already exists in the BSP: {path}"),
        ));
    }
    Ok(())
}

fn overview_material(texture: &str, hud: bool) -> Vec<u8> {
    let vertex_color = if hud {
        "\t\"$vertexcolor\" \"1\"\n"
    } else {
        ""
    };
    format!(
        "\"UnlitGeneric\"\n{{\n\t\"$basetexture\" \"{texture}\"\n\t\"$translucent\" \"1\"\n\t\"$vertexalpha\" \"1\"\n{vertex_color}\t\"$no_fullbright\" \"1\"\n\t\"$ignorez\" \"1\"\n}}\n"
    )
    .into_bytes()
}

pub(crate) fn pack(
    bsp: &mut Bsp,
    mode: Mode,
    context: &CompileContext,
    control: &Control,
) -> Result<Stats, String> {
    let ceiling = match mode {
        Mode::Off => return Err("Cannot generate a disabled radar overview".into()),
        Mode::Auto => automatic_ceiling(bsp, control)?,
        Mode::Ceiling(z) => f64::from(z),
    };
    let name = context.namespace();
    let triangles = surfaces(bsp, ceiling, control)?;
    let stats = frame(&triangles, overview_path(name), ceiling);
    let image = rasterize(&triangles, &stats, control)?;
    let material = format!("overviews/{name}_hammerr");
    let description = format!(
        "{MARKER}\"{name}\"\n{{\n\t\"material\" \"{material}\"\n\t\"pos_x\" \"{}\"\n\t\"pos_y\" \"{}\"\n\t\"scale\" \"{}\"\n}}\n",
        stats.pos_x, stats.pos_y, stats.scale,
    );
    let limits = PakLimits::default();
    let mut check = || checkpoint(control);
    let mut archive = bsp
        .pak_data_controlled(limits, &mut check)
        .map_err(|error| error.to_string())?;
    for (name, data) in [
        (stats.file.clone(), description.into_bytes()),
        (
            format!("materials/{material}.vmt"),
            overview_material(&material, false),
        ),
        (
            format!("materials/{material}_radar.vmt"),
            overview_material(&material, true),
        ),
        (format!("materials/{material}.vtf"), image),
    ] {
        require_free_path(&archive, &name).map_err(|error| error.to_string())?;
        archive.entries.push(PakEntry { name, data });
    }
    bsp.set_pak_data_controlled(&archive, limits, &mut check)
        .map_err(|error| error.to_string())?;
    Ok(stats)
}

pub fn rebase(
    bytes: Vec<u8>,
    bsp: &Bsp,
    destination: &str,
    control: &Control,
) -> io::Result<Vec<u8>> {
    let mut check = || checkpoint(control);
    let limits = PakLimits::default();
    let mut archive = bsp.pak_data_controlled(limits, &mut check)?;
    let mut generated = archive.entries.iter().enumerate().filter(|(_, entry)| {
        entry.name.starts_with(OVERVIEWS)
            && entry.name.ends_with(".txt")
            && entry.data.starts_with(MARKER.as_bytes())
    });
    let Some((index, entry)) = generated.next() else {
        return Ok(bytes);
    };
    if generated.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "BSP has multiple HammeRR radar overviews",
        ));
    }
    let old = entry
        .name
        .trim_start_matches(OVERVIEWS)
        .trim_end_matches(".txt");
    let context = CompileContext::new(destination).map_err(io::Error::other)?;
    let new = context.namespace();
    if old == new {
        return Ok(bytes);
    }
    let path = overview_path(new);
    require_free_path(&archive, &path)?;
    let exact_root = format!("{MARKER}\"{old}\"\n{{\n");
    let body = entry
        .data
        .strip_prefix(exact_root.as_bytes())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Generated radar root does not match its filename",
            )
        })?;
    let data = [format!("{MARKER}\"{new}\"\n{{\n").as_bytes(), body].concat();
    archive.entries[index] = PakEntry { name: path, data };
    bsp::replace_bsp_pak_controlled(&bytes, &archive, limits, &mut check)
}
