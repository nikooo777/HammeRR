use serde::Serialize;

pub const MAX_LIGHTS: usize = 32;
pub const MAX_DIAGNOSTICS: usize = 128;
#[derive(Serialize)]
pub struct Limits {
    pub lights: usize,
    pub diagnostics: usize,
    pub exposure_ev: [i32; 2],
    pub receiver_counts: &'static str,
    pub policy: &'static str,
}

pub fn limits() -> Limits {
    Limits {
        lights: MAX_LIGHTS,
        diagnostics: MAX_DIAGNOSTICS,
        exposure_ev: [-8, 8],
        receiver_counts: RECEIVER_COUNTS,
        policy: POLICY,
    }
}

pub const RECEIVER_COUNTS: &str = "flat brush polygons counted separately from admitted/excluded terrain surfaces and drawn triangles; outline-only terrain has zero drawn triangles; separate authored model instance/part/drawn-triangle counts; resource guides and helpers excluded by category";

pub const POLICY: &str = "Unshadowed authored LDR point/spot diffuse preview. At most 32 constant unnamed style-0 lights; this is an editor budget, not an engine limit. No sun, ambient, bounce, shadows, animation, native brightness or tone mapping. Opaque flat LightmappedGeneric brush polygons and single-base world/func_detail terrain with checked sewn LOD normals; terrain excludes bump/normal, blend and detail declarations. Authored selected MDL meshes require whole-instance opaque single-base VertexLitGeneric admission, Normal render mode and checked transformed normals; bump/normal, proxies and unsupported material declarations are excluded; excluded surfaces retain their ordinary display. Fast interpolates vertex lighting and can miss small pools on large faces; Detail evaluates each fragment and composes with supported normal maps. Linear base × tint × compiler contribution / 255, exposure 2^EV, per-channel Reinhard, then one sRGB target conversion. Zero admitted lights make supported receivers black. Reload material assets refreshes cached base display and receiver policy together.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Fast,
    #[default]
    Detail,
}

