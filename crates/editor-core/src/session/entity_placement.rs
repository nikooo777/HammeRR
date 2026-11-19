use std::collections::BTreeMap;

use compiler::decals;
use glam::DVec3;
use map_core::{
    Document, EditError, ObjectId, edit,
    query::{self, Ray, SurfaceHit},
    surface::{self, FaceId},
};
use serde_json::{Value, json};
use source_formats::{
    bsp::{contents, surf},
    fgd::ClassKind,
    vmt::Material,
};

use super::{
    Actor, SelectMode, Session,
    placement::{self, Method, Options},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub classname: String,
    pub properties: BTreeMap<String, String>,
    pub offset: f64,
    pub align: bool,
    pub snap: bool,
}

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    draft: Option<Draft>,
}

struct Draft {
    token: u64,
    owner: Actor,
    generation: u64,
    revision: u64,
    scene: u64,
    tools: bool,
    selection: Vec<ObjectId>,
    request: Request,
    placement: Option<Placement>,
}

struct Placement {
    classname: String,
    properties: Vec<(String, String)>,
    origin: DVec3,
    transform: Option<edit::Transform>,
    surface: SurfaceHit,
    picked: DVec3,
    id: ObjectId,
    document: Document,
    bounds: (DVec3, DVec3),
    support_points: usize,
    bounds_source: &'static str,
    model_error: Option<String>,
    contact: f64,
}

const SURFACE_EPSILON: f64 = 1e-5;
const SNAP_PROBE_DISTANCE: f64 = 0.5;

impl Session {
    pub(super) fn entity_surface_picking(&self) -> bool {
        self.geometry
            .entity_placement
            .draft
            .as_ref()
            .is_some_and(|draft| {
                draft.placement.is_none() && self.checked_entity_placement(draft.token).is_ok()
            })
    }

