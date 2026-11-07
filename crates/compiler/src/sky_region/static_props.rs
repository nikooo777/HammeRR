use std::collections::BTreeMap;
use std::io;

use glam::DVec3;
use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::model::{HardwareCharge, HardwareLimits, Model};
use source_formats::vmf::Entity;

use crate::Control;
use crate::asset_sources::{AssetSourceGuard, BuildMaterialSources, CapturedBuildAsset};

pub const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PARSE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_METADATA_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_WORK: usize = 4_000_000;
pub const MAX_VERTICES: usize = 32_768;
pub const MAX_LOGICAL_PATHS: usize = 64;
pub const MAX_PHYSICAL_PATHS: usize = 96;
pub const MAX_LOOKUP_ROWS: usize = 192;
pub const MAX_MATERIAL_BYTES: usize = 64 * 1024;
const MATERIAL_SCRATCH_BYTES: usize = MAX_MATERIAL_BYTES * 128;
pub const CPU_RESERVATION_BYTES: usize =
    MAX_SOURCE_BYTES * 2 + MAX_PARSE_BYTES + MAX_METADATA_BYTES * 2 + MATERIAL_SCRATCH_BYTES;

pub(super) struct Capture<'a> {
    game: &'a GameFs,
    control: &'a Control,
    sources: BuildMaterialSources,
    parsed: HardwareCharge,
    models: BTreeMap<String, ModelEnvelope>,
    props: usize,
    vertices: usize,
}

impl<'a> Capture<'a> {
    pub(super) fn new(game: &'a GameFs, control: &'a Control) -> Self {
        Self {
            game,
            control,
            sources: BuildMaterialSources::bounded(
                MAX_SOURCE_BYTES,
                MAX_LOGICAL_PATHS,
                MAX_PHYSICAL_PATHS,
                MAX_LOOKUP_ROWS,
            ),
            parsed: HardwareCharge::default(),
            models: BTreeMap::new(),
            props: 0,
            vertices: 0,
        }
    }

    pub(super) fn bounds(&mut self, entity: &Entity) -> Result<(DVec3, DVec3), String> {
        crate::instances::admit_static_prop(entity)?;
        let origin = entity
            .origin()
            .ok_or("Static prop requires a finite origin")?;
        let path = entity
            .properties
            .get("model")
            .ok_or("Static prop requires model")?;
        if asset_relative(path).map_err(|error| error.to_string())? != path
            || path.len() >= source_formats::bsp::STATIC_PROP_NAME_BYTES
        {
            return Err("Static prop requires a canonical models/*.mdl path".into());
        }
        let basename = path
            .strip_suffix(".mdl")
            .ok_or("Static prop requires a .mdl model")?;
        let envelope = if let Some(envelope) = self.models.get(path) {
            *envelope
        } else {
            let envelope = self.model(path, basename)?;
            self.models.insert(path.to_owned(), envelope);
            envelope
        };
        let skin = entity
            .properties
            .get("skin")
            .unwrap_or("0")
            .parse::<usize>()
            .map_err(|_| "Static prop requires a nonnegative skin")?;
        if skin >= envelope.skins {
            return Err(format!(
                "{path}: static-prop skin exceeds the original model families"
            ));
        }
        self.charge(HardwareCharge {
            bytes: 0,
            work: 3 * 3usize.pow(4),
        })
        .map_err(|error| error.to_string())?;
        let bounds = rotation_envelope(envelope.magnitude, origin)?;
        self.props += 1;
        Ok(bounds)
    }

    fn model(&mut self, path: &str, basename: &str) -> Result<ModelEnvelope, String> {
        let mdl = self.required(path)?;
        let vvd = self.required(&format!("{basename}.vvd"))?;
        let vtx = self.required(&format!("{basename}.dx90.vtx"))?;
        let model = source_formats::model::validate_fixed_static_geometry(
            path,
            mdl.bytes(),
            vvd.bytes(),
            vtx.bytes(),
            HardwareLimits {
                vertices: MAX_VERTICES,
                sections: 4_096,
                work: MAX_WORK,
                bytes: MAX_PARSE_BYTES,
            },
            &mut |charge| self.charge(charge),
        )
        .map_err(|error| format!("{path}: {error}"))?;
        self.vertices = self
            .vertices
            .checked_add(model.vertices.len())
            .filter(|&vertices| vertices <= MAX_VERTICES)
            .ok_or("Sky region exceeded its static-prop vertex budget")?;
        self.charge(HardwareCharge {
            bytes: 0,
            work: model.vertices.len() * 12,
        })
        .map_err(|error| error.to_string())?;
        self.materials(&model)?;
        Ok(ModelEnvelope {
            magnitude: magnitude(&model, self.control)?,
            skins: model.skins.len(),
        })
    }

    fn charge(&mut self, charge: HardwareCharge) -> io::Result<()> {
        self.control.checkpoint().map_err(io::Error::other)?;
        self.parsed.bytes = self
            .parsed
            .bytes
            .checked_add(charge.bytes)
            .filter(|&bytes| bytes <= MAX_PARSE_BYTES)
            .ok_or_else(|| {
                io::Error::other("Sky static-prop aggregate parse byte budget exceeded")
            })?;
        self.parsed.work = self
            .parsed
            .work
            .checked_add(charge.work)
            .filter(|&work| work <= MAX_WORK)
            .ok_or_else(|| {
                io::Error::other("Sky static-prop aggregate parse work budget exceeded")
            })?;
        Ok(())
    }

