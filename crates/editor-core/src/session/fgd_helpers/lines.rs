use std::collections::BTreeMap;

use map_core::{
    Document, ObjectId,
    entity_helpers::lines::{self, Line},
};
use serde::Serialize;

use super::super::{Session, working_region::Evaluation};

pub(super) const MAX_DISPLAYED_LINES: usize = 128;
pub(super) const LIMITS: &str = "Selected visible declared line sources; guides coexist with brush, model, light and rope displays. line(R G B, targetname, property) draws self to the named anchor; the seven-argument form draws between two referenced anchors. nodeid pairs require declared node_dest reference properties and point endpoints with a declared readonly integer nodeid. Node IDs must be authored exactly once, canonical decimal 0–2147483647; -1 references declare no association. Numeric aliases are not rendered and cannot hide duplicate matches. No IDs are generated or inferred from VMF object IDs, order or runtime node indexes. Literal integer RGB 0–255 is independent of rendercolor. Authored reference values precede labeled FGD defaults; named endpoints need one authored targetname. Point anchors require a finite authored origin; brush anchors use the bounded brush bounds centre, reported as position_source. Unique ASCII-case-insensitive literal names are displayed; duplicate names or case aliases are ambiguous. Wildcards and !tokens are unsupported. Hidden or working-region-excluded sources/endpoints suppress the line. At most 64 declarations per entity, 128 displayed lines and 65536 lookup entities/raw targetname plus eligible nodeid values; property/value previews cap at 128/256 bytes. Each report pass shares one bounded anchor cache and clipping-work ledger; static_guide_limits reports those limits. Endpoints follow transform previews; bounds, picking, framing, selection, placement and working regions remain unchanged. These are stored FGD editor links, not runtime beams, I/O or AI nodegraph connectivity. Runtime ground-node dropping is not evaluated.";

pub(super) fn source_suppression(facts: &super::classification::Facts) -> Option<&'static str> {
    if facts.hidden {
        Some("authored_hidden")
    } else if facts.editor_hidden {
        Some("editor_hidden")
    } else if facts.outside {
        Some("outside_working_region")
    } else if !facts.selected {
        Some("not_selected")
    } else {
        None
    }
}

pub(super) fn endpoint_suppression(
    document: &Document,
    evaluation: &Evaluation,
    endpoints: &[lines::Endpoint],
) -> Option<&'static str> {
    for endpoint in endpoints {
        let Some(entity) = endpoint.entity.and_then(|id| document.entity(id)) else {
            return Some("missing_endpoint");
        };
        if entity.hidden {
            return Some("endpoint_hidden");
        }
        let object = ObjectId::Entity(entity.id);
        if evaluation.editor_hidden.contains(&object) {
            return Some("endpoint_editor_hidden");
        }
        if !evaluation.objects.contains(&object) {
            return Some("endpoint_outside_working_region");
        }
    }
    None
}

#[derive(Debug, Serialize)]
pub struct Endpoint {
    pub lookup_property: Option<&'static str>,
    pub property: Option<String>,
    pub raw: Option<String>,
    pub raw_truncated: bool,
    pub value_source: &'static str,
    pub entity: Option<u32>,
    pub position: Option<[f64; 3]>,
    pub position_source: Option<&'static str>,
    pub matches: usize,
    pub state: &'static str,
    pub error: Option<String>,
}

impl From<lines::Endpoint> for Endpoint {
    fn from(value: lines::Endpoint) -> Self {
        Self {
            lookup_property: value.lookup_property,
            property: value.property,
            raw: value.raw,
            raw_truncated: value.raw_truncated,
            value_source: value.value_source,
            entity: value.entity,
            position: value.position,
            position_source: value.position_source,
            matches: value.matches,
            state: value.state,
            error: value.error,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub color: Option<[u8; 3]>,
    pub endpoints: [Endpoint; 2],
    pub state: &'static str,
    pub error: Option<String>,
    pub displayed: bool,
    pub suppression_reason: Option<&'static str>,
}

impl Report {
    fn new(line: Line, reason: Option<&'static str>) -> Self {
        Self {
            color: line.color,
            endpoints: line.endpoints.map(Into::into),
            state: line.state,
            error: line.error,
            displayed: reason.is_none(),
            suppression_reason: reason,
        }
    }
}

impl Session {
    fn line_reports(
        &self,
        document: &Document,
        evaluation: &Evaluation,
        inspected: Option<u32>,
    ) -> BTreeMap<u32, Vec<Report>> {
        let context = super::classification::Context::new(evaluation, self.selection());
        let entities = context.entities(document, inspected, true);
        if !entities.values().any(|entity| {
            entity
                .classname()
                .and_then(|name| self.definitions().class(name))
                .is_some_and(|class| class.helpers.iter().any(|helper| helper.name == "line"))
        }) {
            return BTreeMap::new();
        }
        let targets = lines::Targets::new(&document.vmf().entities, self.definitions());
        let mut displayed = 0;
        let mut result = BTreeMap::new();
        for (id, entity) in entities {
            let Some(class) = entity
                .classname()
                .and_then(|name| self.definitions().class(name))
            else {
                continue;
            };
            let facts = context.facts(entity, class);
            let reason = source_suppression(&facts);
            if inspected != Some(id) && (reason.is_some() || displayed == MAX_DISPLAYED_LINES) {
                continue;
            }
            let rows = lines::resolve(class, entity, &targets)
                .into_iter()
                .map(|line| {
                    let reason = reason.or_else(|| {
                        if line.state != "ready" {
                            return Some("unavailable_endpoint");
                        }
                        if let Some(reason) =
                            endpoint_suppression(document, evaluation, &line.endpoints)
                        {
                            return Some(reason);
                        }
                        (displayed == MAX_DISPLAYED_LINES).then_some("scene_guide_limit")
                    });
                    if reason.is_none() {
                        displayed += 1;
                    }
                    Report::new(line, reason)
                })
                .collect::<Vec<_>>();
            if inspected == Some(id)
                || (inspected.is_none() && rows.iter().any(|row| row.displayed))
            {
                result.insert(id, rows);
            }
            if inspected == Some(id) || (inspected.is_none() && displayed == MAX_DISPLAYED_LINES) {
                break;
            }
        }
        result
    }

    pub(super) fn inspected_lines(&self, id: u32) -> Vec<Report> {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        let mut rows = self
            .line_reports(document, &evaluation, Some(id))
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

    pub(in crate::session) fn append_fgd_line_guides(&self, mesh: &mut render::SceneMesh) {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        for line in self
            .line_reports(document, &evaluation, None)
            .into_values()
            .flatten()
            .filter(|line| line.displayed)
        {
            let [r, g, b] = line.color.unwrap();
            for endpoint in line.endpoints {
                mesh.lines.push(render::scene::LineVertex {
                    position: glam::DVec3::from_array(endpoint.position.unwrap())
                        .as_vec3()
                        .to_array(),
                    color: [r, g, b, 255],
                });
            }
        }
    }
}
