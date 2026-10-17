use compiler::face_origins::{FaceOrigins, Origin};
use serde::{Deserialize, Serialize};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceReference {
    Root {
        solid: u32,
        side: u32,
    },
    Instance {
        instance: u32,
        file: String,
        solid: u32,
        side: u32,
    },
}

impl SourceReference {
    fn matches(&self, origin: &Origin) -> bool {
        match (self, origin) {
            (
                Self::Root { solid, side },
                Origin::Root {
                    solid: other_solid,
                    side: other_side,
                    ..
                },
            ) => solid == other_solid && side == other_side,
            (
                Self::Instance {
                    instance,
                    file,
                    solid,
                    side,
                },
                Origin::Instance {
                    instance: other_instance,
                    file: other_file,
                    solid: other_solid,
                    side: other_side,
                },
            ) => {
                instance == other_instance
                    && file == other_file
                    && solid == other_solid
                    && side == other_side
            }
            _ => false,
        }
    }
}

pub(super) fn unavailable(error: &str) -> Value {
    json!({"status":"unavailable", "diagnostics":[error], "contributors":[], "matches":[]})
}

pub(super) fn matches(
    origins: &FaceOrigins,
    source: &SourceReference,
    offset: usize,
    limit: usize,
) -> (Value, Option<usize>) {
    let mut total = 0;
    let mut rows = Vec::with_capacity(limit);
    for face in 0..origins.len() {
        if origins
            .contributors(face)
            .is_some_and(|mut contributors| contributors.any(|origin| source.matches(origin)))
        {
            if total >= offset && rows.len() < limit {
                rows.push(json!({"face":face,"model":origins.model(face)}));
            }
            total += 1;
        }
    }
    let selected = rows
        .first()
        .and_then(|row| row["face"].as_u64())
        .map(|face| face as usize);
    (
        json!({"source":source,"matches":rows,"match_page":{"offset":offset,"limit":limit,"total":total,"next_offset":offset.checked_add(limit).filter(|&next| next < total)},
        "diagnostics":if total == 0 {vec!["No retained emitted face matches this authored source; no omission reason was retained"]} else if offset >= total {vec!["Match page offset exceeds the retained match count"]} else {Vec::<&str>::new()}}),
        selected,
    )
}

pub(super) fn selected(
    origins: &FaceOrigins,
    bsp: &Bsp,
    channel: Channel,
    face: usize,
    offset: usize,
    limit: usize,
) -> Value {
    let Some(contributors) = origins.contributors(face) else {
        return unavailable("No retained record for this compiled face");
    };
    let count = contributors.len();
    let contributors: Vec<_> = contributors.skip(offset).take(limit).collect();
    let displacement = bsp
        .lighting_faces(channel.kind())
        .ok()
        .and_then(|faces| faces.get(face).map(|face| face.dispinfo))
        .filter(|&index| index >= 0);
    json!({"face":face,"model":origins.model(face),"displacement":displacement,
        "contributors":contributors,"contributor_page":{"offset":offset,"limit":limit,"total":count,"next_offset":offset.checked_add(limit).filter(|&next| next < count)},
        "relation":"Retained contributing authored surfaces; splits may yield many compiled faces, and merges may combine sources. No exact authored polygon footprint or vertex/luxel correspondence is asserted.",
        "diagnostics":if count == 0 {vec!["No authored source was retained for this generated or unresolved face"]} else if offset >= count {vec!["Contributor page offset exceeds the retained contributor count"]} else {Vec::<&str>::new()}})
}

impl Session {
    pub fn selected_baked_lightmap_source(&self) -> Result<SourceReference, String> {
        let faces = self.selected_faces();
        let [face] = faces.as_slice() else {
            return Err(
                "Select exactly one authored root VMF face before inspecting its compiled matches"
                    .into(),
            );
        };
        if map_core::surface::side(self.document(), *face).is_none() {
            return Err("The selected face is absent from the committed root VMF".into());
        }
        Ok(SourceReference::Root {
            solid: face.solid,
            side: face.side,
        })
    }
}
