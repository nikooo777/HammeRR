use std::collections::{BTreeSet, HashSet};

use glam::DVec3;
use map_core::{
    Document, ObjectId, query,
    surface::{self, FaceId},
};
use render::ViewOptions;
use serde::Serialize;
use serde_json::{Value, json};

use super::{Session, manipulation};

const MAX_DIAGNOSTICS: usize = 64;
const MAX_COORDINATE: f64 = crate::spatial::MAX_EXTENT;

#[derive(Default, Serialize)]
pub struct SelectionCounts {
    pub objects: usize,
    pub world_brushes: usize,
    pub child_brushes: usize,
    pub child_owners: usize,
    pub brush_entities: usize,
    pub point_entities: usize,
    pub selected_brushes: usize,
    pub faces: usize,
    pub components: usize,
    pub in_region: usize,
    pub hidden_or_outside_region: usize,
    pub unresolved: usize,
}

#[derive(Serialize)]
pub struct SelectionPresentation {
    pub counts: SelectionCounts,
    pub counts_complete: bool,
    pub bounds: Option<[[f64; 3]; 2]>,
    pub center: Option<[f64; 3]>,
    pub dimensions: Option<[f64; 3]>,
    pub preview: bool,
    pub bounds_error: Option<String>,
    pub summary: String,
}

impl Session {
    pub(super) fn displayed_selection(&self) -> (HashSet<ObjectId>, HashSet<FaceId>) {
        let selected: HashSet<ObjectId> = self.geometry.clip_preview.as_ref().map_or_else(
            || self.selection.iter().copied().collect(),
            |preview| preview.result.objects.iter().copied().collect(),
        );
        let selected = self
            .geometry
            .primitives
            .preview
            .as_ref()
            .map_or(selected, |preview| preview.ids.iter().copied().collect());
        let selected = self
            .geometry
            .repeat
            .preview
            .as_ref()
            .map_or(selected, |preview| {
                preview.result.objects().into_iter().collect()
            });
        let selected = self
            .geometry
            .clipboard
            .preview
            .as_ref()
            .map_or(selected, |preview| {
                preview.result.objects.iter().copied().collect()
            });
        let selected = self
            .geometry
            .instance_collapse
            .selection()
            .map_or(selected, |ids| ids.iter().copied().collect());
        let selected = self
            .area_preview
            .as_ref()
            .map_or(selected.clone(), |preview| {
                self.grouped_preview_selection(&selected, &preview.matches.objects, preview.mode)
            });
        let selected = self
            .entity_placement_preview_id()
            .map_or(selected, |id| [id].into_iter().collect());
        let selected_faces: HashSet<FaceId> = self.face_selection.iter().copied().collect();
        let selected_faces = self.area_preview.as_ref().map_or_else(
            || selected_faces.clone(),
            |preview| {
                manipulation::combined_selection(
                    &selected_faces,
                    &preview.matches.faces,
                    preview.mode,
                )
            },
        );
        let (selected, selected_faces) = if let Some(preview) = self.volume_preview() {
            if preview.spec.faces {
                (
                    HashSet::new(),
                    manipulation::combined_selection(
                        &selected_faces,
                        &preview.matches.faces,
                        preview.spec.mode,
                    ),
                )
            } else {
                (
                    self.grouped_preview_selection(
                        &selected,
                        &preview.matches.objects,
                        preview.spec.mode,
                    ),
                    HashSet::new(),
                )
            }
        } else {
            (selected, selected_faces)
        };
        let (selected, selected_faces) = if let Some(stroke) = self.paint_stroke() {
            if stroke.spec.faces {
                (
                    HashSet::new(),
                    manipulation::combined_selection(
                        &selected_faces,
                        &stroke.faces.iter().copied().collect::<Vec<_>>(),
                        stroke.spec.mode,
                    ),
                )
            } else {
                (
                    self.grouped_preview_selection(
                        &selected,
                        &stroke.objects.iter().copied().collect::<Vec<_>>(),
                        stroke.spec.mode,
                    ),
                    HashSet::new(),
                )
            }
        } else {
            (selected, selected_faces)
        };
        let selected = self
            .geometry
            .merging
            .preview
            .as_ref()
            .map_or(selected, |preview| {
                [preview.result.object].into_iter().collect()
            });
        let selected = self
            .geometry
            .carving
            .preview
            .as_ref()
            .map_or(selected, |preview| {
                preview.result.objects.iter().copied().collect()
            });
        let selected = self
            .geometry
            .hollowing
            .preview
            .as_ref()
            .map_or(selected, |preview| {
                preview.result.objects.iter().copied().collect()
            });
        let selected_faces = if self.geometry.primitives.preview.is_some() {
            HashSet::new()
        } else {
            selected_faces
        };
        let selected = if self.geometry.components.selection.is_empty()
            && self.component_picking().is_none()
        {
            selected
        } else {
            HashSet::new()
        };
        (selected, selected_faces)
    }

