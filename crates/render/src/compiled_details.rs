use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use glam::{DQuat, DVec3, EulerRot, Vec3};
use source_formats::{
    bsp::{DetailProp, DetailPropLighting, DetailProps, DetailSprite, RgbExp32},
    gamefs::GameFs,
};

use crate::{
    MaterialLibrary, SceneMesh,
    models::Instance,
    scene::{Batch, LineVertex, Vertex},
};

pub const MAX_VERTICES: usize = 262_144;
pub const MAX_INDICES: usize = 786_432;
pub const MAX_BYTES: usize = 32 * 1024 * 1024;
const FULL_BILLBOARD_TAG: f32 = -2.5;
const VERTICAL_BILLBOARD_TAG: f32 = -3.5;
pub const BAKED_LIGHTING_RECEIVER: u32 = 2;
pub const BAKED_RADIANCE_RECEIVER: u32 = 3;
pub const LIGHTING_RECEIVER_KIND_BITS: u32 = 4;
pub const LIGHTING_RECEIVER_KIND_MASK: u32 = (1 << LIGHTING_RECEIVER_KIND_BITS) - 1;
pub const MAX_HDR_TABLE_BYTES: usize =
    source_formats::bsp::MAX_DETAIL_PROPS * size_of::<[f32; 4]>();
pub const MAX_HDR_DRAW_METADATA_BYTES: usize =
    source_formats::bsp::MAX_DETAIL_PROPS * size_of::<u32>();
pub const MAX_HDR_TABLE_CPU_BYTES: usize = MAX_HDR_TABLE_BYTES
    + source_formats::bsp::MAX_DETAIL_PROPS * size_of::<HdrRadianceRecord>()
    + size_of::<HdrRadiance>()
    + 2 * size_of::<usize>();
pub const MAX_HDR_RETAINED_CPU_BYTES: usize = 4 * MAX_HDR_TABLE_CPU_BYTES;
const HDR_SCENE_METADATA_BYTES: usize = size_of::<(
    Option<Arc<HdrRadiance>>,
    Vec<u32>,
    Option<HdrRadianceTableIssue>,
)>();
static HDR_RETAINED_CPU_BYTES: AtomicUsize = AtomicUsize::new(0);
pub const MIN_EXPOSURE_EV: f64 = -8.0;
pub const MAX_EXPOSURE_EV: f64 = 8.0;
pub const MAX_STYLE_GAIN: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrRadianceIssue {
    Negative,
    NonFinite,
    Unrepresentable,
    SourceUnavailable,
}

impl HdrRadianceIssue {
    fn message(self) -> &'static str {
        match self {
            Self::Negative => "raw radiance is negative",
            Self::NonFinite => "raw radiance is not finite",
            Self::Unrepresentable => "raw radiance is not representable as finite nonzero f32",
            Self::SourceUnavailable => "raw source/style composition is unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrRadianceRecord {
    Available,
    Unavailable(HdrRadianceIssue),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrRadianceTableIssue {
    RecordCount,
    Allocation,
    RetainedBudget,
    SizeOverflow,
}

impl HdrRadianceTableIssue {
    pub fn message(self) -> &'static str {
        match self {
            Self::RecordCount => "raw table exceeds the native detail record limit",
            Self::Allocation => "raw table allocation failed",
            Self::RetainedBudget => "raw table replacement exceeds the retained CPU budget",
            Self::SizeOverflow => "raw table retained size exceeds its checked bound",
        }
    }
}

#[derive(Debug)]
struct HdrRadianceReservation {
    bytes: usize,
}

impl HdrRadianceReservation {
    fn acquire(bytes: usize) -> Result<Self, HdrRadianceTableIssue> {
        #[allow(deprecated, reason = "Atomic::try_update requires Rust 1.95")]
        HDR_RETAINED_CPU_BYTES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |retained| {
                retained
                    .checked_add(bytes)
                    .filter(|total| *total <= MAX_HDR_RETAINED_CPU_BYTES)
            })
            .map_err(|_| HdrRadianceTableIssue::RetainedBudget)?;
        Ok(Self { bytes })
    }
}

