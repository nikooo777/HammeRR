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
use source_formats::bsp::{Bsp, Leaf, VisibilityKind, lump};

use super::{Actor, Session, baked_build};

pub const MAX_PAGE_ROWS: usize = 64;
pub const MAX_REPORT_BYTES: usize = 512 * 1024;
const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;
const MAX_CALL_BYTES: usize = 384 * 1024 * 1024;
const SOURCE_MODE: baked_build::SourceVerification =
    baked_build::SourceVerification::OriginalMetadata {
        maximum_bytes: MAX_SOURCE_BYTES,
    };
const NOTES: &str = "Compiler-owned world leaves and final clusters. Leaf boxes are native bounds, not exact convex cells. Portal rings retain compiler binary64 coordinates and build-local original slots, including boundaries inside coarsened clusters; they are not authored faces or BSP render surfaces. PVS/PAS decode separate native visibility rows. Historical ordinals belong only to their captured build. Freshness covers the committed map, settings/content epoch and retained source guards, not every world material. Unapplied drafts are not compiled. Framing and the single selected guide require a current owned build.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Summary {},
    Leaf { ordinal: usize },
    Cluster { ordinal: usize },
    Portal { ordinal: usize },
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
    pub page: Page<Row>,
    pub visibility: Page<usize>,
    pub diagnostics: Vec<String>,
    pub overlay: Value,
    pub limits: Value,
    pub notes: &'static str,
}

impl Inspection {
    pub fn current_bounds(&self) -> Result<[[f64; 3]; 2], String> {
        if self.build["current"] != true || self.build["status"] != "current" {
            return Err("Framing and guides require a current successful owned build".into());
        }
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
    bounds: [[f64; 3]; 2],
    snapshot: Arc<Snapshot>,
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
            page: Page::empty(query.offset, query.limit),
            visibility: Page::empty(query.visibility_offset, query.limit),
            diagnostics: Vec::new(),
            overlay: self.compiled_visibility_overlay(),
            limits: json!({"bsp_bytes":baked_build::MAX_BSP_BYTES,"call_bytes":MAX_CALL_BYTES,"metadata_bytes":compiler::visibility_inspection::MAX_BYTES,"report_bytes":MAX_REPORT_BYTES,"page_rows":MAX_PAGE_ROWS,"portal_vertices":MAX_PORTAL_POINTS}),
            notes: NOTES,
        };
        let Some(build) = &self.compiled else {
            return finish(report, control);
        };
        if report.build["status"] == "unavailable" {
            return finish(report, control);
        }
        let metadata = match &build.visibility_inspection {
            Ok(metadata) => metadata,
            Err(error) => {
                unavailable(&mut report, error);
                return finish(report, control);
            }
        };
        let identity = build
            .lightmap_identity
            .output
            .as_ref()
            .map_err(|error| baked_build::bounded_message(error))?;
        let decoded = read_native(identity, metadata, control);
        let (bsp, leaves) = match decoded {
            Ok(decoded) => decoded,
            Err(error) => {
                control.checkpoint()?;
                unavailable(&mut report, &error);
                return finish(report, control);
            }
        };
        if let Some(error) = baked_build::source_issue_with_mode(build, SOURCE_MODE, control) {
            control.checkpoint()?;
            stale(&mut report, &error);
        }
        fill(&mut report, query, metadata, &bsp, &leaves, control)?;
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
        finish(report, control)
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
        json!({"requested":true,"displayed":current,"build_token":guide.token.to_string(),"target":guide.target,"reason":if current {Value::Null} else {json!("The retained guide is suppressed until its owned build is current")}})
    }

    pub fn set_compiled_visibility_overlay(
        &mut self,
        actor: Actor,
        build_token: Option<String>,
        target: Option<Target>,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        let Some(target) = target else {
            self.compiled_visibility.guide = None;
            self.compiled_visibility.displayed = false;
            self.scene_serial += 1;
            self.log(actor, "Cleared compiled visibility guide");
            return Ok(self.compiled_visibility_overlay());
        };
        self.visibility_view_ready()?;
        if !matches!(target, Target::Leaf { .. } | Target::Portal { .. }) {
            return Err("Choose a compiled leaf or portal guide".into());
        }
        let build_token =
            build_token.ok_or("A compiled visibility guide requires the returned build token")?;
        let report = self.inspect_compiled_visibility(&Query {
            build_token: Some(build_token),
            target,
            ..Query::default()
        })?;
        let bounds = report.current_bounds()?;
        let build = self.compiled.as_ref().ok_or("No owned build remains")?;
        let metadata = build
            .visibility_inspection
            .as_ref()
            .map_err(|error| baked_build::bounded_message(error))?;
        self.compiled_visibility.guide = Some(Guide {
            token: build.lightmap_identity.token,
            target,
            bounds,
            snapshot: Arc::clone(metadata),
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
        if !aspect.is_finite() || !(0.01..=100.0).contains(&aspect) {
            return Err("Viewport aspect must be finite and between 0.01 and 100".into());
        }
        let report = self.inspect_compiled_visibility(&Query {
            build_token: Some(build_token),
            target,
            ..Query::default()
        })?;
        let [mins, maxs] = report.current_bounds()?.map(DVec3::from_array);
        let camera = self
            .camera
            .frame_bounds(mins.as_vec3(), maxs.as_vec3(), aspect);
        if !camera.position.is_finite()
            || !camera.view_projection(aspect).is_finite()
            || !camera.yaw_degrees.is_finite()
            || !camera.pitch_degrees.is_finite()
            || !camera.fov_y_degrees.is_finite()
            || !camera.ortho_height.is_finite()
        {
            return Err("Compiled visibility cannot be framed with the current camera".into());
        }
        self.set_camera(camera);
        self.log(actor, "Frame compiled visibility bounds");
        Ok(report)
    }

    fn visibility_view_ready(&self) -> Result<(), String> {
        if self.viewport_pick_conflict()
            || self.measurement_picking()
            || self.linked_path_picking()
            || self.block_preview.is_some()
            || self.clip_drag.is_some()
        {
            return Err("Finish or cancel the active viewport gesture or block preview before framing or showing compiled visibility".into());
        }
        Ok(())
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
        match guide.target {
            Target::Leaf { .. } => {
                let [mins, maxs] = guide.bounds.map(DVec3::from_array);
                mesh.add_bounds_lines(mins, maxs, color);
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
    control: &Control,
) -> Result<(Bsp, Vec<Leaf>), String> {
    let bytes = baked_build::read_owned_bytes_controlled(identity, control)?;
    let decoded = baked_build::decoded_bsp_bound(&bytes)?;
    let live = decoded
        .checked_add(bytes.len())
        .and_then(|bytes| bytes.checked_add(metadata.bytes()))
        .and_then(|bytes| {
            bytes.checked_add(MAX_SOURCE_BYTES + MAX_LEAVES * 56 + 4 * MAX_REPORT_BYTES)
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
) -> Result<(), String> {
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
        report.visibility.total = Some(total);
        report.visibility.next_offset = (query.visibility_offset + report.visibility.rows.len()
            < total)
            .then_some(query.visibility_offset + report.visibility.rows.len());
    } else if query.visibility_offset != 0 {
        return Err("This visibility target has no PVS/PAS source cluster".into());
    }
    Ok(())
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

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
mod tests;