impl Quality {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "fast" => Ok(Self::Fast),
            "detail" => Ok(Self::Detail),
            _ => Err("Lighting quality must be fast or detail".into()),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Detail => "detail",
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Light {
    pub origin_radius: [f32; 4],
    pub intensity_cap: [f32; 4],
    pub coefficients_spot: [f32; 4],
    pub direction_inner: [f32; 4],
    pub outer_exponent_fade: [f32; 4],
}

#[derive(Clone, Debug, Serialize)]
pub struct Diagnostic {
    pub entity: Option<u32>,
    pub material: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub source: Option<Source>,
    pub requested: bool,
    pub active: bool,
    pub admitted_lights: usize,
    pub excluded_lights: usize,
    pub admitted_entities: Vec<u32>,
    pub admitted_receivers: usize,
    pub excluded_receivers: usize,
    pub terrain: TerrainReceivers,
    pub models: ModelReceivers,
    pub diagnostics: Vec<Diagnostic>,
    pub diagnostics_truncated: bool,
    pub unavailable: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct TerrainReceivers {
    pub admitted_surfaces: usize,
    pub excluded_surfaces: usize,
    pub admitted_triangles: usize,
    pub excluded_triangles: usize,
}

impl TerrainReceivers {
    pub fn exclude_admitted(&mut self) {
        self.excluded_surfaces += self.admitted_surfaces;
        self.excluded_triangles += self.admitted_triangles;
        self.admitted_surfaces = 0;
        self.admitted_triangles = 0;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ModelReceivers {
    pub admitted_instances: usize,
    pub excluded_instances: usize,
    pub admitted_parts: usize,
    pub excluded_parts: usize,
    pub admitted_triangles: usize,
    pub excluded_triangles: usize,
}

impl ModelReceivers {
    pub fn exclude_admitted(&mut self) {
        self.excluded_instances += self.admitted_instances;
        self.excluded_parts += self.admitted_parts;
        self.excluded_triangles += self.admitted_triangles;
        self.admitted_instances = 0;
        self.admitted_parts = 0;
        self.admitted_triangles = 0;
    }
}

pub(crate) fn triangle_error(
    points: [glam::Vec3; 3],
    normals: [glam::Vec3; 3],
) -> Option<&'static str> {
    if points.iter().any(|point| {
        !point.is_finite()
            || point.abs().max_element() > map_core::checks::SOURCE_COORDINATE_LIMIT as f32
    }) {
        return Some("Authored-light receivers must fit finite Source compile coordinates");
    }
    if normals.iter().any(|normal| {
        let squared = normal.length_squared();
        !normal.is_finite() || !squared.is_finite() || squared < 1e-12
    }) {
        return Some("Authored lighting needs finite nonzero packed normals");
    }
    let [a, b, c] = points;
    let Some(geometric) = (c - a).cross(b - a).try_normalize() else {
        return Some("Authored lighting excludes degenerate packed triangles");
    };
    if normals
        .iter()
        .any(|normal| normal.normalize().dot(geometric) <= 1e-4)
    {
        return Some("Authored lighting needs a safe common normal hemisphere on every triangle");
    }
    None
}

#[derive(Clone, Debug, Serialize)]
pub struct Source {
    pub document_generation: u64,
    pub revision: u64,
    pub render_revision: u64,
    pub scene_serial: u64,
    pub working_region_stamp: String,
    pub material_generation: u64,
}

impl Report {
    pub fn diagnostic(&mut self, entity: Option<u32>, material: Option<&str>, reason: &str) {
        if self.diagnostics.len() >= MAX_DIAGNOSTICS {
            self.diagnostics_truncated = true;
            return;
        }
        self.diagnostics.push(Diagnostic {
            entity,
            material: material.map(|name| name.chars().take(256).collect()),
            reason: reason.chars().take(512).collect(),
        });
    }
    fn terrain_summary(&self) -> String {
        format!(
            "{} admitted / {} excluded terrain surfaces ({} admitted / {} excluded drawn triangles)",
            self.terrain.admitted_surfaces,
            self.terrain.excluded_surfaces,
            self.terrain.admitted_triangles,
            self.terrain.excluded_triangles
        )
    }
    fn model_summary(&self) -> String {
        format!(
            "Model receivers: {} supported / {} excluded instances (selected parts: {} supported / {} excluded; drawn triangles: {} supported / {} excluded)",
            self.models.admitted_instances,
            self.models.excluded_instances,
            self.models.admitted_parts,
            self.models.excluded_parts,
            self.models.admitted_triangles,
            self.models.excluded_triangles
        )
    }
    pub fn summary(&self) -> String {
        if !self.requested {
            return "Authored lighting preview is off".into();
        }
        if let Some(reason) = &self.unavailable {
            return format!(
                "Unshadowed lighting unavailable: {reason}\n{} admitted / {} excluded lights · {} admitted / {} excluded brush receiver polygons · {} · {}; ordinary display retained",
                self.admitted_lights,
                self.excluded_lights,
                self.admitted_receivers,
                self.excluded_receivers,
                self.terrain_summary(),
                self.model_summary()
            );
        }
        if !self.active {
            return "Unshadowed lighting requires Textured view; no light admission".into();
        }
        let mut text = format!(
            "Unshadowed lighting · {} admitted / {} excluded lights · {} admitted / {} excluded brush receiver polygons · {} · {}",
            self.admitted_lights,
            self.excluded_lights,
            self.admitted_receivers,
            self.excluded_receivers,
            self.terrain_summary(),
            self.model_summary()
        );
        if self.admitted_lights == 0 {
            text.push_str("\nZero admitted lights: supported receivers are black");
        }
        if self.excluded_lights > 0
            || self.excluded_receivers > 0
            || self.terrain.excluded_surfaces > 0
            || self.models.excluded_instances > 0
        {
            text.push_str(
                "\nPartial scene coverage; excluded receivers keep their ordinary display",
            );
        }
        for diagnostic in self.diagnostics.iter().take(2) {
            text.push_str(&format!("\n{}", diagnostic.reason));
        }
        text
    }
}

#[derive(Clone, Debug, Default)]
pub struct Preview {
    pub lights: Vec<Light>,
    pub report: Report,
}
