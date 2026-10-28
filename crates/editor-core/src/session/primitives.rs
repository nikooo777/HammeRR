use super::previews;
use glam::DVec3;
use map_core::primitive::{self, PolygonSpec, Spec};
use map_core::{Document, ObjectId, query};
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};

pub struct Preview {
    pub token: u64,
    pub spec: Spec,
    pub document: Document,
    pub ids: Vec<ObjectId>,
    pub bounds: [DVec3; 2],
    generation: u64,
    revision: u64,
}

#[derive(Default)]
pub struct State {
    pub(super) serial: u64,
    pub preview: Option<Preview>,
}

pub fn spec_json(spec: &Spec) -> Value {
    json!({"shape":spec.shape.name(),"mins":spec.mins.to_array(),"maxs":spec.maxs.to_array(),"axis":spec.axis.name(),"segments":spec.segments,"bands":spec.bands,"inset":spec.inset,"start_angle":spec.start_angle,"sweep_angle":spec.sweep_angle,"material":spec.material,"outline":spec.outline.iter().map(|point|point.to_array()).collect::<Vec<_>>()})
}

impl Session {
    pub fn create_polygon(
        &mut self,
        actor: Actor,
        mut spec: PolygonSpec,
        snap: bool,
        preview: bool,
    ) -> Result<Vec<ObjectId>, String> {
        if snap {
            for point in &mut spec.outline {
                *point = (*point / self.spatial.grid_size).round() * self.spatial.grid_size;
            }
        }
        self.create_primitive(
            actor,
            spec.into_spec().map_err(|error| error.to_string())?,
            preview,
        )
    }

    pub fn primitive_serial(&self) -> u64 {
        self.geometry.primitives.serial
    }
    pub fn primitive_preview(&self) -> Option<&Preview> {
        self.geometry.primitives.preview.as_ref()
    }

    pub fn clear_primitive_preview(&mut self) {
        if self.geometry.primitives.preview.take().is_some() {
            self.geometry.primitives.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn cancel_primitive_preview(&mut self, token: u64) -> Result<(), String> {
        self.checked_primitive(token)?;
        self.clear_primitive_preview();
        Ok(())
    }

    fn checked_primitive(&self, token: u64) -> Result<&Preview, String> {
        let preview = self
            .geometry
            .primitives
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Primitive preview was closed or replaced")?;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
        {
            return Err("Map changed; preview the primitive again".into());
        }
        Ok(preview)
    }

    pub fn create_primitive(
        &mut self,
        actor: Actor,
        spec: Spec,
        preview: bool,
    ) -> Result<Vec<ObjectId>, String> {
        if preview {
            if let Some(current) = &self.geometry.primitives.preview
                && current.spec == spec
                && current.generation == self.document_generation
                && current.revision == self.document.revision()
            {
                return Ok(current.ids.clone());
            }
            let mut document = self.document.preview_snapshot();
            let ids = document
                .edit("preview primitive", |tx| primitive::create(tx, &spec))
                .map_err(|error| error.to_string())?;
            let (mins, maxs) =
                query::bounds_of(&document, &ids).ok_or("Primitive has no bounds")?;
            self.clear_geometry_previews_except(&[
                previews::Kind::Primitive,
                previews::Kind::Paint,
            ]);

            self.cancel_block_preview();

            self.cancel_area_selection();
            self.clear_volume_selection();

            self.clip_tool = false;
            self.spatial.draw_block = false;
            self.geometry.primitives.serial += 1;
            self.geometry.primitives.preview = Some(Preview {
                token: self.geometry.primitives.serial,
                spec,
                document,
                ids: ids.clone(),
                bounds: [mins, maxs],
                generation: self.document_generation,
                revision: self.document.revision(),
            });
            self.scene_serial += 1;
            self.view_serial += 1;
            Ok(ids)
        } else {
            let label = format!("create {}", spec.shape.name());
            let ids = self.edit(actor, &label, |tx| primitive::create(tx, &spec))?;
            self.select(&ids, SelectMode::Replace)?;
            Ok(ids)
        }
    }

    pub fn commit_primitive_preview(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Vec<ObjectId>, String> {
        let spec = self.checked_primitive(token)?.spec.clone();
        self.create_primitive(actor, spec, false)
    }

    pub fn frame_primitive_preview(&mut self, token: u64, aspect: f32) -> Result<(), String> {
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("View aspect must be finite and positive".into());
        }
        let bounds = self.checked_primitive(token)?.bounds;
        self.set_camera(
            self.camera
                .frame_bounds(bounds[0].as_vec3(), bounds[1].as_vec3(), aspect),
        );
        Ok(())
    }

    pub fn primitive_state(&self) -> Value {
        let preview = self.geometry.primitives.preview.as_ref().map(|preview| {
            let brushes = preview.ids.iter().filter_map(|id| {
                let ObjectId::Solid(id) = id else { return None };
                let solid = preview.document.solid(*id)?;
                Some(json!({
                    "id": format!("solid:{id}"),
                    "sides": solid.sides.iter().map(|side| side.id).collect::<Vec<_>>(),
                    "face_count": solid.sides.len(),
                    "operation_side_count_supported": solid.sides.len() <= map_core::subobject::MAX_SIDES
                }))
            }).collect::<Vec<_>>();
            json!({"token":preview.token.to_string(),"generation":preview.generation,"revision":preview.revision,"spec":spec_json(&preview.spec),"brushes":brushes,"face_count":brushes.iter().map(|brush|brush["sides"].as_array().unwrap().len()).sum::<usize>(),"bounds":{"mins":preview.bounds[0].to_array(),"maxs":preview.bounds[1].to_array()}})
        });
        let defaults = Spec::default();
        json!({
            "preview": preview,
            "defaults": {"segments": defaults.segments, "bands": defaults.bands},
            "limits": {"segments":primitive::MAX_SEGMENTS,"faces_per_brush":primitive::MAX_FACES,"coordinate":primitive::MAX_COORDINATE,"minimum_size":primitive::MIN_SIZE},
            "operation_limits": {
                "hollow": {"faces_per_brush": map_core::subobject::MAX_SIDES},
                "carve": {"faces_per_brush": map_core::subobject::MAX_SIDES},
                "component_editing": {"faces_per_brush": map_core::subobject::MAX_SIDES}
            },
            "operation_note": "The operation side limit applies to each brush. Geometry, thickness and selection limits are checked when the operation runs; explicit primitive resolution is preserved.",
            "bounds_note":"Curved shapes are inscribed in the supplied bounds. Arch bounds describe the full ellipse; the sweep chooses which part is built."
        })
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.preview
            .as_ref()
            .map(|preview| serde_json::json!([self.serial, preview.token]))
    }
}
