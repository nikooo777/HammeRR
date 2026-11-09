use glam::Vec3;
use serde_json::{Value, json};
use source_formats::bsp::native_terrain::{NativeTerrainDisplacement, NativeTerrainTopology};

use super::{
    Actor, Guide, NativeTerrainSample, Ready, Session, bounded_report, inspection_framing,
    vector_report,
};

const POLICY: &str = "Actual native CPU displacement topology captured after completed map-load tessellation. Raw mesh-global u16 index order and all ten active/allowed mask words are retained independently of exact compiled expectations. Local triangle IDs are derived from the observed vertex base. Mismatches remain diagnostics; observed positions and indices are never replaced with expected or predicted geometry. This installed build tessellates at load; no later camera-distance CPU writer is established. No GPU draw, completed rendered frame, dynamic camera LOD or authored triangle correspondence is claimed.";

#[derive(Clone, Copy)]
pub struct TopologyQuery {
    pub token: u64,
    pub displacement: u16,
    pub offset: usize,
    pub limit: usize,
}

pub(super) struct ObservedTriangle {
    global: [u16; 3],
    local: [u16; 3],
    samples: [NativeTerrainSample; 3],
    expected: Option<[u16; 3]>,
}

impl ObservedTriangle {
    pub(super) fn positions(&self) -> Result<[Vec3; 3], String> {
        let points = self
            .samples
            .map(|sample| Vec3::from_array(sample.position_bits().map(f32::from_bits)));
        if points.iter().any(|point| !point.is_finite()) {
            return Err("Observed native triangle has a nonfinite vertex position".into());
        }
        Ok(points)
    }

    pub(super) fn report(&self) -> Value {
        json!({"global_indices":self.global,"local_indices":self.local,
            "positions":self.samples.map(|sample|vector_report(sample.position_bits())),
            "comparison":{"equal":self.expected==Some(self.local),"expected_local_indices":self.expected}})
    }
}

pub(super) fn bounds(points: [Vec3; 3]) -> [Vec3; 2] {
    [
        points[0].min(points[1]).min(points[2]),
        points[0].max(points[1]).max(points[2]),
    ]
}

pub(super) fn topology_report(displacement: &NativeTerrainDisplacement) -> Result<Value, String> {
    let topology = displacement
        .topology()
        .ok_or("Observed native CPU topology is unavailable in this snapshot")?;
    let comparison = topology.comparison();
    let active = topology.active_vertices();
    let allowed = topology.allowed_vertices();
    let compiled = topology.compiled_allowed_vertices();
    Ok(
        json!({"vertex_base":topology.vertex_base(),"vertices":displacement.samples().len(),
        "index_count":topology.indices().len(),"triangle_count":topology.triangles().len(),"expected_triangle_count":topology.expected_triangles().len(),
        "active_vertex_count":topology.active_vertex_count(),"allowed_vertex_count":topology.allowed_vertex_count(),
        "compiled_allowed_vertex_count":topology.compiled_allowed_vertex_count(),
        "active_padding_bit_count":topology.active_padding_bit_count(),"allowed_padding_bit_count":topology.allowed_padding_bit_count(),
        "compiled_allowed_padding_bit_count":topology.compiled_allowed_padding_bit_count(),
        "active_mask_set_bits_320":active.iter().map(|word|word.count_ones()).sum::<u32>(),
        "allowed_mask_set_bits_320":allowed.iter().map(|word|word.count_ones()).sum::<u32>(),
        "compiled_allowed_mask_set_bits_320":compiled.iter().map(|word|word.count_ones()).sum::<u32>(),
        "active_words":active.map(|word|format!("0x{word:08x}")),
        "allowed_words":allowed.map(|word|format!("0x{word:08x}")),
        "compiled_allowed_words":compiled.map(|word|format!("0x{word:08x}")),
        "comparison":{"matches":comparison.matches(),"active_allowed_mismatched_bits":comparison.active_allowed_mismatched_bits,
            "active_compiled_mismatched_bits":comparison.active_compiled_mismatched_bits,
            "allowed_compiled_mismatched_bits":comparison.allowed_compiled_mismatched_bits,
            "ordered_triangle_mismatches":comparison.ordered_triangle_mismatches,
            "mask_comparison_bits":320,"triangle_order":"exact observed ordinal order, including missing or extra triangles"},
        "source":"observed_native_cpu","policy":POLICY}),
    )
}

