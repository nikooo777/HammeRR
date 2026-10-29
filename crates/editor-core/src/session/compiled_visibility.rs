use std::{
    io::{self, Write},
    sync::Arc,
};

use compiler::{
    Control,
    visibility_inspection::{MAX_LEAVES, MAX_PORTAL_POINTS, Snapshot},
};
use glam::DVec3;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use source_formats::bsp::{
    Bsp, Leaf, MAX_WORLD_LIGHTS, VisibilityKind, WorldLight, lump, world_light_kind,
};

pub use super::compiled_world_lights::Channel;
use super::{Actor, Session, baked_build};

pub const MAX_PAGE_ROWS: usize = 64;
pub const MAX_REPORT_BYTES: usize = 512 * 1024;
pub const MAX_GUIDE_LEAVES: usize = 512;
pub const MAX_GUIDE_BYTES: usize = 256 * 1024;
const LINE_VERTICES_PER_BOX: usize = 24;
const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;
const MAX_CALL_BYTES: usize = 384 * 1024 * 1024;
const SOURCE_MODE: baked_build::SourceVerification =
    baked_build::SourceVerification::OriginalMetadata {
        maximum_bytes: MAX_SOURCE_BYTES,
    };
const NOTES: &str = "Compiler-owned world leaves and final clusters. Leaf boxes are native bounds, not exact convex cells. Portal rings retain compiler binary64 coordinates and build-local original slots, including boundaries inside coarsened clusters; they are not authored faces or BSP render surfaces. PVS/PAS decode separate native visibility rows. World-light targets use the actual stored source cluster in the selected native channel, without position lookup. This is compiler potential visibility, not native light selection, falloff, ray visibility or exact finite-area emission. Historical ordinals belong only to their captured build. Freshness covers the committed map, settings/content epoch and retained source guards, not every world material. Unapplied drafts are not compiled. Framing and guides require a current owned build. Destination guides show the complete potential PVS/PAS set as native world-leaf bounds, not current game occlusion or rendered cells.";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Summary {},
    Leaf { ordinal: usize },
    Cluster { ordinal: usize },
    Portal { ordinal: usize },
    Position { source: PositionSource },
    WorldLight { channel: Channel, ordinal: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PositionSource {
    Position { position: [f64; 3] },
    Camera {},
}

#[derive(Debug, Clone, Serialize)]
pub struct PositionResult {
    pub source: PositionSource,
    pub requested_position: [f64; 3],
    pub effective_position: [f32; 3],
    pub native_leaf: usize,
    pub cluster: i16,
    pub contents: i32,
    pub retained_world_leaf: bool,
    pub classification: &'static str,
    pub nodes_visited: usize,
    pub numeric_policy: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct WorldLightResult {
    pub channel: Channel,
    pub ordinal: usize,
    pub cluster: i32,
}

impl Default for Target {
    fn default() -> Self {
        Self::Summary {}
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Pvs,
    Pas,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GuideMode {
    Selected {},
    Destinations { visibility: Kind },
}

impl Default for GuideMode {
    fn default() -> Self {
        Self::Selected {}
    }
}

#[derive(Debug, Clone)]
pub struct Query {
    pub build_token: Option<String>,
    pub target: Target,
    pub kind: Kind,
    pub offset: usize,
    pub visibility_offset: usize,
    pub limit: usize,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            build_token: None,
            target: Target::Summary {},
            kind: Kind::Pvs,
            offset: 0,
            visibility_offset: 0,
            limit: MAX_PAGE_ROWS,
        }
    }
}

impl Query {
    fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_PAGE_ROWS).contains(&self.limit) {
            return Err("Visibility page limit must be 1–64".into());
        }
        self.offset
            .checked_add(self.limit)
            .ok_or("Visibility page overflow")?;
        self.visibility_offset
            .checked_add(self.limit)
            .ok_or("Visibility row page overflow")?;
        if let Some(token) = &self.build_token {
            if token.is_empty()
                || token.len() > 20
                || !token.bytes().all(|byte| byte.is_ascii_digit())
                || token.starts_with('0')
                || token.parse::<u64>().is_err()
            {
                return Err("Build token must be the returned opaque decimal string".into());
            }
        } else if !matches!(self.target, Target::Summary {})
            || self.offset != 0
            || self.visibility_offset != 0
        {
            return Err(
                "Explicit visibility targets and retained pages require the returned build token"
                    .into(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Page<T> {
    pub offset: usize,
    pub limit: usize,
    pub returned: usize,
    pub total: Option<usize>,
    pub next_offset: Option<usize>,
    pub rows: Vec<T>,
}

impl<T> Page<T> {
    fn empty(offset: usize, limit: usize) -> Self {
        Self {
            offset,
            limit,
            returned: 0,
            total: None,
            next_offset: None,
            rows: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub world_leaves: usize,
    pub clusters: usize,
    pub portals: usize,
    pub excluded_portals: usize,
    pub metadata_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct LeafRow {
    pub ordinal: usize,
    pub cluster: i16,
    pub contents: i32,
    pub bounds: [[f64; 3]; 2],
}

#[derive(Debug, Clone, Serialize)]
pub struct PortalRow {
    pub ordinal: usize,
    pub leaves: [u32; 2],
    pub clusters: [i16; 2],
    pub internal_cluster: bool,
    pub vertices: usize,
    pub bounds: [[f64; 3]; 2],
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Row {
    Leaf(LeafRow),
    Portal(PortalRow),
    Cluster { ordinal: usize, leaves: usize },
}

impl Row {
    pub fn target(&self) -> Target {
        match self {
            Self::Leaf(row) => Target::Leaf {
                ordinal: row.ordinal,
            },
            Self::Portal(row) => Target::Portal {
                ordinal: row.ordinal,
            },
            Self::Cluster { ordinal, .. } => Target::Cluster { ordinal: *ordinal },
        }
    }
    fn bounds(&self) -> Option<[[f64; 3]; 2]> {
        match self {
            Self::Leaf(row) => Some(row.bounds),
            Self::Portal(row) => Some(row.bounds),
            Self::Cluster { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Inspection {
    pub build: Value,
    pub historical: bool,
    pub target: Target,
    pub kind: Kind,
    pub summary: Option<Summary>,
    pub selection: Option<Row>,
    pub selected_vertices: Option<Vec<[f64; 3]>>,
    pub position: Option<PositionResult>,
    pub world_light: Option<WorldLightResult>,
    pub page: Page<Row>,
    pub visibility: Page<usize>,
    pub diagnostics: Vec<String>,
    pub overlay: Value,
    pub limits: Value,
    pub notes: &'static str,
}

impl Inspection {
    fn require_current(&self) -> Result<(), String> {
        if self.build["current"] != true || self.build["status"] != "current" {
            return Err("Framing and guides require a current successful owned build".into());
        }
        Ok(())
    }

    pub fn current_bounds(&self) -> Result<[[f64; 3]; 2], String> {
        self.require_current()?;
        self.selection
            .as_ref()
            .and_then(Row::bounds)
            .ok_or_else(|| "Select a compiled leaf or portal".into())
    }
}

#[derive(Default)]
pub(super) struct State {
    guide: Option<Guide>,
    displayed: bool,
}

struct Guide {
    token: u64,
    target: Target,
    mode: GuideMode,
    geometry: GuideGeometry,
    snapshot: Arc<Snapshot>,
    world_light: Option<WorldLightResult>,
}

enum GuideGeometry {
    Selected([[f64; 3]; 2]),
    Destinations(DestinationGuide),
}

#[derive(Debug)]
struct GuideBox {
    ordinal: usize,
    bounds: [[f64; 3]; 2],
}

#[derive(Debug)]
struct DestinationGuide {
    source_cluster: usize,
    clusters: usize,
    boxes: Vec<GuideBox>,
    retained_bytes: usize,
    line_vertices: usize,
}

impl Session {
    pub fn inspect_compiled_visibility(&self, query: &Query) -> Result<Inspection, String> {
        self.inspect_compiled_visibility_controlled(query, &Control::default())
    }

    pub fn inspect_compiled_visibility_controlled(
        &self,
        query: &Query,
        control: &Control,
    ) -> Result<Inspection, String> {
        self.inspect_visibility_prepared(query, control, false)
            .map(|(report, _)| report)
    }

    fn inspect_visibility_prepared(
        &self,
        query: &Query,
        control: &Control,
        prepare_destinations: bool,
    ) -> Result<(Inspection, Option<DestinationGuide>), String> {
        control.checkpoint()?;
        query.validate()?;
        match (&self.compiled, &query.build_token) {
            (None, Some(_)) => {
                return Err("The requested successful build is no longer retained".into());
            }
            (Some(build), Some(token)) if token != &build.lightmap_identity.token.to_string() => {
                return Err(
                    "Build token differs from the retained successful artifact; refresh inspection"
                        .into(),
                );
            }
            _ => {}
        }
        let mut state = baked_build::build_state_controlled(self, SOURCE_MODE, control)?;
        if self.compiled.is_none() {
            state["diagnostics"] = json!([
                "Compile the map successfully in this session before inspecting visibility"
            ]);
        }
        if state["status"] == "unavailable" {
            state["token"] = Value::Null;
        }
        if let Some(build) = &self.compiled {
            state["visibility"] = json!(build.options.visibility.name());
        }
        let mut report = Inspection {
            build: state,
            historical: false,
            target: query.target,
            kind: query.kind,
            summary: None,
            selection: None,
            selected_vertices: None,
            position: None,
            world_light: None,
            page: Page::empty(query.offset, query.limit),
            visibility: Page::empty(query.visibility_offset, query.limit),
            diagnostics: Vec::new(),
            overlay: self.compiled_visibility_overlay(),
            limits: json!({"bsp_bytes":baked_build::MAX_BSP_BYTES,"call_bytes":MAX_CALL_BYTES,"metadata_bytes":compiler::visibility_inspection::MAX_BYTES,"report_bytes":MAX_REPORT_BYTES,"page_rows":MAX_PAGE_ROWS,"portal_vertices":MAX_PORTAL_POINTS,"point_lookup_records":source_formats::bsp::MAX_POINT_LEAF_RECORDS,"point_lookup_steps":source_formats::bsp::MAX_POINT_LEAF_RECORDS + 1,"guide_leaves":MAX_GUIDE_LEAVES,"guide_bytes":MAX_GUIDE_BYTES,"guide_line_vertices":MAX_GUIDE_LEAVES * LINE_VERTICES_PER_BOX}),
            notes: NOTES,
        };
        let Some(build) = &self.compiled else {
            return finish(report, control).map(|report| (report, None));
        };
        if report.build["status"] == "unavailable" {
            return finish(report, control).map(|report| (report, None));
        }
        let metadata = match &build.visibility_inspection {
            Ok(metadata) => metadata,
            Err(error) => {
                unavailable(&mut report, error);
                return finish(report, control).map(|report| (report, None));
            }
        };
        let identity = build
            .lightmap_identity
            .output
            .as_ref()
            .map_err(|error| baked_build::bounded_message(error))?;
        let retained_guide_bytes =
            self.compiled_visibility
                .guide
                .as_ref()
                .map_or(Ok(0), |guide| {
                    let snapshot_bytes = if Arc::ptr_eq(&guide.snapshot, metadata) {
                        0
                    } else {
                        guide.snapshot.bytes()
                    };
                    let geometry_bytes = match &guide.geometry {
                        GuideGeometry::Selected(_) => 0,
                        GuideGeometry::Destinations(destinations) => destinations.retained_bytes,
                    };
                    snapshot_bytes
                        .checked_add(geometry_bytes)
                        .and_then(|bytes| bytes.checked_add(size_of::<Guide>()))
                        .ok_or("Retained visibility guide storage overflow")
                })?;
        let decoded = read_native(identity, metadata, retained_guide_bytes, control);
        let (bsp, leaves) = match decoded {
            Ok(decoded) => decoded,
            Err(error) => {
                control.checkpoint()?;
                unavailable(&mut report, &error);
                return finish(report, control).map(|report| (report, None));
            }
        };
        if let Some(error) = baked_build::source_issue_with_mode(build, SOURCE_MODE, control) {
            control.checkpoint()?;
            stale(&mut report, &error);
        }
        let mut resolved = query.clone();
        match query.target {
            Target::Position { source } => {
                let position =
                    resolve_position(&bsp, &leaves, metadata, source, self.camera, control)?;
                if position.retained_world_leaf {
                    resolved.target = Target::Leaf {
                        ordinal: position.native_leaf,
                    };
                    report.target = resolved.target;
                }
                report.position = Some(position);
            }
            Target::WorldLight { channel, ordinal } => {
                let light = resolve_world_light(
                    &bsp,
                    metadata,
                    build.options.lighting,
                    channel,
                    ordinal,
                    control,
                )?;
                resolved.target = Target::Cluster {
                    ordinal: light.cluster as usize,
                };
                report.world_light = Some(light);
            }
            _ => {}
        }
        let destinations = fill(
            &mut report,
            &resolved,
            metadata,
            &bsp,
            &leaves,
            control,
            prepare_destinations,
        )?;
        drop(leaves);
        drop(bsp);
        if let Some(error) = baked_build::source_issue_with_mode(build, SOURCE_MODE, control) {
            control.checkpoint()?;
            stale(&mut report, &error);
        }
        if let Err(error) = identity.verify(control) {
            control.checkpoint()?;
            unavailable(
                &mut report,
                &format!("Compiled BSP changed during visibility inspection: {error}"),
            );
        }
        let observed = baked_build::recheck_with_mode(build, SOURCE_MODE, control);
        for error in observed.stale {
            stale(&mut report, &error);
        }
        if let Some(error) = observed.unavailable {
            unavailable(&mut report, &error);
        }
        finish(report, control).map(|report| (report, destinations))
    }

    pub fn compiled_visibility_overlay(&self) -> Value {
        let Some(guide) = &self.compiled_visibility.guide else {
            return json!({"requested":false,"displayed":false});
        };
        let control = Control::default();
        let state = baked_build::build_state_controlled(self, SOURCE_MODE, &control);
        let current = self
            .compiled
            .as_ref()
            .is_some_and(|build| build.lightmap_identity.token == guide.token)
            && state.as_ref().is_ok_and(|state| state["current"] == true);
        let mut state = json!({"requested":true,"displayed":current,"build_token":guide.token.to_string(),"target":guide.target,"mode":guide.mode,"guide_leaf_cap":MAX_GUIDE_LEAVES,"guide_byte_cap":MAX_GUIDE_BYTES,"reason":if current {Value::Null} else {json!("The retained guide is suppressed until its owned build is current")}});
        if let Some(light) = guide.world_light {
            state["world_light"] = json!(light);
        }
        if let GuideGeometry::Destinations(destinations) = &guide.geometry {
            state["source_cluster"] = json!(destinations.source_cluster);
            state["destination_clusters"] = json!(destinations.clusters);
            state["leaf_count"] = json!(destinations.boxes.len());
            state["native_leaves"] = json!(
                destinations
                    .boxes
                    .iter()
                    .map(|leaf| leaf.ordinal)
                    .collect::<Vec<_>>()
            );
            state["retained_bytes"] = json!(destinations.retained_bytes);
            state["line_vertices"] = json!(destinations.line_vertices);
        }
        state
    }

    pub fn set_compiled_visibility_overlay(
        &mut self,
        actor: Actor,
        build_token: Option<String>,
        target: Option<Target>,
        mode: GuideMode,
    ) -> Result<Value, String> {
        self.set_compiled_visibility_overlay_controlled(
            actor,
            build_token,
            target,
            mode,
            &Control::default(),
        )
    }

    pub fn set_compiled_visibility_overlay_controlled(
        &mut self,
        actor: Actor,
        build_token: Option<String>,
        target: Option<Target>,
        mode: GuideMode,
        control: &Control,
    ) -> Result<Value, String> {
        control.checkpoint()?;
        self.ensure_not_closing()?;
        let Some(target) = target else {
            if !matches!(mode, GuideMode::Selected {}) {
                return Err("Destination bounds require an explicit source leaf or cluster and build token; previous guide is unchanged".into());
            }
            self.compiled_visibility.guide = None;
            self.compiled_visibility.displayed = false;
            self.scene_serial += 1;
            self.log(actor, "Cleared compiled visibility guide");
            return Ok(self.compiled_visibility_overlay());
        };
        self.visibility_view_ready()?;
        let (kind, prepare_destinations) = match mode {
            GuideMode::Selected {} => {
                if !matches!(target, Target::Leaf { .. } | Target::Portal { .. }) {
                    return Err("Choose a compiled leaf or portal guide".into());
                }
                (Kind::Pvs, false)
            }
            GuideMode::Destinations { visibility } => {
                if !matches!(
                    target,
                    Target::Leaf { .. } | Target::Cluster { .. } | Target::WorldLight { .. }
                ) {
                    return Err("Choose a compiled leaf, cluster or world light with a native PVS/PAS source; resolve camera/position first".into());
                }
                (visibility, true)
            }
        };
        let build_token =
            build_token.ok_or("A compiled visibility guide requires the returned build token")?;
        let (report, destinations) = self.inspect_visibility_prepared(
            &Query {
                build_token: Some(build_token),
                target,
                kind,
                ..Query::default()
            },
            control,
            prepare_destinations,
        )?;
        report.require_current()?;
        let geometry = if prepare_destinations {
            GuideGeometry::Destinations(destinations.ok_or(
                "This native leaf has no PVS/PAS source cluster; previous guide is unchanged",
            )?)
        } else {
            GuideGeometry::Selected(report.current_bounds()?)
        };
        let build = self.compiled.as_ref().ok_or("No owned build remains")?;
        let metadata = build
            .visibility_inspection
            .as_ref()
            .map_err(|error| baked_build::bounded_message(error))?;
        control.checkpoint()?;
        self.compiled_visibility.guide = Some(Guide {
            token: build.lightmap_identity.token,
            target,
            mode,
            geometry,
            snapshot: Arc::clone(metadata),
            world_light: report.world_light,
        });
        self.compiled_visibility.displayed = true;
        self.scene_serial += 1;
        self.log(actor, "Show compiled visibility guide");
        Ok(self.compiled_visibility_overlay())
    }

    pub fn frame_compiled_visibility(
        &mut self,
        actor: Actor,
        build_token: String,
        target: Target,
        aspect: f32,
    ) -> Result<Inspection, String> {
        self.ensure_not_closing()?;
        self.visibility_view_ready()?;
        if !matches!(target, Target::Leaf { .. } | Target::Portal { .. }) {
            return Err("Choose compiled leaf bounds or a portal to frame".into());
        }
        super::inspection_framing::validate_aspect(aspect)?;
        let report = self.inspect_compiled_visibility(&Query {
            build_token: Some(build_token),
            target,
            ..Query::default()
        })?;
        let [mins, maxs] = report.current_bounds()?.map(DVec3::from_array);
        let camera = self.inspection_frame_camera(
            mins.as_vec3(),
            maxs.as_vec3(),
            aspect,
            "Compiled visibility",
        )?;
        self.set_camera(camera);
        self.log(actor, "Frame compiled visibility bounds");
        Ok(report)
    }

    fn visibility_view_ready(&self) -> Result<(), String> {
        self.inspection_viewport_ready("framing or showing compiled visibility")
    }

    pub fn preflight_compiled_visibility(&mut self) {
        let displayed = self.compiled_visibility.guide.is_some()
            && self.compiled_visibility_overlay()["displayed"] == true;
        if displayed != self.compiled_visibility.displayed {
            self.compiled_visibility.displayed = displayed;
            self.scene_serial += 1;
        }
    }

    pub fn compiled_visibility_observation(&self) -> Value {
        let mut observation = self.compiled_world_light_observation();
        observation["visibility_overlay"] = self.compiled_visibility_overlay();
        observation
    }

    pub(super) fn append_compiled_visibility_guide(&self, mesh: &mut render::SceneMesh) {
        if self.compiled_visibility.guide.is_none()
            || self.compiled_visibility_overlay()["displayed"] != true
        {
            return;
        }
        let guide = self.compiled_visibility.guide.as_ref().unwrap();
        let color = [70, 220, 255, 255];
        if let GuideGeometry::Destinations(destinations) = &guide.geometry {
            mesh.lines.reserve_exact(destinations.line_vertices);
            for leaf in &destinations.boxes {
                let [mins, maxs] = leaf.bounds.map(DVec3::from_array);
                mesh.add_bounds_lines(mins, maxs, color);
            }
            return;
        }
        match guide.target {
            Target::Leaf { .. } => {
                if let GuideGeometry::Selected(bounds) = &guide.geometry {
                    let [mins, maxs] = (*bounds).map(DVec3::from_array);
                    mesh.add_bounds_lines(mins, maxs, color);
                }
            }
            Target::Portal { ordinal } => {
                if let Some(portal) = guide.snapshot.portal(ordinal) {
                    for index in 0..portal.points().len() {
                        for point in [
                            portal.points()[index],
                            portal.points()[(index + 1) % portal.points().len()],
                        ] {
                            mesh.lines.push(render::scene::LineVertex {
                                position: point.as_vec3().to_array(),
                                color,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn read_native(
    identity: &compiler::FileIdentity,
    metadata: &Snapshot,
    retained_guide_bytes: usize,
    control: &Control,
) -> Result<(Bsp, Vec<Leaf>), String> {
    let bytes = baked_build::read_owned_bytes_controlled(identity, control)?;
    let decoded = baked_build::decoded_bsp_bound(&bytes)?;
    let live = decoded
        .checked_add(bytes.len())
        .and_then(|bytes| bytes.checked_add(metadata.bytes()))
        .and_then(|bytes| {
            bytes
                .checked_add(
                    MAX_SOURCE_BYTES
                        + MAX_LEAVES * 56
                        + 4 * MAX_REPORT_BYTES
                        + MAX_GUIDE_BYTES
                        + MAX_WORLD_LIGHTS * size_of::<WorldLight>(),
                )
                .and_then(|bytes| bytes.checked_add(retained_guide_bytes))
        })
        .ok_or("Visibility inspection allocation overflow")?;
    if decoded > baked_build::MAX_BSP_BYTES as usize || live > MAX_CALL_BYTES {
        return Err("Visibility inspection exceeds its 384 MiB call budget".into());
    }
    control.checkpoint()?;
    let bsp = Bsp::parse(&bytes).map_err(|error| error.to_string())?;
    drop(bytes);
    metadata.validate(&bsp, control)?;
    if bsp.lumps[lump::LEAFS].data.len() > MAX_LEAVES * 56 {
        return Err("Native visibility leaves exceed their inspection limit".into());
    }
    let leaves = bsp.records::<Leaf>().map_err(|error| error.to_string())?;
    if leaves.len() > MAX_LEAVES {
        return Err("Native visibility leaf count exceeds its inspection limit".into());
    }
    Ok((bsp, leaves))
}

fn leaf_row(ordinal: usize, leaves: &[Leaf]) -> Result<LeafRow, String> {
    let leaf = leaves
        .get(ordinal)
        .ok_or("Compiler visibility leaf is outside the native table")?;
    if leaf.mins.iter().zip(leaf.maxs).any(|(&min, max)| min > max) {
        return Err("Native visibility leaf bounds are invalid".into());
    }
    Ok(LeafRow {
        ordinal,
        cluster: leaf.cluster,
        contents: leaf.contents,
        bounds: [leaf.mins.map(f64::from), leaf.maxs.map(f64::from)],
    })
}

fn resolve_world_light(
    bsp: &Bsp,
    metadata: &Snapshot,
    lighting: compiler::LightingMode,
    channel: Channel,
    ordinal: usize,
    control: &Control,
) -> Result<WorldLightResult, String> {
    if !super::compiled_world_lights::channel_baked(lighting, channel) {
        return Err("World-light visibility requires a baked lighting channel; Fullbright and unbaked HDR have no native source light".into());
    }
    let lights = super::compiled_world_lights::read_native_table(bsp, lighting, channel, control)?;
    let light = lights
        .get(ordinal)
        .ok_or("World-light ordinal is outside the selected native table")?;
    if !matches!(
        light.kind,
        world_light_kind::POINT | world_light_kind::SPOT | world_light_kind::SURFACE
    ) {
        return Err("Visibility targets require a compiled point, spot or surface light; directional sky and unknown kinds have no supported source cluster".into());
    }
    let visibility = bsp
        .visibility()
        .map_err(|error| error.to_string())?
        .ok_or("Native visibility is unavailable")?;
    if light.cluster < 0 || light.cluster as usize >= visibility.clusters() {
        return Err(
            "World-light stored source cluster is outside the native visibility table".into(),
        );
    }
    let mut retained = false;
    for leaf in metadata.leaves() {
        control.checkpoint()?;
        if i32::from(leaf.cluster) == light.cluster {
            retained = true;
            break;
        }
    }
    if !retained {
        return Err("World-light stored source cluster has no retained world leaves".into());
    }
    Ok(WorldLightResult {
        channel,
        ordinal,
        cluster: light.cluster,
    })
}

fn resolve_position(
    bsp: &Bsp,
    leaves: &[Leaf],
    metadata: &Snapshot,
    source: PositionSource,
    camera: render::Camera,
    control: &Control,
) -> Result<PositionResult, String> {
    let requested_position = match source {
        PositionSource::Position { position } => position,
        PositionSource::Camera {} => {
            if camera.projection != render::Projection::Perspective {
                return Err("Inspect camera position requires the Perspective view; enter an explicit world position in orthographic views".into());
            }
            camera.position.as_dvec3().to_array()
        }
    };
    let hit = bsp
        .world_leaf_at_controlled(requested_position, &mut || {
            control.checkpoint().map_err(io::Error::other)
        })
        .map_err(|error| error.to_string())?;
    let leaf = leaves
        .get(hit.ordinal)
        .ok_or("World point lookup escaped the native leaf table")?;
    let retained_world_leaf = metadata
        .leaves()
        .binary_search_by_key(&hit.ordinal, |leaf| leaf.ordinal as usize)
        .is_ok();
    if !retained_world_leaf
        && (hit.ordinal != 0
            || leaf.cluster != -1
            || leaf.contents & source_formats::bsp::contents::SOLID == 0)
    {
        return Err("Model0 point lookup escaped the retained world-leaf mapping".into());
    }
    Ok(PositionResult {
        source,
        requested_position,
        effective_position: hit.effective_position,
        native_leaf: hit.ordinal,
        cluster: leaf.cluster,
        contents: leaf.contents,
        retained_world_leaf,
        classification: if leaf.cluster < 0 {
            "solid_or_exterior"
        } else {
            "clustered"
        },
        nodes_visited: hit.nodes_visited,
        numeric_policy: source_formats::bsp::POINT_LEAF_CLASSIFICATION,
    })
}

fn portal_row(
    portal: &compiler::visibility_inspection::PortalRecord,
    leaves: &[Leaf],
) -> Result<PortalRow, String> {
    let mut mins = DVec3::splat(f64::INFINITY);
    let mut maxs = DVec3::splat(f64::NEG_INFINITY);
    for &point in portal.points() {
        mins = mins.min(point);
        maxs = maxs.max(point);
    }
    let [a, b] = portal
        .leaves
        .map(|ordinal| leaves.get(ordinal as usize).map(|leaf| leaf.cluster));
    let clusters = [
        a.ok_or("Portal endpoint is outside the native leaf table")?,
        b.ok_or("Portal endpoint is outside the native leaf table")?,
    ];
    Ok(PortalRow {
        ordinal: portal.ordinal as usize,
        leaves: portal.leaves,
        clusters,
        internal_cluster: clusters[0] == clusters[1],
        vertices: portal.points().len(),
        bounds: [mins.to_array(), maxs.to_array()],
    })
}

fn fill(
    report: &mut Inspection,
    query: &Query,
    metadata: &Snapshot,
    bsp: &Bsp,
    leaves: &[Leaf],
    control: &Control,
    prepare_destinations: bool,
) -> Result<Option<DestinationGuide>, String> {
    let visibility = bsp
        .visibility()
        .map_err(|error| error.to_string())?
        .ok_or("Native visibility is unavailable")?;
    report.summary = Some(Summary {
        world_leaves: metadata.leaves().len(),
        clusters: visibility.clusters(),
        portals: metadata.portals().len(),
        excluded_portals: metadata.excluded_portals(),
        metadata_bytes: metadata.bytes(),
    });
    let cluster = match query.target {
        Target::Summary {} => None,
        Target::Position { .. } => {
            if !report
                .position
                .as_ref()
                .is_some_and(|position| position.native_leaf == 0 && !position.retained_world_leaf)
            {
                return Err("Position lookup did not resolve an owned world leaf".into());
            }
            if query.offset != 0 || query.visibility_offset != 0 {
                return Err("Reserved leaf0 has no retained portal or PVS/PAS pages".into());
            }
            report.page.total = Some(0);
            return Ok(None);
        }
        Target::WorldLight { .. } => {
            return Err(
                "World-light visibility target did not resolve its stored source cluster".into(),
            );
        }
        Target::Leaf { ordinal } => {
            if metadata
                .leaves()
                .binary_search_by_key(&ordinal, |leaf| leaf.ordinal as usize)
                .is_err()
            {
                return Err("Requested leaf is not an emitted world leaf in this build".into());
            }
            let row = leaf_row(ordinal, leaves)?;
            let cluster = (row.cluster >= 0).then_some(row.cluster as usize);
            report.selection = Some(Row::Leaf(row));
            cluster
        }
        Target::Cluster { ordinal } => {
            if ordinal >= visibility.clusters() {
                return Err("Requested cluster is outside the native visibility table".into());
            }
            let mut count = 0;
            for leaf in metadata.leaves() {
                control.checkpoint()?;
                count += usize::from(leaf.cluster >= 0 && leaf.cluster as usize == ordinal);
            }
            report.selection = Some(Row::Cluster {
                ordinal,
                leaves: count,
            });
            Some(ordinal)
        }
        Target::Portal { ordinal } => {
            let portal = metadata
                .portal(ordinal)
                .ok_or("Requested portal slot is not retained by this build")?;
            report.selection = Some(Row::Portal(portal_row(portal, leaves)?));
            report.selected_vertices = Some(
                portal
                    .points()
                    .iter()
                    .map(|point| point.to_array())
                    .collect(),
            );
            None
        }
    };
    let mut total = 0usize;
    match query.target {
        Target::Leaf { ordinal } => {
            for portal in metadata.portals() {
                control.checkpoint()?;
                if portal.leaves.contains(&(ordinal as u32)) {
                    if (query.offset..query.offset + query.limit).contains(&total) {
                        report
                            .page
                            .rows
                            .push(Row::Portal(portal_row(portal, leaves)?));
                    }
                    total += 1;
                }
            }
        }
        Target::Portal { ordinal } => {
            for leaf in metadata.portal(ordinal).unwrap().leaves {
                control.checkpoint()?;
                if (query.offset..query.offset + query.limit).contains(&total) {
                    report
                        .page
                        .rows
                        .push(Row::Leaf(leaf_row(leaf as usize, leaves)?));
                }
                total += 1;
            }
        }
        _ => {
            for leaf in metadata.leaves() {
                control.checkpoint()?;
                if matches!(query.target, Target::Summary {})
                    || cluster.is_some_and(|cluster| {
                        leaf.cluster >= 0 && leaf.cluster as usize == cluster
                    })
                {
                    if (query.offset..query.offset + query.limit).contains(&total) {
                        report
                            .page
                            .rows
                            .push(Row::Leaf(leaf_row(leaf.ordinal as usize, leaves)?));
                    }
                    total += 1;
                }
            }
        }
    }
    if query.offset > total {
        return Err("Visibility page offset exceeds its table count".into());
    }
    report.page.total = Some(total);
    report.page.next_offset = (query.offset + report.page.rows.len() < total)
        .then_some(query.offset + report.page.rows.len());
    let mut destinations = None;
    if let Some(cluster) = cluster {
        let kind = match query.kind {
            Kind::Pvs => VisibilityKind::Visible,
            Kind::Pas => VisibilityKind::Audible,
        };
        let row = visibility
            .row(cluster, kind)
            .map_err(|error| error.to_string())?;
        let mut total = 0usize;
        for target in 0..visibility.clusters() {
            control.checkpoint()?;
            if row[target / 8] & (1 << (target % 8)) != 0 {
                if (query.visibility_offset..query.visibility_offset + query.limit).contains(&total)
                {
                    report.visibility.rows.push(target);
                }
                total += 1;
            }
        }
        if query.visibility_offset > total {
            return Err("PVS/PAS offset exceeds its destination count".into());
        }
        if prepare_destinations {
            destinations = Some(destination_bounds(
                &row,
                metadata.leaves(),
                leaves,
                cluster,
                total,
                control,
            )?);
        }
        report.visibility.total = Some(total);
        report.visibility.next_offset = (query.visibility_offset + report.visibility.rows.len()
            < total)
            .then_some(query.visibility_offset + report.visibility.rows.len());
    } else if query.visibility_offset != 0 {
        return Err("This visibility target has no PVS/PAS source cluster".into());
    }
    Ok(destinations)
}

fn destination_storage(capacity: usize) -> Result<(usize, usize), String> {
    let retained = capacity
        .checked_mul(size_of::<GuideBox>())
        .and_then(|bytes| bytes.checked_add(size_of::<DestinationGuide>()))
        .ok_or("Destination guide storage overflow")?;
    let line_vertices = capacity
        .checked_mul(LINE_VERTICES_PER_BOX)
        .ok_or("Destination guide line count overflow")?;
    line_vertices.checked_mul(size_of::<render::scene::LineVertex>())
        .and_then(|bytes| bytes.checked_add(retained))
        .filter(|&bytes| bytes <= MAX_GUIDE_BYTES)
        .ok_or("Destination guide exceeds its 256 KiB bounds/renderer budget; previous guide is unchanged")?;
    Ok((retained, line_vertices))
}

fn destination_bounds(
    row: &[u8],
    world: &[compiler::visibility_inspection::WorldLeaf],
    leaves: &[Leaf],
    source_cluster: usize,
    clusters: usize,
    control: &Control,
) -> Result<DestinationGuide, String> {
    let included = |leaf: &compiler::visibility_inspection::WorldLeaf| {
        if leaf.cluster < 0 {
            return Ok(false);
        }
        let byte = row
            .get(leaf.cluster as usize / 8)
            .ok_or("Destination cluster exceeds its native row")?;
        Ok::<_, String>(byte & (1 << (leaf.cluster as usize % 8)) != 0)
    };
    let mut count = 0usize;
    for leaf in world {
        control.checkpoint()?;
        if included(leaf)? {
            count = count
                .checked_add(1)
                .ok_or("Destination guide leaf count overflow")?;
        }
    }
    control.checkpoint()?;
    if count > MAX_GUIDE_LEAVES {
        return Err(format!(
            "Complete destination guide has {count} native world leaves across {clusters} destination clusters; cap is {MAX_GUIDE_LEAVES} leaves. Guide unavailable; previous guide is unchanged. Ordinary inspection remains available"
        ));
    }
    destination_storage(count)?;
    control.checkpoint()?;
    let mut boxes = Vec::new();
    boxes
        .try_reserve_exact(count)
        .map_err(|_| "Cannot reserve complete destination guide; previous guide is unchanged")?;
    let (retained_bytes, _) = destination_storage(boxes.capacity())?;
    for leaf in world {
        control.checkpoint()?;
        if included(leaf)? {
            let row = leaf_row(leaf.ordinal as usize, leaves)?;
            boxes.push(GuideBox {
                ordinal: row.ordinal,
                bounds: row.bounds,
            });
        }
    }
    control.checkpoint()?;
    Ok(DestinationGuide {
        source_cluster,
        clusters,
        boxes,
        retained_bytes,
        line_vertices: count * LINE_VERTICES_PER_BOX,
    })
}

fn stale(report: &mut Inspection, message: &str) {
    if report.build["status"] != "unavailable" {
        report.build["status"] = json!("stale");
    }
    report.build["current"] = json!(false);
    report.overlay["displayed"] = json!(false);
    if report.diagnostics.len() < 32 {
        report
            .diagnostics
            .push(baked_build::bounded_message(message));
    }
}

fn unavailable(report: &mut Inspection, message: &str) {
    stale(report, message);
    report.build["status"] = json!("unavailable");
    report.build["token"] = Value::Null;
    report.summary = None;
    report.selection = None;
    report.selected_vertices = None;
    report.position = None;
    report.world_light = None;
    report.page.rows.clear();
    report.page.total = None;
    report.page.next_offset = None;
    report.visibility.rows.clear();
    report.visibility.total = None;
    report.visibility.next_offset = None;
}

struct ReportLimit<'a> {
    bytes: usize,
    control: &'a Control,
}
impl Write for ReportLimit<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.control.checkpoint().map_err(io::Error::other)?;
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|&bytes| bytes <= MAX_REPORT_BYTES)
            .ok_or_else(|| io::Error::other("Visibility report exceeds its 512 KiB limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn finish(mut report: Inspection, control: &Control) -> Result<Inspection, String> {
    control.checkpoint()?;
    report.page.returned = report.page.rows.len();
    report.visibility.returned = report.visibility.rows.len();
    report.historical = report.build["status"] == "stale";
    if report.build["current"] != true {
        report.overlay["displayed"] = json!(false);
    }
    serde_json::to_writer(ReportLimit { bytes: 0, control }, &report)
        .map_err(|error| error.to_string())?;
    control.checkpoint()?;
    Ok(report)
}