    fn selection_presentation_for(
        &self,
        document: &Document,
        selected: &HashSet<ObjectId>,
        faces: &HashSet<FaceId>,
        view: ViewOptions,
    ) -> SelectionPresentation {
        let evaluation = self.working_evaluation(document);
        let mut counts = SelectionCounts {
            objects: selected.len(),
            faces: faces.len(),
            components: self.component_selection().len(),
            ..Default::default()
        };
        let presentation_bounds = self.presentation_bounds_in(document, Some(view.rope_preview));
        let mut owners = BTreeSet::new();
        let mut brushes = BTreeSet::new();
        let mut bounds = Vec::new();
        let mut bounds_error = None;
        for &object in selected {
            let ambiguous_entity = matches!(object,ObjectId::Entity(id) if document.vmf().entities.iter().filter(|entity|entity.id == id).count() != 1);
            if ambiguous_entity {
                counts.unresolved += 1;
                counts.hidden_or_outside_region += 1;
                bounds_error =
                    Some("Selection contains an ambiguous or missing entity identity".into());
                continue;
            }
            let mut ordinary_bounds = None;
            let mut reference = false;
            let visible = match object {
                ObjectId::Solid(id) => match map_core::subobject::solid(document, id) {
                    Ok((owner, solid)) => {
                        ordinary_bounds = query::solid_bounds(solid);
                        brushes.insert(id);
                        match owner {
                            ObjectId::Entity(id) => {
                                counts.child_brushes += 1;
                                owners.insert(id);
                            }
                            ObjectId::Solid(_) => counts.world_brushes += 1,
                        }
                        evaluation.solids.contains(&id)
                    }
                    Err(_) => {
                        counts.unresolved += 1;
                        false
                    }
                },
                ObjectId::Entity(id) => match document.entity(id) {
                    Some(entity) => {
                        ordinary_bounds = if entity.solids.is_empty() {
                            query::entity_bounds(entity)
                        } else {
                            entity
                                .solids
                                .iter()
                                .map(query::solid_bounds)
                                .collect::<Option<Vec<_>>>()
                                .and_then(query::union)
                        };
                        reference = entity
                            .classname()
                            .is_some_and(|name| name.eq_ignore_ascii_case("func_instance"));
                        if entity.solids.is_empty() {
                            counts.point_entities += 1;
                        } else {
                            counts.brush_entities += 1;
                            brushes.extend(entity.solids.iter().map(|solid| solid.id));
                        }
                        evaluation.objects.contains(&object)
                    }
                    None => {
                        counts.unresolved += 1;
                        false
                    }
                },
            };
            if visible {
                counts.in_region += 1;
            } else {
                counts.hidden_or_outside_region += 1;
            }
            let object_bounds = if reference && view.instance_contents {
                self.framing_bounds_with_rope_preview(
                    document,
                    &[object],
                    Some(true),
                    Some(view.rope_preview),
                )
            } else {
                presentation_bounds
                    .get(&object)
                    .copied()
                    .or(ordinary_bounds)
            };
            match object_bounds {
                Some(value) => bounds.push(value),
                None => bounds_error = Some("One or more selected objects have unavailable bounds; no partial bounds are shown".into()),
            }
        }
        for &face in faces {
            let solid = surface::solid(document, face.solid);
            let value = solid.and_then(|solid| query::face_bounds(solid, face.side));
            if let Some(value) = value {
                bounds.push(value);
                brushes.insert(face.solid);
            } else {
                counts.unresolved += 1;
                bounds_error = Some("One or more selected faces have unavailable bounds; no partial bounds are shown".into());
            }
            if evaluation.solids.contains(&face.solid) {
                counts.in_region += 1;
            } else {
                counts.hidden_or_outside_region += 1;
            }
        }
        counts.child_owners = owners.len();
        counts.selected_brushes = brushes.len();
        if counts.components > 0 {
            bounds_error = Some("Component handles provide the active component bounds".into());
        }
        let bounds = query::union(bounds).filter(|(low, high)| {
            low.is_finite()
                && high.is_finite()
                && low.cmple(*high).all()
                && low.abs().max_element() <= MAX_COORDINATE
                && high.abs().max_element() <= MAX_COORDINATE
        });
        if bounds.is_none() && (!selected.is_empty() || !faces.is_empty()) && bounds_error.is_none()
        {
            bounds_error = Some("Selection bounds are nonfinite or outside ±65536 units".into());
        }
        let bounds = bounds.filter(|_| bounds_error.is_none());
        let dimensions = bounds.map(|(low, high)| (high - low).to_array());
        let center = bounds.map(|(low, high)| ((low + high) * 0.5).to_array());
        let counts_complete = counts.unresolved == 0;
        let mut summary = if !counts_complete {
            format!(
                "{} selected identities · category totals unavailable",
                selected.len() + faces.len()
            )
        } else if !faces.is_empty() {
            format!(
                "{} faces · {} source brushes",
                counts.faces, counts.selected_brushes
            )
        } else if counts.components > 0 {
            format!("{} components", counts.components)
        } else {
            format!(
                "{} world brushes · {} child brushes in {} owners · {} brush entities · {} point entities",
                counts.world_brushes,
                counts.child_brushes,
                counts.child_owners,
                counts.brush_entities,
                counts.point_entities
            )
        };
        if let Some([x, y, z]) = dimensions {
            summary += &format!("\nSize · X {x:.3} · Y {y:.3} · Z {z:.3}");
        }
        if counts_complete {
            summary += &format!(
                "\nIn region {} · hidden/outside {}",
                counts.in_region, counts.hidden_or_outside_region
            );
        } else {
            summary += "\nVisibility totals unavailable";
        }
        if counts.unresolved > 0 {
            summary += &format!("\n{} unresolved selection identities", counts.unresolved);
        }
        if let Some(error) = &bounds_error {
            summary += &format!("\n{error}");
        }
        SelectionPresentation {
            counts,
            counts_complete,
            bounds: bounds.map(|(low, high)| [low.to_array(), high.to_array()]),
            center,
            dimensions,
            preview: !std::ptr::eq(document, &self.document)
                || self.area_preview.is_some()
                || self.volume_preview().is_some()
                || self.paint_stroke().is_some(),
            bounds_error,
            summary,
        }
    }