pub(super) fn catalog_report(ready: &Ready) -> Value {
    let mut indices = 0usize;
    let mut triangles = 0usize;
    let mut mismatched_displacements = 0usize;
    for displacement in ready.snapshot.displacements() {
        let Some(topology) = displacement.topology() else {
            return json!({"available":false,"error":"Observed native CPU topology is unavailable in this snapshot"});
        };
        indices += topology.indices().len();
        triangles += topology.triangles().len();
        mismatched_displacements += usize::from(!topology.comparison().matches());
    }
    json!({"available":true,"displacements":ready.snapshot.displacements().len(),"indices":indices,"triangles":triangles,
        "matches":mismatched_displacements==0,"mismatched_displacements":mismatched_displacements,"source":"observed_native_cpu","policy":POLICY})
}

impl Ready {
    fn topology(&self, ordinal: u16) -> Result<&NativeTerrainTopology, String> {
        self.displacement(ordinal)?
            .topology()
            .ok_or_else(|| "Observed native CPU topology is unavailable in this snapshot".into())
    }

    pub(super) fn observed_triangle(
        &self,
        ordinal: u16,
        triangle: usize,
    ) -> Result<ObservedTriangle, String> {
        let topology = self.topology(ordinal)?;
        let local = *topology
            .triangles()
            .get(triangle)
            .ok_or("Observed native triangle ordinal is absent from this displacement")?;
        let index = triangle
            .checked_mul(3)
            .ok_or("Observed native triangle index overflows")?;
        let raw = topology
            .indices()
            .get(index..index + 3)
            .ok_or("Observed native triangle raw index triple is unavailable")?;
        Ok(ObservedTriangle {
            global: [raw[0], raw[1], raw[2]],
            local,
            samples: [
                self.sample(ordinal, usize::from(local[0]))?,
                self.sample(ordinal, usize::from(local[1]))?,
                self.sample(ordinal, usize::from(local[2]))?,
            ],
            expected: topology.expected_triangles().get(triangle).copied(),
        })
    }
}

impl Session {
    pub(super) fn terrain_triangle_guide_error(
        &self,
        ready: &Ready,
        displacement: u16,
        triangle: usize,
    ) -> Option<String> {
        let result = ready
            .observed_triangle(displacement, triangle)
            .and_then(|triangle| triangle.positions());
        match result {
            Err(error) => Some(error),
            Ok(points) => (!self
                .working_region_bounds_visible(Some(bounds(points).map(|point| point.as_dvec3()))))
            .then(|| "The observed native triangle is outside the working region".into()),
        }
    }

