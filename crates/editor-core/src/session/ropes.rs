use std::collections::BTreeMap;
use std::sync::Arc;

use glam::DVec3;
use map_core::{Document, ObjectId, query};
use render::ropes::{self, Preview};
use serde_json::{Value, json};

use super::Session;

impl Session {
    pub(crate) fn presentation_bounds_in(
        &self,
        document: &Document,
        rope_preview: Option<bool>,
    ) -> BTreeMap<ObjectId, (DVec3, DVec3)> {
        let mut bounds = self.display_bounds_in(document);
        if !rope_preview.unwrap_or(self.view.rope_preview) {
            return bounds;
        }
        let evaluation = self.working_evaluation(document);
        for link in &evaluation.ropes.links {
            let source = ObjectId::Entity(link.source);
            let span = link.bounds();
            let helper = bounds
                .get(&source)
                .copied()
                .or_else(|| query::bounds(document, source));
            let combined = helper.map_or(span, |(mins, maxs)| (mins.min(span.0), maxs.max(span.1)));
            bounds.insert(source, combined);
        }
        bounds
    }

    pub(crate) fn presentation_object_bounds_in(
        &self,
        document: &Document,
        ids: &[ObjectId],
        rope_preview: Option<bool>,
    ) -> Option<(DVec3, DVec3)> {
        if ids.is_empty() {
            return None;
        }
        let bounds = self.presentation_bounds_in(document, rope_preview);
        super::models::combined_bounds(document, ids, &bounds)
    }

    pub(super) fn rope_preview_geometry(&self) -> Arc<Preview> {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        evaluation.ropes.clone()
    }

    pub fn rope_preview_state(&self) -> Value {
        self.rope_preview_report(self.view.rope_preview)
    }

    pub fn rope_preview_report(&self, enabled: bool) -> Value {
        let preview = self.rope_preview_geometry();
        let mut summary = if enabled {
            format!("{} static rope spans", preview.links.len())
        } else {
            "Rope preview is off".into()
        };
        if enabled {
            if let Some(issue) = preview.issues.first() {
                summary.push_str(&format!(" · entity:{}: {}", issue.entity, issue.message));
                if preview.issues.len() > 1 {
                    summary.push_str(&format!(" ({} more issues)", preview.issues.len() - 1));
                }
            }
            if preview.omitted > 0 {
                summary.push_str(&format!(
                    " · {} rope nodes exceed the preview limit",
                    preview.omitted
                ));
            }
        }
        let links = preview.links.iter().map(|link| json!({
            "source":format!("entity:{}", link.source), "target":format!("entity:{}", link.target),
            "width":link.width, "slack":link.slack, "length":link.length(),
            "points":link.points.iter().map(|point| point.to_array()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>();
        let issues = preview
            .issues
            .iter()
            .map(|issue| {
                json!({
                    "entity":format!("entity:{}", issue.entity), "message":issue.message,
                })
            })
            .collect::<Vec<_>>();
        json!({
            "enabled":enabled, "applied":enabled && !links.is_empty(), "static":true,
            "summary":summary, "links":links, "issues":issues, "omitted_nodes":preview.omitted,
            "limits":{"nodes":ropes::MAX_LINKS, "segments_per_span":ropes::MAX_CURVE_SEGMENTS,
                "identifier_bytes":ropes::MAX_IDENTIFIER_BYTES, "diagnostic_excerpt_bytes":ropes::MAX_DIAGNOSTIC_EXCERPT_BYTES,
                "coordinate_extent":ropes::MAX_EXTENT, "slack": [0.0, ropes::MAX_SLACK], "width":[1.0, ropes::MAX_WIDTH]},
            "appearance":"Neutral helper shading; authored RopeMaterial, TextureScale and Barbed are not rendered",
            "motion":"Static downward curve fitted to endpoint distance plus Slack; no physics, collision, wind, animation, runtime I/O, parenting or attachment binding",
            "selection":"A span belongs to its source entity. Enabled admitted spans add their sampled curve bounds, including half Width, to that source's ordinary helper for framing, object-bound inspection and area/volume selection. These are conservative axis-aligned bounds; contained requires the helper and entire outgoing span. Targets retain their own helper and outgoing-span ownership. Authored transforms, pivots, placement and paint keep ordinary helper bounds.",
        })
    }
}

#[cfg(test)]
mod tests;
