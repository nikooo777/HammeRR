use render::ropes::{self, Preview};
use serde_json::{Value, json};
use std::sync::Arc;

use super::Session;

impl Session {
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
            "selection":"A span picks its source entity; point helpers remain available. Area selection and framing use endpoint helpers.",
        })
    }
}

#[cfg(test)]
mod tests;
