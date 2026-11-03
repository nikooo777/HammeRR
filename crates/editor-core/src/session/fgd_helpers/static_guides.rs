use std::collections::BTreeMap;

use map_core::{
    Document,
    entity_helpers::{
        lines,
        static_guides::{self, Guide},
    },
};
use serde::Serialize;

use super::super::{Session, working_region::Evaluation};
use super::lines::{endpoint_suppression, source_suppression};

const MAX_DISPLAYED_GUIDES: usize = 128;
const MAX_SOURCE_SEGMENTS: usize =
    static_guides::MAX_GUIDES_PER_ENTITY * static_guides::MAX_SEGMENTS_PER_GUIDE;
const MAX_SCENE_SEGMENTS: usize = 8192;

#[derive(Debug, Serialize)]
pub struct Limits {
    pub guides_per_source: usize,
    pub segments_per_guide: usize,
    pub segments_per_source: usize,
    pub displayed_guides: usize,
    pub line_declarations_per_source: usize,
    pub displayed_lines: usize,
    pub scene_segments: usize,
    pub property_bytes: usize,
    pub value_bytes: usize,
    pub lookup_entities: usize,
    pub anchor_solids: usize,
    pub anchor_sides: usize,
    pub anchor_brush_sides: usize,
    pub anchor_work: usize,
    pub anchor_cache_entries: usize,
    pub policy: &'static str,
}

pub(super) fn limits() -> Limits {
    Limits {
        guides_per_source: static_guides::MAX_GUIDES_PER_ENTITY,
        segments_per_guide: static_guides::MAX_SEGMENTS_PER_GUIDE,
        segments_per_source: MAX_SOURCE_SEGMENTS,
        displayed_guides: MAX_DISPLAYED_GUIDES,
        line_declarations_per_source: lines::MAX_LINES_PER_ENTITY,
        displayed_lines: super::lines::MAX_DISPLAYED_LINES,
        scene_segments: MAX_SCENE_SEGMENTS,
        property_bytes: static_guides::MAX_KEY_BYTES,
        value_bytes: static_guides::MAX_VALUE_BYTES,
        lookup_entities: lines::MAX_TARGET_ENTITIES,
        anchor_solids: static_guides::MAX_ANCHOR_SOLIDS,
        anchor_sides: static_guides::MAX_ANCHOR_SIDES,
        anchor_brush_sides: static_guides::MAX_ANCHOR_BRUSH_SIDES,
        anchor_work: static_guides::MAX_ANCHOR_WORK,
        anchor_cache_entries: lines::MAX_ANCHOR_CACHE,
        policy: "Selected visible sources in entity-ID/declaration order; guides coexist with brush, model and specialized displays. Entity helpers controls drawing. Only complete guides within source and scene geometry budgets are drawn. Wirebox vectors are relative to the point origin or brush bounds centre, without rotation; typed origin, vecline and axis coordinates are absolute world positions. Cylinder radii belong to their resolved endpoints. Authored values precede labeled FGD defaults; missing, malformed and unsupported inputs retain diagnostics without replacement geometry. Typed point markers are fixed camera-independent wire crosses, not draggable native handles. Visual bounds are separate from object bounds; picking, placement, framing, working regions, map data and history are unchanged.",
    }
}

#[derive(Debug, Serialize)]
pub struct Input {
    pub entity: u32,
    pub property: String,
    pub property_type: Option<String>,
    pub raw: Option<String>,
    pub raw_truncated: bool,
    pub value_source: &'static str,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub kind: &'static str,
    pub declaration_index: Option<usize>,
    pub property: Option<String>,
    pub property_truncated: bool,
    pub arguments: Vec<String>,
    pub arguments_truncated: bool,
    pub inputs: Vec<Input>,
    pub endpoints: Vec<super::lines::Endpoint>,
    pub anchor: Option<[f64; 3]>,
    pub anchor_source: Option<&'static str>,
    pub radii: Option<[f64; 2]>,
    pub color: Option<[u8; 3]>,
    pub marker_policy: Option<&'static str>,
    pub segments: Vec<[[f64; 3]; 2]>,
    pub visual_bounds: Option<[[f64; 3]; 2]>,
    pub state: &'static str,
    pub error: Option<String>,
    pub omitted: usize,
    pub displayed: bool,
    pub suppression_reason: Option<&'static str>,
}