    pub fn entity_placement_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{:?}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.scene_serial,
            self.geometry.entity_placement.serial,
            self.selected_ids(),
            self.spatial.grid_size,
            self.spatial.snap,
            self.view.tool_textures
        )
    }

    fn entity_placement_context(&self) -> Result<(), String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        let own = self
            .geometry
            .entity_placement
            .draft
            .as_ref()
            .and_then(|draft| draft.placement.as_ref())
            .is_some_and(|placement| std::ptr::eq(self.render_document(), &placement.document));
        if !std::ptr::eq(self.render_document(), &self.document) && !own {
            return Err("Apply or cancel the geometry preview before placing an entity".into());
        }
        Ok(())
    }

    fn prepare_entity_placement_request(&self, request: &mut Request) -> Result<(), String> {
        let (classname, properties) = self.entity_properties(
            &request.classname,
            ClassKind::Point,
            request.properties.clone(),
        )?;
        if !request.offset.is_finite() || request.offset.abs() > crate::spatial::MAX_EXTENT {
            return Err("Surface offset must be finite and within ±65536 units".into());
        }
        if ["info_overlay", map_core::overlay::water::CLASSNAME]
            .iter()
            .any(|class| classname.trim().eq_ignore_ascii_case(class))
        {
            return Err("Use Overlay… to place overlays and water transitions".into());
        }
        if decals::is_class(&classname) {
            if request.offset != 0.0 || request.align {
                return Err(
                    "Decal placement uses the surface point without offset or alignment".into(),
                );
            }
            if request
                .properties
                .keys()
                .any(|key| key.eq_ignore_ascii_case("origin"))
                || properties.iter().any(|(key, value)| {
                    key.eq_ignore_ascii_case("parentname") && !value.trim().is_empty()
                })
            {
                return Err("Decal placement derives a world origin from the picked surface; remove origin and parenting overrides".into());
            }
            let mut textures = request
                .properties
                .keys()
                .filter(|key| key.eq_ignore_ascii_case(decals::TEXTURE_KEY));
            if textures.nth(1).is_some() {
                return Err("Decal placement requires exactly one texture property".into());
            }
            let texture = properties
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(decals::TEXTURE_KEY))
                .map(|(_, value)| value.as_str())
                .ok_or("Enter a decal material in the texture property")?;
            let texture = decals::texture_name(texture)?;
            decals::validate_placement_material(self.game(), &texture)?;
            request.classname = decals::CLASSNAME.into();
            request
                .properties
                .retain(|key, _| !key.eq_ignore_ascii_case(decals::TEXTURE_KEY));
            request
                .properties
                .insert(decals::TEXTURE_KEY.into(), texture);
        }
        Ok(())
    }

    fn decal_placement_origin(&self, hit: SurfaceHit) -> Result<DVec3, String> {
        let unsupported = "Pick a visible solid world or func_detail brush face for this decal";
        let solid = hit.hit.solid.ok_or(unsupported)?;
        let side = hit.hit.side.ok_or(unsupported)?;
        if hit.terrain_vertex.is_some() {
            return Err(unsupported.into());
        }
        if let ObjectId::Entity(id) = hit.hit.object
            && self
                .document
                .entity(id)
                .and_then(|entity| entity.classname())
                != Some("func_detail")
        {
            return Err(unsupported.into());
        }
        let face = surface::side(&self.document, FaceId { solid, side }).ok_or(unsupported)?;
        let material = Material::load_limited(self.game(), &face.material, 64 * 1024)
            .map_err(|error| format!("Decal target material {}: {error}", face.material))?
            .ok_or_else(|| format!("Decal target material {} was not found", face.material))?;
        let semantics = material.compile_semantics();
        if material.is_tool()
            || semantics.surface & (surf::NODRAW | surf::NODECALS) != 0
            || semantics.contents & (contents::SOLID | contents::WINDOW | contents::GRATE) == 0
        {
            return Err(unsupported.into());
        }
        let origin = hit.hit.point;
        if !origin.is_finite()
            || origin.abs().max_element() > map_core::checks::SOURCE_COORDINATE_LIMIT
        {
            return Err("Decal origin must remain within Source's compile bounds".into());
        }
        Ok(origin)
    }

    fn checked_entity_placement(&self, token: u64) -> Result<&Draft, String> {
        let draft = self
            .geometry
            .entity_placement
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Entity placement was closed or replaced")?;
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
            || draft.scene != self.scene_serial
            || draft.selection != self.selected_ids()
            || draft.tools != self.view.tool_textures
        {
            return Err("Map, selection or scene changed; pick the entity surface again".into());
        }
        self.entity_placement_context()?;
        Ok(draft)
    }

    fn owned_entity_placement(&self, actor: Actor, token: u64) -> Result<&Draft, String> {
        let draft = self.checked_entity_placement(token)?;
        if draft.owner != actor {
            return Err("Review the other actor's entity placement before changing it".into());
        }
        Ok(draft)
    }

    pub fn get_entity_placement(&self) -> Value {
        let draft = self.geometry.entity_placement.draft.as_ref().map(|draft| {
            let error = self.checked_entity_placement(draft.token).err();
            let plan = draft.placement.as_ref().map(|plan| {
                let ObjectId::Entity(id) = plan.id else { unreachable!() };
                let entity = plan.document.entity(id).unwrap();
                json!({"entity":crate::identity::id_string(plan.id),"origin":entity.origin().unwrap().to_array(),
                    "properties":entity.properties.0,"angles":entity.properties.get("angles"),
                    "angle":entity.properties.get("angle"),"pitch":entity.properties.get("pitch"),"target":crate::identity::id_string(plan.surface.hit.object),
                    "decal_texture":decals::is_class(&plan.classname).then(|| entity.properties.get(decals::TEXTURE_KEY)).flatten(),
                    "solid":plan.surface.hit.solid,"side":plan.surface.hit.side,"picked":plan.picked.to_array(),
                    "point":plan.surface.hit.point.to_array(),"normal":plan.surface.normal.to_array(),
                    "bounds":{"mins":plan.bounds.0.to_array(),"maxs":plan.bounds.1.to_array()},
                    "support_points":plan.support_points,"bounds_source":plan.bounds_source,"model_error":plan.model_error,"contact_offset":plan.contact})
            });
            json!({"token":draft.token,"owner":draft.owner.label(),"generation":draft.generation,
                "revision":draft.revision,"valid":error.is_none(),"picking":error.is_none() && plan.is_none(),
                "classname":draft.request.classname,"properties":draft.request.properties,
                "offset":draft.request.offset,"align":draft.request.align,"snap":draft.request.snap,
                "plan":plan,"error":error})
        });
        json!({"snapshot":self.entity_placement_stamp(),"generation":self.document_generation,
            "revision":self.document.revision(),"grid_size":self.spatial.grid_size,"draft":draft})
    }

    pub fn begin_entity_placement(
        &mut self,
        actor: Actor,
        mut request: Request,
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.entity_placement_stamp() {
            return Err("Entity placement context changed; inspect it again".into());
        }
        self.entity_placement_context()?;
        if let Some(draft) = &self.geometry.entity_placement.draft
            && draft.owner != actor
        {
            return Err("Review the other actor's entity placement before changing it".into());
        }
        self.prepare_entity_placement_request(&mut request)?;
        self.clear_geometry_previews_except(&[super::previews::Kind::EntityPlacement]);
        self.cancel_block_preview();
        self.cancel_area_selection();
        self.clear_volume_selection();
        self.spatial.draw_block = false;
        self.clip_tool = false;
        self.geometry.entity_placement.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        self.geometry.entity_placement.draft = Some(Draft {
            token: self.geometry.entity_placement.serial,
            owner: actor,
            generation: self.document_generation,
            revision: self.document.revision(),
            scene: self.scene_serial,
            tools: self.view.tool_textures,
            selection: self.selected_ids(),
            request,
            placement: None,
        });
        Ok(self.get_entity_placement())
    }

    fn snapped_entity_surface(&mut self, mut surface: SurfaceHit) -> Result<SurfaceHit, String> {
        let normal = surface.normal;
        let mut point =
            (surface.hit.point / self.spatial.grid_size).round() * self.spatial.grid_size;
        let axis = (0..3)
            .max_by(|a, b| normal[*a].abs().total_cmp(&normal[*b].abs()))
            .unwrap();
        point[axis] += normal.dot(surface.hit.point - point) / normal[axis];
        let checked = self.placement_surface(
            &[],
            Ray {
                origin: point + normal * SNAP_PROBE_DISTANCE,
                direction: -normal,
            },
            false,
        )?;
        if checked.hit.object != surface.hit.object
            || checked.hit.solid != surface.hit.solid
            || checked.hit.side != surface.hit.side
            || !checked.hit.point.abs_diff_eq(point, SURFACE_EPSILON)
            || !checked.normal.abs_diff_eq(normal, SURFACE_EPSILON)
        {
            return Err("The snapped point leaves the picked surface; disable snapping or pick farther from its edge".into());
        }
        surface.hit.point = checked.hit.point;
        Ok(surface)
    }

    pub fn pick_entity_placement(
        &mut self,
        actor: Actor,
        token: u64,
        ray: Ray,
    ) -> Result<Value, String> {
        let draft = self.owned_entity_placement(actor, token)?;
        if draft.placement.is_some() {
            return Err("Start Pick surface again to choose another entity target".into());
        }
        let request = draft.request.clone();
        let (classname, properties) = self.entity_properties(
            &request.classname,
            ClassKind::Point,
            request.properties.clone(),
        )?;
        let mut surface = self.placement_surface(&[], ray, false)?;
        let picked = surface.hit.point;
        if request.snap {
            surface = self.snapped_entity_surface(surface)?;
        }
        let decal = decals::is_class(&classname);
        let origin = if decal {
            self.decal_placement_origin(surface)?
        } else {
            DVec3::ZERO
        };
        let mut document = self.document.preview_snapshot();
        let id = document
            .edit("preview point entity", |tx| {
                edit::create_entity(tx, &classname, Some(origin), &properties, Vec::new())
            })
            .map_err(|error| error.to_string())?;
        let (transform, bounds_source, model_error, support_points, contact) = if decal {
            (None, "decal_origin", None, 1, 0.0)
        } else {
            let models = self.model_instances(&document);
            let supported = models
                .instances
                .iter()
                .any(|instance| instance.id == id && instance.has_geometry());
            let ObjectId::Entity(number) = id else {
                unreachable!()
            };
            let model_error = models.issues.get(&number).cloned();
            let points = self.placement_points_in(&document, &[id])?;
            let up = query::local_basis(&document, id)
                .ok_or("Entity has no valid Source orientation")?
                .z_axis;
            let plan = placement::contact_plan(
                points,
                up,
                Method::Ray,
                ray,
                surface,
                &Options {
                    align: request.align,
                    offset: request.offset,
                    texture_lock: false,
                    source_corner: None,
                },
                Some(false),
            )?;
            document
                .edit("preview entity contact", |tx| {
                    edit::transform(tx, &[id], plan.transform)
                })
                .map_err(|error| error.to_string())?;
            (
                Some(plan.transform),
                if supported {
                    "model_mesh"
                } else {
                    "editor_helper"
                },
                model_error,
                plan.support_points,
                plan.contact,
            )
        };
        let bounds = self
            .object_bounds_in(&document, &[id])
            .ok_or("Entity has no finite bounds")?;
        if [bounds.0, bounds.1].iter().any(|point| {
            !point.is_finite() || point.abs().max_element() > crate::spatial::MAX_EXTENT
        }) {
            return Err("Placed entity bounds must remain within ±65536 units".into());
        }
        let draft = self.geometry.entity_placement.draft.as_mut().unwrap();
        draft.placement = Some(Placement {
            classname,
            properties,
            origin,
            transform,
            surface,
            picked,
            id,
            document,
            bounds,
            support_points,
            bounds_source,
            model_error,
            contact,
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        self.geometry.entity_placement.draft.as_mut().unwrap().scene = self.scene_serial;
        Ok(self.get_entity_placement())
    }

    pub fn review_entity_placement(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.geometry
            .entity_placement
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Entity placement was closed or replaced")?;
        self.entity_placement_context()?;
        self.geometry.entity_placement.serial += 1;
        let draft = self.geometry.entity_placement.draft.as_mut().unwrap();
        draft.owner = actor;
        draft.token = self.geometry.entity_placement.serial;
        self.view_serial += 1;
        Ok(self.get_entity_placement())
    }

    pub fn apply_entity_placement(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self.owned_entity_placement(actor, token)?;
        let plan = draft
            .placement
            .as_ref()
            .ok_or("Pick a surface before applying entity placement")?;
        let classname = plan.classname.clone();
        let properties = plan.properties.clone();
        let origin = plan.origin;
        let transform = plan.transform;
        let expected_id = plan.id;
        let ObjectId::Entity(expected_number) = expected_id else {
            unreachable!()
        };
        let expected = plan.document.entity(expected_number).unwrap().clone();
        let id = self.edit(actor, &format!("place {classname} on surface"), |tx| {
            let id = edit::create_entity(tx, &classname, Some(origin), &properties, Vec::new())?;
            if let Some(transform) = transform {
                edit::transform(tx, &[id], transform)?;
            }
            if id != expected_id
                || tx
                    .vmf()
                    .entities
                    .iter()
                    .find(|entity| entity.id == expected_number)
                    != Some(&expected)
            {
                return Err(EditError::Invalid(
                    "Entity placement replay changed; preview again".into(),
                ));
            }
            Ok(id)
        })?;
        self.select(&[id], SelectMode::Replace)?;
        Ok(json!({"created":crate::identity::id_string(id),"state":self.get_entity_placement()}))
    }

    pub fn cancel_entity_placement(&mut self, actor: Actor, token: u64) -> Result<(), String> {
        let draft = self
            .geometry
            .entity_placement
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Entity placement was closed or replaced")?;
        if draft.owner != actor {
            return Err("Review the other actor's entity placement before cancelling it".into());
        }
        self.clear_entity_placement();
        Ok(())
    }

    pub(super) fn clear_entity_placement(&mut self) {
        if self.geometry.entity_placement.draft.take().is_some() {
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub(super) fn entity_placement_preview_id(&self) -> Option<ObjectId> {
        Some(
            self.geometry
                .entity_placement
                .draft
                .as_ref()?
                .placement
                .as_ref()?
                .id,
        )
    }

    pub(super) fn append_entity_placement_guide(&self, mesh: &mut render::SceneMesh) {
        let Some(draft) = self.geometry.entity_placement.draft.as_ref() else {
            return;
        };
        if self.checked_entity_placement(draft.token).is_err() {
            return;
        }
        let Some(plan) = &draft.placement else {
            return;
        };
        mesh.add_bounds_lines(plan.bounds.0, plan.bounds.1, [80, 200, 255, 255]);
        for point in [
            plan.surface.hit.point,
            plan.surface.hit.point + plan.surface.normal * 24.0,
        ] {
            mesh.lines.push(render::scene::LineVertex {
                position: point.as_vec3().to_array(),
                color: [255, 220, 95, 255],
            });
        }
    }

    pub fn ensure_entity_placement_resolved(&self) -> Result<(), String> {
        self.ensure_rope_placement_resolved()?;
        if self.geometry.entity_placement.draft.is_some() {
            return Err(
                "Apply or cancel entity placement before saving, compiling or playing".into(),
            );
        }
        Ok(())
    }
}

impl State {
    pub(super) fn has_draft(&self) -> bool {
        self.draft.is_some()
    }

    pub(super) fn document(&self) -> Option<&Document> {
        self.draft
            .as_ref()?
            .placement
            .as_ref()
            .map(|plan| &plan.document)
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.draft.as_ref().map(|draft| {
            serde_json::json!([
                self.serial,
                draft.token,
                draft.owner.label(),
                draft.placement.is_some()
            ])
        })
    }
}