    fn read(&mut self, path: &str, maximum: usize) -> io::Result<Option<CapturedBuildAsset>> {
        self.charge(HardwareCharge { bytes: 0, work: 1 })?;
        let asset = self.sources.read_asset(self.game, path, maximum)?;
        if let Some(asset) = &asset {
            self.charge(HardwareCharge {
                bytes: 0,
                work: asset.bytes().len().div_ceil(64),
            })?;
        }
        Ok(asset)
    }

    fn required(&mut self, path: &str) -> Result<CapturedBuildAsset, String> {
        self.read(path, MAX_SOURCE_BYTES)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Static-prop original asset is missing: {path}"))
    }

    fn materials(&mut self, model: &Model) -> Result<(), String> {
        for name in &model.materials {
            self.control.checkpoint()?;
            let mut found = false;
            for candidate in
                source_formats::model::material_candidates(name, &model.material_directories)
            {
                let path = format!("materials/{candidate}.vmt");
                let material = crate::lighting::load_prop_vertex_material(&path, &mut |path| {
                    self.read(path, MAX_MATERIAL_BYTES)
                        .map(|asset| asset.map(|asset| asset.bytes().to_vec()))
                })?;
                if material.is_some() {
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(format!("Static-prop fixed material is missing: {name}"));
            }
        }
        Ok(())
    }

    pub(super) fn finish(
        self,
        stats: &mut super::Stats,
    ) -> Result<Option<AssetSourceGuard>, String> {
        let guard = if self.props == 0 {
            None
        } else {
            let guard = self
                .sources
                .source_guard(self.game, self.control)
                .map_err(|error| error.to_string())?;
            guard
                .verify_original_metadata(self.control, MAX_METADATA_BYTES)
                .map_err(|error| error.to_string())?;
            Some(guard)
        };
        stats.excluded_static_props = self.props;
        stats.static_prop_models = self.models.len();
        stats.static_prop_vertices = self.vertices;
        Ok(guard)
    }
}

#[derive(Clone, Copy)]
struct ModelEnvelope {
    magnitude: f32,
    skins: usize,
}

fn magnitude(model: &Model, control: &Control) -> Result<f32, String> {
    if model.vertices.is_empty() {
        return Err("Static-prop geometry has no validated vertices".into());
    }
    let mut magnitude = f32::MIN_POSITIVE;
    for (index, vertex) in model.vertices.iter().enumerate() {
        if index.is_multiple_of(1_024) {
            control.checkpoint()?;
        }
        for value in vertex.position {
            while magnitude < value.abs() {
                magnitude *= 2.0;
            }
        }
    }
    Ok(magnitude)
}

fn rotation_envelope(magnitude: f32, origin: DVec3) -> Result<(DVec3, DVec3), String> {
    // Native AngleMatrix coefficients are products or sums of two products of
    // sincosf outputs in [-1, 1]. A power-of-two magnitude bounds each rounded
    // transform product by 2M; every sum association includes directed rounding.
    let term = Interval {
        low: -2.0 * magnitude,
        high: 2.0 * magnitude,
    };
    let radius = 6.0 * f64::from(magnitude);
    let native_origin = origin.as_vec3();
    let mut mins = DVec3::ZERO;
    let mut maxs = DVec3::ZERO;
    for axis in 0..3 {
        let native = sum_envelope(term, native_origin[axis])?;
        mins[axis] = (origin[axis] - radius)
            .next_down()
            .min(f64::from(native.low));
        maxs[axis] = (origin[axis] + radius)
            .next_up()
            .max(f64::from(native.high));
    }
    if !mins.is_finite() || !maxs.is_finite() {
        return Err("Static-prop geometry exceeded finite world bounds".into());
    }
    Ok((mins, maxs))
}

#[derive(Clone, Copy)]
struct Interval {
    low: f32,
    high: f32,
}

impl Interval {
    fn union(self, other: Self) -> Self {
        Self {
            low: self.low.min(other.low),
            high: self.high.max(other.high),
        }
    }

    fn add(self, other: Self) -> Self {
        let low = f64::from(self.low) + f64::from(other.low);
        let high = f64::from(self.high) + f64::from(other.high);
        let mut interval = Self {
            low: (low as f32).next_down(),
            high: (high as f32).next_up(),
        };
        if low < f64::from(f32::MIN_POSITIVE) && high > -f64::from(f32::MIN_POSITIVE) {
            interval = interval.union(Self {
                low: 0.0,
                high: 0.0,
            });
        }
        interval
    }
}

fn sum_envelope(term: Interval, origin: f32) -> Result<Interval, String> {
    let origin = if origin.is_subnormal() {
        Interval {
            low: origin.min(0.0),
            high: origin.max(0.0),
        }
    } else {
        Interval {
            low: origin,
            high: origin,
        }
    };
    let mut sums = [None; 16];
    sums[1] = Some(term);
    sums[2] = Some(term);
    sums[4] = Some(term);
    sums[8] = Some(origin);
    for mask in 1usize..16 {
        if mask.is_power_of_two() {
            continue;
        }
        let mut left = (mask - 1) & mask;
        while left != 0 {
            let right = mask ^ left;
            if left < right
                && let (Some(a), Some(b)) = (sums[left], sums[right])
            {
                let interval = a.add(b);
                sums[mask] = Some(sums[mask].map_or(interval, |current| current.union(interval)));
            }
            left = (left - 1) & mask;
        }
    }
    sums[15].ok_or_else(|| "Static-prop transform interval is incomplete".into())
}
