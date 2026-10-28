use std::collections::{BTreeMap, HashSet};

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
pub const MIN_EXPOSURE_EV: f64 = -8.0;
pub const MAX_EXPOSURE_EV: f64 = 8.0;
pub const MAX_STYLE_GAIN: f64 = 2.0;

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
                    Lighting::BakedRadiance => BAKED_RADIANCE_RECEIVER,
                };
            }
        }
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
            .and_then(|bytes| bytes.checked_add(size_of::<Self>()));
        if vertices > MAX_VERTICES
            || indices > MAX_INDICES
            || bytes.is_none_or(|bytes| bytes > MAX_BYTES)
        {
            return Err("Compiled details exceed the 32 MiB geometry budget".into());
        }
        Ok(())
    }

    pub fn compiled_detail_bytes(&self) -> usize {
        size_of::<Self>()
            + self.vertices.capacity() * size_of::<Vertex>()
            + self.indices.capacity() * size_of::<u32>()
            + self.batches.capacity() * size_of::<Batch>()
            + self.wire_lines.capacity() * size_of::<LineVertex>()
            + self.detail_billboards.capacity() * size_of::<Billboard>()
            + self.detail_parts.capacity() * size_of::<Part>()
    }

    pub fn append_compiled_details(
        &mut self,
        details: &Self,
        region: Option<[DVec3; 2]>,
    ) -> Result<(), String> {
        let included = region.map(|region| {
            details
                .detail_parts
                .iter()
                .map(|part| part.intersects(region))
                .collect::<Vec<_>>()
        });
        let mut allowed_vertices = Vec::new();
        if let Some(included) = &included {
            allowed_vertices
                .try_reserve_exact(details.vertices.len())
                .map_err(|_| "Compiled detail filter allocation failed")?;
            allowed_vertices.resize(details.vertices.len(), false);
            let mut cursor = [0usize; 3];
            for (part, enabled) in details.detail_parts.iter().zip(included) {
                for (slot, range, maximum) in [
                    (0, &part.vertices, details.vertices.len()),
                    (1, &part.wires, details.wire_lines.len()),
                    (2, &part.billboards, details.detail_billboards.len()),
                ] {
                    if range.start != cursor[slot] || range.start > range.end || range.end > maximum
                    {
                        return Err("Compiled detail spatial ranges are inconsistent".into());
                    }
                    cursor[slot] = range.end;
                }
                if *enabled {
                    allowed_vertices[part.vertices.clone()].fill(true);
                }
            }
            if cursor
                != [
                    details.vertices.len(),
                    details.wire_lines.len(),
                    details.detail_billboards.len(),
                ]
            {
                return Err("Compiled detail spatial ranges are incomplete".into());
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baked_radiance_keeps_linear_range_and_updates_every_card_form_atomically() {
        let fs = GameFs {
            game_dir: Default::default(),
            base_dir: Default::default(),
            mounts: Vec::new(),
        };
        let mut materials = MaterialLibrary::flat(["detail".into()]);
        let mut mesh = model_scene(&fs, &mut materials, &[]);
        let sprite = DetailSprite {
            upper_left: [-4.0, 8.0],
            lower_right: [4.0, -8.0],
            texture_lower_right: [1.0; 2],
            ..Default::default()
        };
        let mut records = DetailProps::default();
        let mut baked = BakedLighting {
            ldr: Some(Vec::new()),
            hdr: Some(Vec::new()),
            base_channel: Some(crate::LightingChannel::Hdr),
        };
        for (kind, orientation) in [(1, 0), (1, 1), (1, 2), (2, 0), (3, 0)] {
            let ordinal = records.props.len();
            let prop = DetailProp {
                kind,
                orientation,
                lighting: RgbExp32 {
                    rgb: [255, 0, 0],
                    exponent: 1,
                },
                light_styles: (ordinal * 2) as u32,
                light_style_count: 2,
                ..Default::default()
            };
            let first = mesh.vertices.len();
            mesh.add_compiled_detail_card(0, &prop, sprite).unwrap();
            mesh.detail_parts.push(Part {
                ordinal,
                vertices: first..mesh.vertices.len(),
                wires: 0..0,
                billboards: 0..0,
                bounds: [DVec3::splat(-8.0), DVec3::splat(8.0)],
            });
            records.props.push(prop);
            for (table, colors) in [
                (&mut baked.ldr, [[60, 0, 0], [0; 3]]),
                (&mut baked.hdr, [[0; 3], [0, 0, 40]]),
            ] {
                table
                    .as_mut()
                    .unwrap()
                    .extend(colors.into_iter().enumerate().map(|(index, rgb)| {
                        DetailPropLighting {
                            style: index as u8 + 1,
                            lighting: RgbExp32 { rgb, exponent: 0 },
                        }
                    }));
            }
        }
        let original = mesh.vertices.clone();
        let settings = LightingSettings {
            mode: Lighting::BakedRadiance,
            channel: crate::LightingChannel::Hdr,
            exposure_ev: 1.0,
            style_gains: vec![
                StyleGain {
                    style: 1,
                    gain: 1.0,
                },
                StyleGain {
                    style: 2,
                    gain: 0.5,
                },
            ],
        };
        mesh.set_compiled_detail_lighting_options(&records, &baked, &settings)
            .unwrap();
        assert!(mesh.vertices.iter().all(|vertex| vertex.baked_irradiance
            == [4.0, 0.0, (40.0 / 255.0) as f32]
            && vertex.lighting_receiver == BAKED_RADIANCE_RECEIVER
            && vertex.color[..3] == [255; 3]));
        let mut base_only = settings.clone();
        base_only.style_gains.clear();
        mesh.set_compiled_detail_lighting_options(&records, &baked, &base_only)
            .unwrap();
        assert!(
            mesh.vertices
                .iter()
                .all(|vertex| vertex.baked_irradiance == [4.0, 0.0, 0.0])
        );
        mesh.set_compiled_detail_lighting_options(&records, &baked, &settings)
            .unwrap();
        let before = mesh.vertices.clone();
        let mut invalid = settings.clone();
        invalid.style_gains.push(invalid.style_gains[0]);
        assert!(
            mesh.set_compiled_detail_lighting_options(&records, &baked, &invalid)
                .is_err()
        );
        assert_eq!(mesh.vertices, before);
        records.props.last_mut().unwrap().lighting = RgbExp32 {
            rgb: [255; 3],
            exponent: 127,
        };
        invalid = settings.clone();
        invalid.exposure_ev = 8.0;
        assert!(
            mesh.set_compiled_detail_lighting_options(&records, &baked, &invalid)
                .unwrap_err()
                .contains("finite GPU range")
        );
        assert_eq!(mesh.vertices, before);
        records.props.last_mut().unwrap().light_styles = u32::MAX;
        assert!(
            mesh.set_compiled_detail_lighting_options(&records, &baked, &settings)
                .is_err()
        );
        assert_eq!(mesh.vertices, before);
        records.props.last_mut().unwrap().light_styles = 8;
        mesh.set_compiled_detail_lighting_options(&records, &baked, &LightingSettings::default())
            .unwrap();
        assert_eq!(mesh.vertices, original);
        baked.ldr = Some(Vec::new());
        baked.hdr = Some(Vec::new());
        for (ordinal, prop) in records.props.iter_mut().enumerate() {
            prop.lighting = RgbExp32 {
                rgb: if ordinal == 0 { [255, 0, 0] } else { [0; 3] },
                exponent: 1,
            };
            prop.light_styles = (ordinal * 3) as u32;
            prop.light_style_count = 3;
            for (table, colors) in [
                (&mut baked.ldr, [[0, 64, 0], [60, 0, 0], [0; 3]]),
                (&mut baked.hdr, [[0, 128, 0], [0; 3], [0, 0, 40]]),
            ] {
                table
                    .as_mut()
                    .unwrap()
                    .extend(colors.into_iter().enumerate().map(|(style, rgb)| {
                        DetailPropLighting {
                            style: style as u8,
                            lighting: RgbExp32 { rgb, exponent: 0 },
                        }
                    }));
            }
        }
        assert_eq!(baked.available_styles(&records), [1, 2]);
        for channel in [crate::LightingChannel::Ldr, crate::LightingChannel::Hdr] {
            let mut settings = settings.clone();
            settings.channel = channel;
            mesh.set_compiled_detail_lighting_options(&records, &baked, &settings)
                .unwrap();
            for part in &mesh.detail_parts {
                let shared_red = if part.ordinal == 0 { 4.0 } else { 0.0 };
                let expected = if channel == crate::LightingChannel::Ldr {
                    [shared_red + 120.0 / 255.0, 128.0 / 255.0, 0.0]
                } else {
                    [shared_red, 256.0 / 255.0, 40.0 / 255.0]
                }
                .map(|value| value as f32);
                assert!(
                    mesh.vertices[part.vertices.clone()]
                        .iter()
                        .all(|vertex| vertex.baked_irradiance == expected)
                );
            }
            settings.style_gains.clear();
            mesh.set_compiled_detail_lighting_options(&records, &baked, &settings)
                .unwrap();
            for part in &mesh.detail_parts {
                assert_eq!(
                    mesh.vertices[part.vertices.start].baked_irradiance[1],
                    if channel == crate::LightingChannel::Ldr {
                        (128.0 / 255.0) as f32
                    } else {
                        (256.0 / 255.0) as f32
                    }
                );
            }
        }
        let before = mesh.vertices.clone();
        let mut invalid = settings.clone();
        invalid.style_gains.push(StyleGain {
            style: 0,
            gain: 1.0,
        });
        assert!(
            mesh.set_compiled_detail_lighting_options(&records, &baked, &invalid)
                .is_err()
        );
        baked.hdr = None;
        assert!(
            mesh.set_compiled_detail_lighting_options(&records, &baked, &settings)
                .unwrap_err()
                .contains("absent")
        );
        assert_eq!(mesh.vertices, before);
    }

    #[test]
    fn detail_style_admission_checks_aggregate_layout_and_consumed_ranges_without_active_offset_guessing()
     {
        let records = DetailProps {
            props: vec![DetailProp {
                light_styles: u32::MAX,
                ..Default::default()
            }],
            ..Default::default()
        };
        let entry = DetailPropLighting {
            style: 1,
            lighting: RgbExp32::default(),
        };
        let half = source_formats::bsp::MAX_DETAIL_PROP_LIGHTING / 2;
        let mut baked = BakedLighting {
            ldr: Some(vec![entry; half]),
            hdr: Some(vec![entry; half]),
            base_channel: Some(crate::LightingChannel::Hdr),
        };
        baked.validate(&records).unwrap();
        baked.ldr.as_mut().unwrap().push(entry);
        assert!(baked.validate(&records).is_err());
        baked.ldr.as_mut().unwrap().pop();
        baked.hdr.as_mut().unwrap()[0].style = 2;
        assert!(baked.validate(&records).is_err());
        baked.hdr = None;
        baked.ldr = Some(vec![entry, entry]);
        let mut active = records.clone();
        active.props[0].light_styles = 0;
        active.props[0].light_style_count = 2;
        assert!(baked.validate(&active).is_err());
        active.props[0].light_styles = u32::MAX;
        assert!(baked.validate(&active).unwrap_err().contains("range"));
        baked.ldr.as_mut().unwrap()[0].style = 64;
        assert!(baked.validate(&records).unwrap_err().contains("style"));
        active.props[0].light_styles = 0;
        active.props[0].light_style_count = 64;
        active.props[0].lighting = RgbExp32::default();
        baked.ldr = Some(
            (0..=63)
                .map(|style| DetailPropLighting {
                    style,
                    lighting: RgbExp32 {
                        rgb: if style == 0 { [17, 34, 51] } else { [0; 3] },
                        exponent: 1,
                    },
                })
                .collect(),
        );
        baked.validate(&active).unwrap();
        assert_eq!(
            baked.available_styles(&active),
            (1..=63).collect::<Vec<_>>()
        );
        let settings = LightingSettings {
            mode: Lighting::BakedRadiance,
            ..Default::default()
        };
        assert_eq!(
            baked.colors(&active, &settings).unwrap(),
            vec![[34.0 / 255.0, 68.0 / 255.0, 102.0 / 255.0]]
        );
        active.props[0].light_style_count = 65;
        assert!(baked.validate(&active).is_err());
        let empty = BakedLighting {
            base_channel: Some(crate::LightingChannel::Hdr),
            ..Default::default()
        };
        assert!(
            empty
                .channel_issue(&records, crate::LightingChannel::Hdr)
                .is_none()
        );
        assert!(
            empty
                .channel_issue(&active, crate::LightingChannel::Hdr)
                .unwrap()
                .contains("absent")
        );
    }

    #[test]
    fn baked_ldr_uses_native_colors_once_for_every_card_form_and_refuses_bad_ranges_atomically() {
        let fs = GameFs {
            game_dir: Default::default(),
            base_dir: Default::default(),
            mounts: Vec::new(),
        };
        let mut materials = MaterialLibrary::flat(["detail".into()]);
        let mut mesh = model_scene(&fs, &mut materials, &[]);
        let sprite = DetailSprite {
            upper_left: [-4.0, 8.0],
            lower_right: [4.0, -8.0],
            texture_upper_left: [0.0; 2],
            texture_lower_right: [1.0; 2],
        };
        let cases = [
            (1, 0, [64, 128, 255], -1, [32, 64, 128]),
            (2, 0, [255, 64, 0], 1, [255, 128, 0]),
            (3, 0, [0, 0, 0], 127, [0, 0, 0]),
            (1, 1, [1, 2, 3], 127, [255, 255, 255]),
            (1, 2, [255, 128, 64], -128, [0, 0, 0]),
        ];
        let mut records = DetailProps {
            sprites: vec![sprite],
            props: cases
                .iter()
                .map(|&(kind, orientation, rgb, exponent, _)| DetailProp {
                    kind,
                    orientation,
                    lighting: RgbExp32 { rgb, exponent },
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        for ordinal in [4, 0, 3, 1, 2] {
            let first = mesh.vertices.len();
            mesh.add_compiled_detail_card(0, &records.props[ordinal], sprite)
                .unwrap();
            mesh.detail_parts.push(Part {
                ordinal,
                vertices: first..mesh.vertices.len(),
                wires: 0..0,
                billboards: 0..0,
                bounds: [DVec3::splat(-8.0), DVec3::splat(8.0)],
            });
        }
        let original = mesh.vertices.clone();
        let bounds = (mesh.mins, mesh.maxs);
        let wires = mesh.wire_lines.clone();
        mesh.set_compiled_detail_lighting(&records, Lighting::BakedLdr)
            .unwrap();
        for part in &mesh.detail_parts {
            for index in part.vertices.clone() {
                let mut expected = original[index];
                expected.color[..3].copy_from_slice(&cases[part.ordinal].4);
                expected.lighting_receiver = BAKED_LIGHTING_RECEIVER;
                assert_eq!(mesh.vertices[index], expected);
            }
        }
        assert_eq!((mesh.mins, mesh.maxs), bounds);
        assert_eq!(mesh.wire_lines, wires);
        let colored = mesh.vertices.clone();
        let mut baked = BakedLighting {
            ldr: Some(Vec::new()),
            ..Default::default()
        };
        for (ordinal, prop) in records.props.iter_mut().enumerate() {
            let mut extra = prop.lighting;
            if ordinal == 0 {
                prop.lighting.rgb = prop.lighting.rgb.map(|value| value / 2);
                for (value, shared) in extra.rgb.iter_mut().zip(prop.lighting.rgb) {
                    *value -= shared;
                }
            } else {
                prop.lighting = RgbExp32::default();
            }
            prop.light_styles = ordinal as u32;
            prop.light_style_count = 1;
            baked.ldr.as_mut().unwrap().push(DetailPropLighting {
                lighting: extra,
                style: 0,
            });
        }
        let settings = LightingSettings {
            mode: Lighting::BakedLdr,
            ..Default::default()
        };
        mesh.set_compiled_detail_lighting_options(&records, &baked, &settings)
            .unwrap();
        assert_eq!(mesh.vertices, colored);
        baked.ldr.as_mut().unwrap()[2].style = 32;
        assert!(
            mesh.set_compiled_detail_lighting_options(&records, &baked, &settings)
                .unwrap_err()
                .contains("runtime light styles")
        );
        assert_eq!(mesh.vertices, colored);
        records
            .props
            .iter_mut()
            .for_each(|prop| prop.light_style_count = 0);
        mesh.detail_parts[1].ordinal = mesh.detail_parts[0].ordinal;
        assert!(
            mesh.set_compiled_detail_lighting(&records, Lighting::EditorShading)
                .is_err()
        );
        assert_eq!(mesh.vertices, colored);
        mesh.detail_parts[1].ordinal = 0;
        mesh.set_compiled_detail_lighting(&records, Lighting::EditorShading)
            .unwrap();
        assert_eq!(mesh.vertices, original);
    }

    #[test]
    fn literal_card_cross_and_tri_rest_coordinates_follow_native_records() {
        let sprite = DetailSprite {
            upper_left: [-4.0, 16.0],
            lower_right: [12.0, -16.0],
            ..Default::default()
        };
        let mut prop = DetailProp {
            kind: 1,
            origin: [10.0, 20.0, 30.0],
            scale: 2.0,
            ..Default::default()
        };
        let cards = rest_quads(&prop, sprite).unwrap();
        assert_eq!(
            cards[0],
            [
                DVec3::new(10.0, 28.0, 62.0),
                DVec3::new(10.0, 28.0, -2.0),
                DVec3::new(10.0, -4.0, -2.0),
                DVec3::new(10.0, -4.0, 62.0)
            ]
        );
        prop.kind = 2;
        assert_eq!(
            rest_quads(&prop, sprite).unwrap()[0][3],
            DVec3::new(-6.0, 20.0, 62.0)
        );
        prop.kind = 3;
        prop.shape_size = 255;
        let triangles = rest_quads(&prop, sprite).unwrap();
        assert_eq!(triangles.len(), 3);
        assert_eq!(triangles[0][0], DVec3::new(42.0, 28.0, 62.0));
        prop.orientation = 2;
        assert!(rest_quads(&prop, sprite).is_err());
        prop.kind = 1;
        assert!(rest_quads(&prop, sprite).is_err());
        assert!(Billboard::new(&prop, sprite).is_ok());
        prop.scale = f32::NAN;
        assert!(rest_quads(&prop, sprite).is_err());
    }
}

#[cfg(test)]
mod billboard_tests {
    use super::*;

    #[test]
    fn source_eye_facing_basis_preserves_anchor_vertical_mode_and_singular_views() {
        let sprite = DetailSprite {
            upper_left: [-2.0, 3.0],
            lower_right: [2.0, -1.0],
            ..Default::default()
        };
        let mut prop = DetailProp {
            kind: 1,
            orientation: 1,
            angles: [30.0, 80.0, 10.0],
            ..Default::default()
        };
        let full = Billboard::new(&prop, sprite).unwrap();
        let q = full.quad(Vec3::new(10.0, 0.0, 10.0));
        let diagonal = std::f32::consts::FRAC_1_SQRT_2 * 3.0;
        assert!((q[0] - Vec3::new(-diagonal, 2.0, diagonal)).length() < 1e-5);
        assert_eq!(full.quad(Vec3::Z)[0], Vec3::new(-3.0, 2.0, 0.0));
        assert_eq!(full.quad(Vec3::ZERO)[0], Vec3::new(3.0, 2.0, 0.0));
        prop.orientation = 2;
        let vertical = Billboard::new(&prop, sprite).unwrap();
        assert_eq!(
            vertical.quad(Vec3::new(10.0, 0.0, 10.0))[0],
            Vec3::new(0.0, 2.0, 3.0)
        );
        assert_eq!(vertical.quad(Vec3::Y)[0], Vec3::new(-2.0, 0.0, 3.0));
        assert_eq!(vertical.quad(Vec3::Z)[0], Vec3::new(3.0, 2.0, 0.0));
    }
}