impl Drop for HdrRadianceReservation {
    fn drop(&mut self) {
        HDR_RETAINED_CPU_BYTES.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[derive(Debug)]
pub struct HdrRadiance {
    entries: Vec<[f32; 4]>,
    records: Vec<HdrRadianceRecord>,
    record_count: usize,
    issue: Option<HdrRadianceTableIssue>,
    _reservation: Option<HdrRadianceReservation>,
}

impl HdrRadiance {
    fn prepare(records: &DetailProps, baked: &BakedLighting, settings: &LightingSettings) -> Self {
        match Self::prepare_checked(records, baked, settings) {
            Ok(table) => table,
            Err(issue) => Self {
                entries: Vec::new(),
                records: Vec::new(),
                record_count: records.props.len(),
                issue: Some(issue),
                _reservation: None,
            },
        }
    }

    fn prepare_checked(
        records: &DetailProps,
        baked: &BakedLighting,
        settings: &LightingSettings,
    ) -> Result<Self, HdrRadianceTableIssue> {
        if records.props.len() > source_formats::bsp::MAX_DETAIL_PROPS {
            return Err(HdrRadianceTableIssue::RecordCount);
        }
        let retained = records
            .props
            .len()
            .checked_mul(size_of::<[f32; 4]>() + size_of::<HdrRadianceRecord>())
            .and_then(|bytes| bytes.checked_add(size_of::<Self>() + 2 * size_of::<usize>()))
            .filter(|bytes| *bytes <= MAX_HDR_TABLE_CPU_BYTES)
            .ok_or(HdrRadianceTableIssue::SizeOverflow)?;
        let reservation = HdrRadianceReservation::acquire(retained)?;
        let mut entries = Vec::new();
        let mut outcomes = Vec::new();
        entries
            .try_reserve_exact(records.props.len())
            .map_err(|_| HdrRadianceTableIssue::Allocation)?;
        outcomes
            .try_reserve_exact(records.props.len())
            .map_err(|_| HdrRadianceTableIssue::Allocation)?;
        for prop in &records.props {
            let outcome = baked
                .linear(prop, settings)
                .map_err(|_| HdrRadianceIssue::SourceUnavailable)
                .and_then(Self::pack);
            match outcome {
                Ok(entry) => {
                    entries.push(entry);
                    outcomes.push(HdrRadianceRecord::Available);
                }
                Err(issue) => {
                    entries.push([0.0; 4]);
                    outcomes.push(HdrRadianceRecord::Unavailable(issue));
                }
            }
        }
        let table = Self {
            entries,
            records: outcomes,
            record_count: records.props.len(),
            issue: None,
            _reservation: Some(reservation),
        };
        let actual_retained = table
            .retained_bytes()
            .map_err(|_| HdrRadianceTableIssue::SizeOverflow)?;
        if actual_retained > retained {
            return Err(HdrRadianceTableIssue::SizeOverflow);
        }
        Ok(table)
    }

    fn pack(raw: [f64; 3]) -> Result<[f32; 4], HdrRadianceIssue> {
        let mut entry = [0.0; 4];
        for (packed, value) in entry[..3].iter_mut().zip(raw) {
            if !value.is_finite() {
                return Err(HdrRadianceIssue::NonFinite);
            }
            if value < 0.0 {
                return Err(HdrRadianceIssue::Negative);
            }
            let converted = value as f32;
            if value > f64::from(f32::MAX)
                || !converted.is_finite()
                || (value > 0.0 && converted == 0.0)
            {
                return Err(HdrRadianceIssue::Unrepresentable);
            }
            *packed = converted;
        }
        Ok(entry)
    }

    pub fn entries(&self) -> &[[f32; 4]] {
        &self.entries
    }

    pub fn records(&self) -> &[HdrRadianceRecord] {
        &self.records
    }

    pub fn record_count(&self) -> usize {
        self.record_count
    }

    pub fn issue(&self) -> Option<HdrRadianceTableIssue> {
        self.issue
    }

    pub fn gpu_bytes(&self) -> Result<usize, String> {
        self.entries
            .len()
            .checked_mul(size_of::<[f32; 4]>())
            .filter(|bytes| *bytes <= MAX_HDR_TABLE_BYTES)
            .ok_or_else(|| "Compiled detail HDR table exceeds its GPU byte bound".into())
    }

    pub fn retained_bytes(&self) -> Result<usize, String> {
        self.entries
            .capacity()
            .checked_mul(size_of::<[f32; 4]>())
            .and_then(|bytes| {
                bytes.checked_add(
                    self.records
                        .capacity()
                        .checked_mul(size_of::<HdrRadianceRecord>())?,
                )
            })
            .and_then(|bytes| bytes.checked_add(size_of::<Self>() + 2 * size_of::<usize>()))
            .ok_or_else(|| "Compiled detail HDR retained size overflow".into())
    }

    pub fn validate_drawn(&self, ordinals: &[u32]) -> Result<(), String> {
        if ordinals.is_empty() {
            return Ok(());
        }
        if let Some(issue) = self.issue {
            return Err(format!(
                "Compiled detail HDR unavailable: {}",
                issue.message()
            ));
        }
        if self.record_count > source_formats::bsp::MAX_DETAIL_PROPS
            || self.entries.len() != self.record_count
            || self.records.len() != self.record_count
            || ordinals.len() > source_formats::bsp::MAX_DETAIL_PROPS
        {
            return Err("Compiled detail HDR table and draw ordinals are inconsistent".into());
        }
        for &ordinal in ordinals {
            let record = usize::try_from(ordinal)
                .ok()
                .and_then(|index| self.records.get(index))
                .ok_or("Compiled detail HDR draw ordinal exceeds the retained table")?;
            if let HdrRadianceRecord::Unavailable(issue) = record {
                return Err(format!(
                    "Detail record {ordinal} HDR unavailable: {}",
                    issue.message()
                ));
            }
        }
        Ok(())
    }

    pub fn max_drawn(&self, ordinals: &[u32]) -> Result<[f64; 3], String> {
        self.validate_drawn(ordinals)?;
        let mut maximum = [0.0f64; 3];
        for &ordinal in ordinals {
            let entry = self.entries[ordinal as usize];
            for (bound, value) in maximum.iter_mut().zip(entry) {
                *bound = bound.max(f64::from(value));
            }
        }
        Ok(maximum)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Lighting {
    #[default]
    EditorShading,
    BakedLdr,
    BakedRadiance,
}

impl Lighting {
    pub fn name(self) -> &'static str {
        match self {
            Self::EditorShading => "editor_shading",
            Self::BakedLdr => "baked_ldr",
            Self::BakedRadiance => "baked_radiance",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "editor_shading" => Ok(Self::EditorShading),
            "baked_ldr" => Ok(Self::BakedLdr),
            "baked_radiance" => Ok(Self::BakedRadiance),
            _ => Err(
                "Compiled detail lighting must be editor_shading, baked_ldr or baked_radiance"
                    .into(),
            ),
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct StyleGain {
    #[schemars(range(min = 1, max = 63))]
    pub style: u8,
    #[schemars(range(min = 0, max = MAX_STYLE_GAIN))]
    pub gain: f64,
}

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize)]
pub struct LightingSettings {
    pub mode: Lighting,
    pub channel: crate::LightingChannel,
    pub exposure_ev: f64,
    pub style_gains: Vec<StyleGain>,
}

impl LightingSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !self.exposure_ev.is_finite()
            || !(MIN_EXPOSURE_EV..=MAX_EXPOSURE_EV).contains(&self.exposure_ev)
        {
            return Err("Compiled detail exposure must be finite and between -8 and 8 EV".into());
        }
        if self.style_gains.len() > 63 {
            return Err("Compiled detail preview accepts at most 63 style gains".into());
        }
        let mut seen = [false; 64];
        for entry in &self.style_gains {
            if !(1..=63).contains(&entry.style)
                || seen[usize::from(entry.style)]
                || !entry.gain.is_finite()
                || !(0.0..=MAX_STYLE_GAIN).contains(&entry.gain)
            {
                return Err(
                    "Compiled detail gains require distinct styles 1–63 and finite gains 0–2"
                        .into(),
                );
            }
            seen[usize::from(entry.style)] = true;
        }
        if self.mode != Lighting::BakedRadiance
            && (self.channel != crate::LightingChannel::Ldr
                || self.exposure_ev != 0.0
                || !self.style_gains.is_empty())
        {
            return Err("Channel, exposure and style gains require Baked radiance".into());
        }
        Ok(())
    }

    pub fn gain(&self, style: u8) -> f64 {
        if style == 0 {
            return 1.0;
        }
        self.style_gains
            .iter()
            .find(|entry| entry.style == style)
            .map_or(0.0, |entry| entry.gain)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StaticSource {
    SharedNativeBase,
    SelectedChannelStyleZero,
    SharedNativeBaseAndSelectedChannelStyleZero,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaticLighting {
    pub source: StaticSource,
    pub style_zero: Option<RgbExp32>,
    pub linear: [f64; 3],
}

#[derive(Debug, Default, Clone)]
pub struct BakedLighting {
    pub ldr: Option<Vec<DetailPropLighting>>,
    pub hdr: Option<Vec<DetailPropLighting>>,
    pub base_channel: Option<crate::LightingChannel>,
}

impl BakedLighting {
    pub fn table(&self, channel: crate::LightingChannel) -> Option<&[DetailPropLighting]> {
        match channel {
            crate::LightingChannel::Ldr => self.ldr.as_deref(),
            crate::LightingChannel::Hdr => self.hdr.as_deref(),
        }
    }

    pub fn validate(&self, records: &DetailProps) -> Result<(), String> {
        records
            .validate_lighting(self.ldr.as_deref(), self.hdr.as_deref())
            .map_err(|error| error.to_string())
    }

    pub fn channel_issue(
        &self,
        records: &DetailProps,
        channel: crate::LightingChannel,
    ) -> Option<String> {
        if self.base_channel.is_none() {
            return Some(
                "Baked radiance requires a lit build; Fullbright has no baked channel".into(),
            );
        }
        if channel == crate::LightingChannel::Hdr
            && self.base_channel != Some(crate::LightingChannel::Hdr)
        {
            return Some("The retained build did not produce an HDR lighting channel".into());
        }
        if records.props.iter().any(|prop| prop.light_style_count != 0)
            && self.table(channel).is_none()
        {
            return Some(format!(
                "The selected {} detail style table is absent",
                channel.name()
            ));
        }
        None
    }

    pub fn available_styles(&self, records: &DetailProps) -> Vec<u8> {
        let mut styles = std::collections::BTreeSet::new();
        for channel in [crate::LightingChannel::Ldr, crate::LightingChannel::Hdr] {
            for prop in &records.props {
                if let Ok(range) = self.extras(prop, channel) {
                    styles.extend(
                        range
                            .iter()
                            .map(|entry| entry.style)
                            .filter(|&style| style != 0),
                    );
                }
            }
        }
        styles.into_iter().collect()
    }

    pub fn has_channel_static(&self, records: &DetailProps) -> bool {
        [crate::LightingChannel::Ldr, crate::LightingChannel::Hdr]
            .into_iter()
            .any(|channel| {
                records.props.iter().any(|prop| {
                    self.extras(prop, channel)
                        .is_ok_and(|entries| entries.iter().any(|entry| entry.style == 0))
                })
            })
    }

    pub fn extras(
        &self,
        prop: &DetailProp,
        channel: crate::LightingChannel,
    ) -> Result<&[DetailPropLighting], String> {
        if prop.light_style_count == 0 {
            return Ok(&[]);
        }
        let table = self
            .table(channel)
            .ok_or("Selected detail style table is absent")?;
        let start = prop.light_styles as usize;
        start
            .checked_add(usize::from(prop.light_style_count))
            .and_then(|end| table.get(start..end))
            .ok_or_else(|| "Detail style range exceeds the selected table".into())
    }

    pub fn linear(
        &self,
        prop: &DetailProp,
        settings: &LightingSettings,
    ) -> Result<[f64; 3], String> {
        let mut color = self.static_lighting(prop, settings.channel)?.linear;
        for entry in self.extras(prop, settings.channel)? {
            if entry.style == 0 {
                continue;
            }
            let gain = settings.gain(entry.style);
            for (total, value) in color.iter_mut().zip(baked_ldr_linear(entry.lighting)) {
                *total += value * gain;
            }
        }
        Ok(color)
    }

    pub fn static_lighting(
        &self,
        prop: &DetailProp,
        channel: crate::LightingChannel,
    ) -> Result<StaticLighting, String> {
        let style_zero = self
            .extras(prop, channel)?
            .iter()
            .find(|entry| entry.style == 0)
            .map(|entry| entry.lighting);
        let mut linear = baked_ldr_linear(prop.lighting);
        if let Some(color) = style_zero {
            for (total, value) in linear.iter_mut().zip(baked_ldr_linear(color)) {
                *total += value;
            }
        }
        let source = match (style_zero.is_some(), prop.lighting.rgb != [0; 3]) {
            (false, _) => StaticSource::SharedNativeBase,
            (true, false) => StaticSource::SelectedChannelStyleZero,
            (true, true) => StaticSource::SharedNativeBaseAndSelectedChannelStyleZero,
        };
        Ok(StaticLighting {
            source,
            style_zero,
            linear,
        })
    }

    pub fn baked_ldr_issue(&self, records: &DetailProps) -> Option<String> {
        for (ordinal, prop) in records.props.iter().enumerate() {
            let entries = match self.extras(prop, crate::LightingChannel::Ldr) {
                Ok(entries) => entries,
                Err(error) => return Some(format!("Detail record {ordinal}: {error}")),
            };
            let runtime_styles = entries.iter().filter(|entry| entry.style != 0).count();
            if runtime_styles != 0 {
                return Some(format!(
                    "Detail record {ordinal} uses {runtime_styles} runtime light styles; Baked LDR requires static lighting only"
                ));
            }
        }
        None
    }

    pub fn colors(
        &self,
        records: &DetailProps,
        settings: &LightingSettings,
    ) -> Result<Vec<[f64; 3]>, String> {
        settings.validate()?;
        self.validate(records)?;
        if settings.mode == Lighting::EditorShading {
            return Ok(Vec::new());
        }
        if settings.mode == Lighting::BakedLdr {
            if let Some(issue) = self.baked_ldr_issue(records) {
                return Err(issue);
            }
            return records
                .props
                .iter()
                .map(|prop| {
                    self.static_lighting(prop, crate::LightingChannel::Ldr)
                        .map(|color| color.linear)
                })
                .collect();
        }
        if let Some(issue) = self.channel_issue(records, settings.channel) {
            return Err(issue);
        }
        let available = self.available_styles(records);
        if settings
            .style_gains
            .iter()
            .any(|entry| !available.contains(&entry.style))
        {
            return Err(
                "Compiled detail gain names a style absent from the retained receivers".into(),
            );
        }
        let exposure = 2.0f64.powf(settings.exposure_ev);
        let mut colors = Vec::with_capacity(records.props.len());
        for (ordinal, prop) in records.props.iter().enumerate() {
            let exposed = self.linear(prop, settings)?.map(|value| value * exposure);
            if exposed
                .iter()
                .any(|value| !value.is_finite() || *value > f64::from(f32::MAX))
            {
                return Err(format!(
                    "Detail record {ordinal} exposed radiance exceeds finite GPU range"
                ));
            }
            colors.push(exposed);
        }
        Ok(colors)
    }
}

pub fn baked_ldr_linear(color: RgbExp32) -> [f64; 3] {
    color.linear().map(|channel| channel / 255.0)
}

#[derive(Debug, Clone)]
pub struct Part {
    pub ordinal: usize,
    pub vertices: std::ops::Range<usize>,
    pub wires: std::ops::Range<usize>,
    pub billboards: std::ops::Range<usize>,
    pub bounds: [DVec3; 2],
}

impl Part {
    pub fn intersects(&self, region: [DVec3; 2]) -> bool {
        self.bounds[0].cmple(region[1]).all() && self.bounds[1].cmpge(region[0]).all()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Billboard {
    pub origin: [f32; 3],
    pub offsets: [[f32; 2]; 4],
    pub orientation: u8,
}

impl Billboard {
    pub fn quad(self, eye: Vec3) -> [Vec3; 4] {
        let origin = Vec3::from_array(self.origin);
        let mut direction = eye - origin;
        if self.orientation == 2 {
            direction.z = 0.0;
        }
        let maximum = direction.abs().max_element();
        if maximum > 0.0 {
            direction /= maximum;
        }
        let horizontal = direction.x.hypot(direction.y);
        let (right, up) = if horizontal == 0.0 {
            (
                Vec3::NEG_Y,
                if direction.z > 0.0 {
                    Vec3::NEG_X
                } else {
                    Vec3::X
                },
            )
        } else {
            let right = Vec3::new(direction.y / horizontal, -direction.x / horizontal, 0.0);
            let length = direction.length();
            (
                right,
                Vec3::new(
                    -direction.x * direction.z / (horizontal * length),
                    -direction.y * direction.z / (horizontal * length),
                    horizontal / length,
                ),
            )
        };
        self.offsets.map(|[x, y]| origin + right * x + up * y)
    }

    pub fn bounds(self) -> [DVec3; 2] {
        let radius = self
            .offsets
            .into_iter()
            .map(|[x, y]| f64::from(x).hypot(f64::from(y)))
            .fold(0.0f64, f64::max)
            + 1.0 / 64.0;
        let origin = DVec3::from_array(self.origin.map(f64::from));
        [origin - DVec3::splat(radius), origin + DVec3::splat(radius)]
    }

    fn new(prop: &DetailProp, sprite: DetailSprite) -> Result<Self, String> {
        if prop.kind != 1
            || !matches!(prop.orientation, 1 | 2)
            || !prop.scale.is_finite()
            || prop.scale <= 0.0
            || !Vec3::from_array(prop.origin).is_finite()
            || [
                sprite.upper_left,
                sprite.lower_right,
                sprite.texture_upper_left,
                sprite.texture_lower_right,
            ]
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite())
            || sprite.upper_left[0] >= sprite.lower_right[0]
            || sprite.upper_left[1] <= sprite.lower_right[1]
        {
            return Err("Invalid compiled billboard geometry".into());
        }
        let [left, top] = sprite.upper_left.map(|value| value * prop.scale);
        let [right, bottom] = sprite.lower_right.map(|value| value * prop.scale);
        let offsets = [[left, top], [left, bottom], [right, bottom], [right, top]];
        if offsets
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err("Compiled billboard offsets must be finite".into());
        }
        Ok(Self {
            origin: prop.origin,
            offsets,
            orientation: prop.orientation,
        })
    }
}

pub fn rest_quads(prop: &DetailProp, sprite: DetailSprite) -> Result<Vec<[DVec3; 4]>, String> {
    if [
        sprite.upper_left,
        sprite.lower_right,
        sprite.texture_upper_left,
        sprite.texture_lower_right,
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.is_finite())
        || !(1..=3).contains(&prop.kind)
        || prop.orientation > 2
        || !prop.scale.is_finite()
        || prop.scale <= 0.0
        || sprite.upper_left[0] >= sprite.lower_right[0]
        || sprite.upper_left[1] <= sprite.lower_right[1]
        || prop.orientation != 0
        || prop.kind == 2 && (prop.shape_angle != 0 || prop.shape_size != 0)
    {
        return Err("Unsupported compiled detail card geometry".into());
    }
    let origin = DVec3::from_array(prop.origin.map(f64::from));
    let radians = DVec3::from_array(prop.angles.map(f64::from)).map(f64::to_radians);
    let rotation = DQuat::from_euler(EulerRot::ZYX, radians.y, radians.x, radians.z);
    let width = f64::from(sprite.lower_right[0]) - f64::from(sprite.upper_left[0]);
    let height = f64::from(sprite.lower_right[1]) - f64::from(sprite.upper_left[1]);
    let mut quads = Vec::with_capacity(4);
    if prop.kind == 2 {
        let base = rotation * DVec3::Z * f64::from(sprite.upper_left[1]);
        let vertical = rotation * DVec3::Z * height;
        for axis in [DVec3::NEG_X, DVec3::Y, DVec3::X, DVec3::NEG_Y] {
            let horizontal = rotation * axis * (width * 0.5);
            quads.push([
                base,
                base + vertical,
                base + vertical + horizontal,
                base + horizontal,
            ]);
        }
    } else {
        let branches = if prop.kind == 3 { 3 } else { 1 };
        for branch in 0..branches {
            let basis = rotation
                * DQuat::from_euler(
                    EulerRot::ZYX,
                    f64::from(branch) * std::f64::consts::TAU / 3.0,
                    if prop.kind == 3 {
                        f64::from(prop.shape_angle).to_radians()
                    } else {
                        0.0
                    },
                    0.0,
                );
            let right = basis * DVec3::NEG_Y;
            let up = basis * DVec3::Z;
            let offset = if prop.kind == 3 {
                basis * DVec3::X * f64::from(f32::from(prop.shape_size) * (1.0_f32 / 255.0)) * width
            } else {
                DVec3::ZERO
            };
            let base = right * f64::from(sprite.upper_left[0])
                + up * f64::from(sprite.upper_left[1])
                + offset;
            let horizontal = right * width;
            let vertical = up * height;
            quads.push([
                base,
                base + vertical,
                base + vertical + horizontal,
                base + horizontal,
            ]);
        }
    }
    for quad in &mut quads {
        for point in quad {
            *point = origin + *point * f64::from(prop.scale);
            if !point.is_finite()
                || point.abs().max_element() > map_core::checks::SOURCE_COORDINATE_LIMIT
            {
                return Err("Compiled detail card bounds exceed finite Source coordinates".into());
            }
        }
    }
    Ok(quads)
}

pub fn model_scene(fs: &GameFs, materials: &mut MaterialLibrary, models: &[Instance]) -> SceneMesh {
    let document = map_core::Document::empty();
    SceneMesh::build_with_reference_models_filtered(
        document.vmf(),
        fs,
        materials,
        (&HashSet::new(), &HashSet::new()),
        &[],
        (&[], &BTreeMap::new()),
        (&BTreeMap::new(), &[]),
        None,
        true,
        false,
        false,
        false,
        models,
        |_| false,
    )
}

impl SceneMesh {
    pub fn set_compiled_detail_lighting(
        &mut self,
        records: &DetailProps,
        lighting: Lighting,
    ) -> Result<(), String> {
        self.set_compiled_detail_lighting_options(
            records,
            &BakedLighting::default(),
            &LightingSettings {
                mode: lighting,
                ..Default::default()
            },
        )
    }

    pub fn set_compiled_detail_lighting_options(
        &mut self,
        records: &DetailProps,
        baked: &BakedLighting,
        settings: &LightingSettings,
    ) -> Result<(), String> {
        let radiance = baked.colors(records, settings)?;
        if records.props.len() > source_formats::bsp::MAX_DETAIL_PROPS
            || self.detail_parts.len() != records.props.len()
        {
            return Err("Compiled detail lighting requires complete native record ranges".into());
        }
        let mut seen = vec![false; records.props.len()];
        let mut cursor = 0;
        for part in &self.detail_parts {
            if part.ordinal >= seen.len()
                || seen[part.ordinal]
                || part.vertices.start != cursor
                || part.vertices.start > part.vertices.end
                || part.vertices.end > self.vertices.len()
            {
                return Err("Compiled detail lighting ranges are inconsistent".into());
            }
            seen[part.ordinal] = true;
            cursor = part.vertices.end;
        }
        if cursor != self.vertices.len() {
            return Err("Compiled detail lighting ranges are incomplete".into());
        }
        let mut hdr_ordinals = Vec::new();
        let mut hdr_draw_issue = None;
        let hdr = if settings.mode == Lighting::BakedRadiance {
            if hdr_ordinals.try_reserve_exact(records.props.len()).is_err() {
                hdr_draw_issue = Some(HdrRadianceTableIssue::Allocation);
            } else if hdr_ordinals.capacity() > source_formats::bsp::MAX_DETAIL_PROPS {
                hdr_ordinals = Vec::new();
                hdr_draw_issue = Some(HdrRadianceTableIssue::SizeOverflow);
            }
            for part in &self.detail_parts {
                let ordinal = u32::try_from(part.ordinal)
                    .map_err(|_| "Compiled detail ordinal exceeds renderer bounds")?;
                ordinal
                    .checked_mul(1 << LIGHTING_RECEIVER_KIND_BITS)
                    .ok_or("Compiled detail receiver ordinal overflow")?;
                if hdr_draw_issue.is_none() && !part.vertices.is_empty() {
                    hdr_ordinals.push(ordinal);
                }
            }
            Some(Arc::new(HdrRadiance::prepare(records, baked, settings)))
        } else {
            None
        };
        for part in &self.detail_parts {
            let color = match settings.mode {
                Lighting::EditorShading | Lighting::BakedRadiance => [255; 3],
                Lighting::BakedLdr => radiance[part.ordinal]
                    .map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8),
            };
            for vertex in &mut self.vertices[part.vertices.clone()] {
                vertex.color[..3].copy_from_slice(&color);
                vertex.baked_irradiance = if settings.mode == Lighting::BakedRadiance {
                    radiance[part.ordinal].map(|value| value as f32)
                } else {
                    [0.0; 3]
                };
                vertex.lighting_receiver = match settings.mode {
                    Lighting::EditorShading => 0,
                    Lighting::BakedLdr => BAKED_LIGHTING_RECEIVER,
                    Lighting::BakedRadiance => {
                        ((part.ordinal as u32) << LIGHTING_RECEIVER_KIND_BITS)
                            | BAKED_RADIANCE_RECEIVER
                    }
                };
            }
        }
        self.compiled_detail_hdr = hdr;
        self.compiled_detail_hdr_ordinals = hdr_ordinals;
        self.compiled_detail_hdr_draw_issue = hdr_draw_issue;
        Ok(())
    }

    pub fn add_compiled_detail_card(
        &mut self,
        material: usize,
        prop: &DetailProp,
        sprite: DetailSprite,
    ) -> Result<(), String> {
        if prop.kind == 1 && matches!(prop.orientation, 1 | 2) {
            return self.add_compiled_detail_billboard(material, prop, sprite);
        }
        let quads = rest_quads(prop, sprite)?;
        let vertices = quads.len() * 4;
        let indices = quads.len() * 12;
        self.check_compiled_detail_capacity(vertices, indices)?;
        let first_index = self.indices.len() as u32;
        let uvs = [
            sprite.texture_upper_left,
            [sprite.texture_upper_left[0], sprite.texture_lower_right[1]],
            sprite.texture_lower_right,
            [sprite.texture_lower_right[0], sprite.texture_upper_left[1]],
        ];
        for (branch, quad) in quads.into_iter().enumerate() {
            let first = self.vertices.len() as u32;
            let normal = (quad[1] - quad[0])
                .cross(quad[2] - quad[0])
                .normalize_or_zero()
                .as_vec3()
                .to_array();
            for (index, point) in quad.into_iter().enumerate() {
                let mut uv = uvs[index];
                if prop.kind == 2 {
                    let [left, right] =
                        [sprite.texture_upper_left[0], sprite.texture_lower_right[0]];
                    let middle = (left + right) * 0.5;
                    uv[0] = if index < 2 {
                        middle
                    } else if branch == 0 || branch == 3 {
                        right
                    } else {
                        left
                    };
                }
                self.vertices.push(Vertex {
                    position: point.as_vec3().to_array(),
                    normal,
                    uv,
                    color: [255, 255, 255, 0],
                    lightmap: [0.0, 0.0, -1.0],
                    smoothing: 0,
                    opacity: 1.0,
                    texture_alpha: 0,
                    tangent: [0.0; 4],
                    baked_irradiance: [0.0; 3],
                    lighting_receiver: 0,
                });
                self.mins = self.mins.min(point.as_vec3());
                self.maxs = self.maxs.max(point.as_vec3());
            }
            self.indices
                .extend([0, 1, 2, 0, 2, 3, 2, 1, 0, 3, 2, 0].map(|index| first + index));
            for (a, b) in [(0, 1), (1, 2), (2, 3), (3, 0)] {
                self.wire_lines.extend([a, b].map(|index| LineVertex {
                    position: quad[index].as_vec3().to_array(),
                    color: [90, 125, 150, 255],
                }));
            }
        }
        self.batches.push(Batch {
            material,
            first_index,
            index_count: self.indices.len() as u32 - first_index,
            rope: false,
            model_translucent: true,
        });
        Ok(())
    }

    fn add_compiled_detail_billboard(
        &mut self,
        material: usize,
        prop: &DetailProp,
        sprite: DetailSprite,
    ) -> Result<(), String> {
        let billboard = Billboard::new(prop, sprite)?;
        self.check_compiled_detail_capacity(4, 12)?;
        let first = self.vertices.len() as u32;
        let first_index = self.indices.len() as u32;
        let uvs = [
            sprite.texture_upper_left,
            [sprite.texture_upper_left[0], sprite.texture_lower_right[1]],
            sprite.texture_lower_right,
            [sprite.texture_lower_right[0], sprite.texture_upper_left[1]],
        ];
        for (offset, uv) in billboard.offsets.into_iter().zip(uvs) {
            self.vertices.push(Vertex {
                position: prop.origin,
                normal: [0.0, 0.0, 1.0],
                uv,
                color: [255, 255, 255, 0],
                lightmap: [
                    offset[0],
                    offset[1],
                    if prop.orientation == 1 {
                        FULL_BILLBOARD_TAG
                    } else {
                        VERTICAL_BILLBOARD_TAG
                    },
                ],
                smoothing: 0,
                opacity: 1.0,
                texture_alpha: 0,
                tangent: [0.0; 4],
                baked_irradiance: [0.0; 3],
                lighting_receiver: 0,
            });
        }
        self.indices
            .extend([0, 1, 2, 0, 2, 3, 2, 1, 0, 3, 2, 0].map(|index| first + index));
        self.batches.push(Batch {
            material,
            first_index,
            index_count: 12,
            rope: false,
            model_translucent: true,
        });
        let radius = billboard
            .offsets
            .into_iter()
            .map(|[x, y]| x.hypot(y))
            .fold(0.0f32, f32::max);
        let origin = Vec3::from_array(prop.origin);
        self.mins = self.mins.min(origin - Vec3::splat(radius));
        self.maxs = self.maxs.max(origin + Vec3::splat(radius));
        self.detail_billboards.push(billboard);
        Ok(())
    }

    pub fn check_compiled_detail_capacity(
        &self,
        vertices: usize,
        indices: usize,
    ) -> Result<(), String> {
        let vertices = self
            .vertices
            .len()
            .checked_add(vertices)
            .ok_or("Compiled detail vertex count overflow")?;
        let indices = self
            .indices
            .len()
            .checked_add(indices)
            .ok_or("Compiled detail index count overflow")?;
        let bytes = vertices
            .checked_mul(size_of::<Vertex>())
            .and_then(|bytes| {
                bytes.checked_add(
                    indices.checked_mul(size_of::<u32>() + 2 * size_of::<LineVertex>())?,
                )
            })
            .and_then(|bytes| bytes.checked_mul(2))
            .and_then(|bytes| bytes.checked_add(size_of::<Self>() - HDR_SCENE_METADATA_BYTES));
        if vertices > MAX_VERTICES
            || indices > MAX_INDICES
            || bytes.is_none_or(|bytes| bytes > MAX_BYTES)
        {
            return Err("Compiled details exceed the 32 MiB geometry budget".into());
        }
        Ok(())
    }

    pub fn compiled_detail_bytes(&self) -> usize {
        size_of::<Self>() - HDR_SCENE_METADATA_BYTES
            + self.vertices.capacity() * size_of::<Vertex>()
            + self.indices.capacity() * size_of::<u32>()
            + self.batches.capacity() * size_of::<Batch>()
            + self.wire_lines.capacity() * size_of::<LineVertex>()
            + self.detail_billboards.capacity() * size_of::<Billboard>()
            + self.detail_parts.capacity() * size_of::<Part>()
    }

    pub fn compiled_detail_hdr_retained_bytes(&self) -> Result<usize, String> {
        self.compiled_detail_hdr_ordinals
            .capacity()
            .checked_mul(size_of::<u32>())
            .filter(|bytes| *bytes <= MAX_HDR_DRAW_METADATA_BYTES)
            .and_then(|bytes| bytes.checked_add(HDR_SCENE_METADATA_BYTES))
            .and_then(|bytes| {
                bytes.checked_add(
                    self.compiled_detail_hdr
                        .as_ref()
                        .map_or(Ok(0), |table| table.retained_bytes())
                        .ok()?,
                )
            })
            .ok_or_else(|| "Compiled detail HDR retained metadata exceeds its checked bound".into())
    }

    pub fn validate_compiled_detail_hdr(&self) -> Result<(), String> {
        if let Some(issue) = self.compiled_detail_hdr_draw_issue {
            return Err(format!(
                "Compiled detail HDR unavailable: {}",
                issue.message()
            ));
        }
        if self.compiled_detail_hdr_ordinals.is_empty() {
            return Ok(());
        }
        self.compiled_detail_hdr
            .as_ref()
            .ok_or("Compiled detail HDR table is absent for required drawn records")?
            .validate_drawn(&self.compiled_detail_hdr_ordinals)
    }

    fn validate_compiled_detail_layout(&self) -> Result<(), String> {
        if self.detail_parts.len() > source_formats::bsp::MAX_DETAIL_PROPS {
            return Err("Compiled detail ranges exceed the native record limit".into());
        }
        let mut seen = Vec::new();
        seen.try_reserve_exact(self.detail_parts.len())
            .map_err(|_| "Compiled detail range metadata allocation failed")?;
        seen.resize(self.detail_parts.len(), false);
        let mut cursor = [0usize; 3];
        for part in &self.detail_parts {
            if part.ordinal >= seen.len() || seen[part.ordinal] {
                return Err("Compiled detail original record ordinals are inconsistent".into());
            }
            seen[part.ordinal] = true;
            for (slot, range, maximum) in [
                (0, &part.vertices, self.vertices.len()),
                (1, &part.wires, self.wire_lines.len()),
                (2, &part.billboards, self.detail_billboards.len()),
            ] {
                if range.start != cursor[slot] || range.start > range.end || range.end > maximum {
                    return Err("Compiled detail spatial ranges are inconsistent".into());
                }
                cursor[slot] = range.end;
            }
            for vertex in &self.vertices[part.vertices.clone()] {
                let kind = vertex.lighting_receiver & LIGHTING_RECEIVER_KIND_MASK;
                if self.compiled_detail_hdr.is_some() {
                    let ordinal = vertex.lighting_receiver >> LIGHTING_RECEIVER_KIND_BITS;
                    if kind != BAKED_RADIANCE_RECEIVER || ordinal as usize != part.ordinal {
                        return Err(
                            "Compiled detail vertex does not match its raw record ordinal".into(),
                        );
                    }
                } else if kind == BAKED_RADIANCE_RECEIVER {
                    return Err("Compiled detail radiance vertex has no paired raw table".into());
                }
            }
        }
        if cursor
            != [
                self.vertices.len(),
                self.wire_lines.len(),
                self.detail_billboards.len(),
            ]
        {
            return Err("Compiled detail spatial ranges are incomplete".into());
        }
        if let Some(table) = &self.compiled_detail_hdr {
            if table.record_count() != self.detail_parts.len()
                || (self.compiled_detail_hdr_draw_issue.is_none()
                    && !self.compiled_detail_hdr_ordinals.iter().copied().eq(self
                        .detail_parts
                        .iter()
                        .filter(|part| !part.vertices.is_empty())
                        .map(|part| part.ordinal as u32)))
            {
                return Err("Compiled detail raw table and native record ranges differ".into());
            }
        } else if !self.compiled_detail_hdr_ordinals.is_empty()
            || self.compiled_detail_hdr_draw_issue.is_some()
        {
            return Err("Compiled detail draw ordinals have no paired raw table".into());
        }
        self.compiled_detail_hdr_retained_bytes()?;
        for batch in &self.batches {
            let end = batch
                .first_index
                .checked_add(batch.index_count)
                .ok_or("Compiled detail batch range overflow")?;
            let indices = self
                .indices
                .get(batch.first_index as usize..end as usize)
                .ok_or("Compiled detail batch exceeds the retained index range")?;
            let (triangles, remaining) = indices.as_chunks::<3>();
            if !remaining.is_empty() {
                return Err("Compiled detail batch contains an incomplete triangle".into());
            }
            for triangle in triangles {
                let mut ordinal = None;
                for &index in triangle {
                    let index = usize::try_from(index)
                        .map_err(|_| "Compiled detail index exceeds host bounds")?;
                    let part = self
                        .detail_parts
                        .get(
                            self.detail_parts
                                .partition_point(|part| part.vertices.end <= index),
                        )
                        .filter(|part| part.vertices.contains(&index))
                        .ok_or("Compiled detail triangle has no native record range")?;
                    if ordinal.is_some_and(|ordinal| ordinal != part.ordinal) {
                        return Err("Compiled detail triangle spans multiple native records".into());
                    }
                    ordinal = Some(part.ordinal);
                }
            }
        }
        Ok(())
    }

    pub fn append_compiled_details(
        &mut self,
        details: &Self,
        region: Option<[DVec3; 2]>,
    ) -> Result<(), String> {
        let included = region
            .map(|region| details.compiled_detail_included(|part| part.intersects(region)))
            .transpose()?;
        self.append_compiled_details_included(details, included)
    }

    pub fn append_compiled_details_filtered(
        &mut self,
        details: &Self,
        include: impl FnMut(&Part) -> bool,
    ) -> Result<(), String> {
        let included = details.compiled_detail_included(include)?;
        self.append_compiled_details_included(details, Some(included))
    }

    fn compiled_detail_included(
        &self,
        include: impl FnMut(&Part) -> bool,
    ) -> Result<Vec<bool>, String> {
        let mut included = Vec::new();
        included
            .try_reserve_exact(self.detail_parts.len())
            .map_err(|_| "Compiled detail spatial filter allocation failed")?;
        included.extend(self.detail_parts.iter().map(include));
        Ok(included)
    }

    fn append_compiled_details_included(
        &mut self,
        details: &Self,
        included: Option<Vec<bool>>,
    ) -> Result<(), String> {
        details.validate_compiled_detail_layout()?;
        if self.compiled_detail_hdr.is_some()
            || !self.compiled_detail_hdr_ordinals.is_empty()
            || self.compiled_detail_hdr_draw_issue.is_some()
        {
            return Err("Compiled detail radiance was already appended to the shared scene".into());
        }
        let mut allowed_vertices = Vec::new();
        if let Some(included) = &included {
            if included.len() != details.detail_parts.len() {
                return Err("Compiled detail spatial filter omits native records".into());
            }
            allowed_vertices
                .try_reserve_exact(details.vertices.len())
                .map_err(|_| "Compiled detail filter allocation failed")?;
            allowed_vertices.resize(details.vertices.len(), false);
            for (part, enabled) in details.detail_parts.iter().zip(included) {
                if *enabled {
                    allowed_vertices[part.vertices.clone()].fill(true);
                }
            }
            if !included.iter().any(|enabled| *enabled) {
                return Ok(());
            }
        }
        for (existing, extra) in [
            (self.vertices.len(), details.vertices.len()),
            (self.indices.len(), details.indices.len()),
            (self.wire_lines.len(), details.wire_lines.len()),
        ] {
            if existing
                .checked_add(extra)
                .is_none_or(|count| count > u32::MAX as usize)
            {
                return Err("Combined detail scene exceeds renderer index bounds".into());
            }
        }
        self.vertices
            .try_reserve_exact(details.vertices.len())
            .map_err(|_| "Compiled detail vertex allocation failed")?;
        self.indices
            .try_reserve_exact(details.indices.len())
            .map_err(|_| "Compiled detail index allocation failed")?;
        self.batches
            .try_reserve_exact(details.batches.len())
            .map_err(|_| "Compiled detail batch allocation failed")?;
        self.wire_lines
            .try_reserve_exact(details.wire_lines.len())
            .map_err(|_| "Compiled detail line allocation failed")?;
        self.detail_billboards
            .try_reserve_exact(details.detail_billboards.len())
            .map_err(|_| "Compiled detail billboard allocation failed")?;
        let mut hdr_ordinals = Vec::new();
        let hdr_required = details
            .detail_parts
            .iter()
            .enumerate()
            .any(|(index, part)| {
                !part.vertices.is_empty()
                    && included.as_ref().is_none_or(|included| included[index])
            });
        let mut hdr_draw_issue = details
            .compiled_detail_hdr_draw_issue
            .filter(|_| hdr_required);
        if details.compiled_detail_hdr.is_some() && hdr_required && hdr_draw_issue.is_none() {
            if hdr_ordinals
                .try_reserve_exact(details.detail_parts.len())
                .is_err()
            {
                hdr_draw_issue = Some(HdrRadianceTableIssue::Allocation);
            } else if hdr_ordinals.capacity() > source_formats::bsp::MAX_DETAIL_PROPS {
                hdr_ordinals = Vec::new();
                hdr_draw_issue = Some(HdrRadianceTableIssue::SizeOverflow);
            }
            for (index, part) in details.detail_parts.iter().enumerate() {
                if hdr_draw_issue.is_none()
                    && !part.vertices.is_empty()
                    && included.as_ref().is_none_or(|included| included[index])
                {
                    hdr_ordinals.push(part.ordinal as u32);
                }
            }
        }
        let first_vertex = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&details.vertices);
        if let Some(included) = included {
            for batch in &details.batches {
                let first_index = self.indices.len() as u32;
                let range =
                    batch.first_index as usize..(batch.first_index + batch.index_count) as usize;
                for triangle in details.indices[range].as_chunks::<3>().0 {
                    if triangle
                        .iter()
                        .all(|index| allowed_vertices[*index as usize])
                    {
                        self.indices
                            .extend(triangle.iter().map(|index| first_vertex + index));
                    }
                }
                let index_count = self.indices.len() as u32 - first_index;
                if index_count != 0 {
                    self.batches.push(Batch {
                        first_index,
                        index_count,
                        ..*batch
                    });
                }
            }
            for (part, enabled) in details.detail_parts.iter().zip(included) {
                if !enabled {
                    continue;
                }
                self.wire_lines
                    .extend_from_slice(&details.wire_lines[part.wires.clone()]);
                self.detail_billboards
                    .extend_from_slice(&details.detail_billboards[part.billboards.clone()]);
                self.mins = self.mins.min(part.bounds[0].as_vec3());
                self.maxs = self.maxs.max(part.bounds[1].as_vec3());
            }
        } else {
            let first_index = self.indices.len() as u32;
            self.indices
                .extend(details.indices.iter().map(|index| first_vertex + index));
            self.batches
                .extend(details.batches.iter().map(|batch| Batch {
                    first_index: first_index + batch.first_index,
                    ..*batch
                }));
            self.wire_lines.extend_from_slice(&details.wire_lines);
            self.detail_billboards
                .extend_from_slice(&details.detail_billboards);
            if !details.vertices.is_empty() {
                self.mins = self.mins.min(details.mins);
                self.maxs = self.maxs.max(details.maxs);
            }
        }
        self.compiled_detail_hdr = details.compiled_detail_hdr.clone();
        self.compiled_detail_hdr_ordinals = hdr_ordinals;
        self.compiled_detail_hdr_draw_issue = hdr_draw_issue;
        Ok(())
    }
}