    pub fn inspect_compiled_terrain_topology(&self, query: TopologyQuery) -> Result<Value, String> {
        self.check_terrain_snapshot_token(query.token)?;
        if !(1..=super::MAX_PAGE_ROWS).contains(&query.limit)
            || query.offset.checked_add(query.limit).is_none()
        {
            return Err("Native CPU topology pages require 1–64 triangles without overflow".into());
        }
        let ready = self
            .compiled_terrain_snapshot
            .ready
            .as_ref()
            .ok_or("Prepare a complete observed native terrain snapshot first")?;
        let displacement = ready.displacement(query.displacement)?;
        let topology = ready.topology(query.displacement)?;
        let metadata = topology_report(displacement)?;
        let mut current_error = ready.current(self).err();
        let display_error = self.terrain_snapshot_display_error(ready);
        let mut rows = Vec::new();
        for triangle in query.offset
            ..query
                .offset
                .saturating_add(query.limit)
                .min(topology.triangles().len())
        {
            let observed = ready.observed_triangle(query.displacement, triangle)?;
            let finite = observed.positions().is_ok();
            let mut row = observed.report();
            row["triangle"] = json!(triangle);
            row["frame_available"] =
                json!(current_error.is_none() && display_error.is_none() && finite);
            row["guide_available"] = json!(
                current_error.is_none()
                    && display_error.is_none()
                    && self
                        .terrain_triangle_guide_error(ready, query.displacement, triangle)
                        .is_none()
            );
            rows.push(row);
        }
        current_error = ready.current(self).err().or(current_error);
        if current_error.is_some() {
            for row in &mut rows {
                row["frame_available"] = json!(false);
                row["guide_available"] = json!(false);
            }
        }
        bounded_report(
            json!({"token":query.token.to_string(),"build_token":ready.build.token.to_string(),"profile":ready.profile,
            "status":if current_error.is_some(){"historical"}else{"ready"},"current":current_error.is_none(),"current_error":current_error,
            "displacement":query.displacement,"face":displacement.face(),"power":displacement.power(),"topology":metadata,
            "page":{"offset":query.offset,"limit":query.limit,"total":topology.triangles().len(),"returned":rows.len(),
                "next_offset":query.offset.checked_add(query.limit).filter(|offset|*offset<topology.triangles().len()),"rows":rows},
            "guide":self.terrain_guide_report(),"policy":POLICY}),
        )
    }

    pub fn frame_compiled_terrain_triangle(
        &mut self,
        actor: Actor,
        token: u64,
        displacement: u16,
        triangle: usize,
        aspect: f32,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.check_terrain_snapshot_token(token)?;
        inspection_framing::validate_aspect(aspect)?;
        let ready = self
            .compiled_terrain_snapshot
            .ready
            .as_ref()
            .ok_or("Prepare a complete observed native terrain snapshot first")?;
        if let Some(error) = self.terrain_snapshot_display_error(ready) {
            return Err(error);
        }
        let observed = ready.observed_triangle(displacement, triangle)?;
        let [mins, maxs] = bounds(observed.positions()?);
        let camera =
            self.inspection_frame_camera(mins, maxs, aspect, "Observed native CPU triangle")?;
        ready.current(self)?;
        let report = json!({"token":token.to_string(),"build_token":ready.build.token.to_string(),"profile":ready.profile,
            "displacement":displacement,"triangle":triangle,"observation":observed.report(),
            "camera":{"position":camera.position.to_array(),"yaw_degrees":camera.yaw_degrees,"pitch_degrees":camera.pitch_degrees}});
        self.try_set_camera(camera)?;
        self.log(
            actor,
            format!("Frame observed native displacement {displacement} triangle {triangle}"),
        );
        Ok(report)
    }

    pub fn set_compiled_terrain_triangle_guide(
        &mut self,
        actor: Actor,
        token: u64,
        displacement: u16,
        triangle: usize,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.check_terrain_snapshot_token(token)?;
        let ready = self
            .compiled_terrain_snapshot
            .ready
            .as_ref()
            .ok_or("Prepare a complete observed native terrain snapshot first")?;
        if let Some(error) = self
            .terrain_snapshot_display_error(ready)
            .or_else(|| self.terrain_triangle_guide_error(ready, displacement, triangle))
        {
            return Err(error);
        }
        self.compiled_terrain_snapshot.guide = Some(Guide::TriangleOutline {
            displacement,
            triangle,
        });
        self.compiled_terrain_snapshot.visible = true;
        self.invalidate_compiled_terrain_snapshot_render();
        self.log(
            actor,
            format!("Show observed native displacement {displacement} triangle {triangle}"),
        );
        Ok(self.terrain_guide_report())
    }
}