    pub fn selection_presentation(&self, view: ViewOptions) -> SelectionPresentation {
        let (selected, faces) = self.displayed_selection();
        self.selection_presentation_for(self.render_document(), &selected, &faces, view)
    }

    pub fn helper_presentation(&self, view: ViewOptions) -> Value {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        let errors = self.helper_presentation_errors(document, view);
        let total = errors.len();
        let diagnostics=errors.iter().take(MAX_DIAGNOSTICS).map(|(id,error)|json!({"entity":crate::identity::id_string(ObjectId::Entity(*id)),"error":error,"in_region":evaluation.objects.contains(&ObjectId::Entity(*id))})).collect::<Vec<_>>();
        json!({"authored_models":view.authored_models,"sprite_resources":view.sprite_resources,"entity_helpers":view.entity_helpers,"entity_images":view.entity_images,"entity_model_guides":view.entity_model_guides,"diagnostics":diagnostics,"reported_errors":total,"omitted":total.saturating_sub(MAX_DIAGNOSTICS),"diagnostic_scope":"Authored model and FGD size errors plus relevant cached image/model-resource failures. Missing optional bindings and unsupported helper forms are not errors here. Resource admission limits retain their per-entity inspection states; no complete map-validation claim.","policy":"Display only. Picking, exact source bounds, placement and selection identities are unchanged. Hidden models retain wire bounds; selected helpers retain outlines; known errors retain amber bounds. Sprite resources are static editor images, without runtime animation/FX. Task/compile/measurement guides remain independent."})
    }

    pub(super) fn append_presentation_guides(
        &self,
        mesh: &mut render::SceneMesh,
        view: ViewOptions,
    ) {
        let document = self.render_document();
        let evaluation = self.working_evaluation(document);
        if !view.authored_models {
            for instance in evaluation.models.instances.iter().filter(|instance| {
                evaluation.objects.contains(&instance.id) && instance.has_geometry()
            }) {
                let (low, high) = instance.bounds();
                mesh.add_bounds_lines(low, high, [130, 155, 180, 255]);
            }
        }
        for id in self.helper_presentation_errors(document, view).into_keys() {
            let object = ObjectId::Entity(id);
            if evaluation.objects.contains(&object)
                && let Some((low, high)) = self.object_bounds_in(document, &[object])
            {
                mesh.add_bounds_lines(low, high, [255, 176, 48, 255]);
            }
        }
        if view.selection_bounds {
            let presentation = self.selection_presentation(view);
            if let Some([low, high]) = presentation.bounds {
                render::presentation::append_selection_bounds(
                    mesh,
                    DVec3::from_array(low),
                    DVec3::from_array(high),
                );
            }
        }
    }
}