impl Report {
    fn new(guide: Guide, reason: Option<&'static str>) -> Self {
        Self {
            kind: guide.kind,
            declaration_index: guide.declaration_index,
            property: guide.property,
            property_truncated: guide.property_truncated,
            arguments: guide.arguments,
            arguments_truncated: guide.arguments_truncated,
            inputs: guide
                .inputs
                .into_iter()
                .map(|input| Input {
                    entity: input.entity,
                    property: input.property,
                    property_type: input.property_type,
                    raw: input.raw,
                    raw_truncated: input.raw_truncated,
                    value_source: input.value_source,
                    error: input.error,
                })
                .collect(),
            endpoints: guide.endpoints.into_iter().map(Into::into).collect(),
            anchor: guide.anchor,
            anchor_source: guide.anchor_source,
            radii: guide.radii,
            color: guide.color,
            marker_policy: guide.marker_policy,
            segments: guide.segments,
            visual_bounds: guide.visual_bounds,
            state: guide.state,
            error: guide.error,
            omitted: guide.omitted,
            displayed: reason.is_none(),
            suppression_reason: reason,
        }
    }
}

impl Session {
    fn static_guide_reports(
        &self,
        document: &Document,
        evaluation: &Evaluation,
        inspected: Option<u32>,
    ) -> BTreeMap<u32, Vec<Report>> {
        let context = super::classification::Context::new(evaluation, self.selection());
        let entities = context.entities(document, inspected, true);
        if entities.is_empty() {
            return BTreeMap::new();
        }
        let targets = lines::Targets::new(&document.vmf().entities, self.definitions());
        let mut displayed = 0;
        let mut scene_segments = 0;
        let mut result = BTreeMap::new();
        for (id, entity) in entities {
            if inspected.is_some_and(|target| id > target) {
                break;
            }
            if inspected != Some(id)
                && (displayed == MAX_DISPLAYED_GUIDES || scene_segments == MAX_SCENE_SEGMENTS)
            {
                continue;
            }
            let Some(class) = entity
                .classname()
                .and_then(|name| self.definitions().class(name))
            else {
                continue;
            };
            let facts = context.facts(entity, class);
            let source_reason = source_suppression(&facts);
            if source_reason.is_some() && inspected != Some(id) {
                continue;
            }
            let mut source_segments = 0;
            let rows =
                static_guides::resolve(document, class, entity, self.definitions(), &targets)
                    .into_iter()
                    .map(|guide| {
                        let segments = guide.segments.len();
                        let reason = source_reason.or_else(|| {
                            if guide.omitted > 0 {
                                Some("source_guide_limit")
                            } else if guide.state != "ready" {
                                Some("unavailable_guide")
                            } else if let Some(reason) =
                                endpoint_suppression(document, evaluation, &guide.endpoints)
                            {
                                Some(reason)
                            } else if source_segments + segments > MAX_SOURCE_SEGMENTS {
                                Some("source_segment_limit")
                            } else if displayed == MAX_DISPLAYED_GUIDES {
                                Some("scene_guide_limit")
                            } else if scene_segments + segments > MAX_SCENE_SEGMENTS {
                                Some("scene_segment_limit")
                            } else {
                                None
                            }
                        });
                        if reason.is_none() {
                            displayed += 1;
                            source_segments += segments;
                            scene_segments += segments;
                        }
                        Report::new(guide, reason)
                    })
                    .collect::<Vec<_>>();
            if inspected == Some(id)
                || (inspected.is_none() && rows.iter().any(|row| row.displayed))
            {
                result.insert(id, rows);
            }
            if inspected == Some(id)
                || (inspected.is_none()
                    && (displayed == MAX_DISPLAYED_GUIDES || scene_segments == MAX_SCENE_SEGMENTS))
            {
                break;
            }
        }
        result
    }

    pub(super) fn inspected_static_guides(&self, id: u32) -> Vec<Report> {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        let mut rows = self
            .static_guide_reports(document, &evaluation, Some(id))
            .remove(&id)
            .unwrap_or_default();
        if !self.view.entity_helpers {
            for row in &mut rows {
                if row.displayed {
                    row.displayed = false;
                    row.suppression_reason = Some("view_disabled");
                }
            }
        }
        rows
    }

    pub(in crate::session) fn append_fgd_static_guides(&self, mesh: &mut render::SceneMesh) {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        for row in self
            .static_guide_reports(document, &evaluation, None)
            .into_values()
            .flatten()
            .filter(|row| row.displayed)
        {
            let [r, g, b] = row.color.unwrap();
            for segment in row.segments {
                for position in segment {
                    mesh.lines.push(render::scene::LineVertex {
                        position: glam::DVec3::from_array(position).as_vec3().to_array(),
                        color: [r, g, b, 255],
                    });
                }
            }
        }
    }
}
