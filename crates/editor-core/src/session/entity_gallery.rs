use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicU64, Ordering},
};

use glam::DVec3;
use map_core::{Document, EditError, ObjectId, edit, query};
use serde_json::{Value, json};
use source_formats::fgd::ClassKind;

use super::{Actor, SelectMode, Session, previews};

pub const MAX_EXAMPLES: usize = 32;
pub const PAGE_SIZE: usize = 32;
const MAX_QUERY_BYTES: usize = 256;
const MIN_GAP: f64 = 16.0;
const MAX_GAP: f64 = 4096.0;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct Example {
    pub classname: String,
    pub properties: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub examples: Vec<Example>,
    pub origin: DVec3,
    pub columns: usize,
    pub gap: f64,
}

pub(super) struct State {
    serial: u64,
    draft: Option<Draft>,
}

struct Draft {
    token: u64,
    owner: Actor,
    generation: u64,
    revision: u64,
    definitions: u64,
    scene: u64,
    request: Request,
    document: Document,
    examples: Vec<Planned>,
    bounds: (DVec3, DVec3),
    cell_size: DVec3,
}

struct Planned {
    id: ObjectId,
    classname: String,
    properties: Vec<(String, String)>,
    origin: DVec3,
    bounds: (DVec3, DVec3),
}

fn unsupported(classname: &str) -> Option<&'static str> {
    match classname.to_ascii_lowercase().as_str() {
        "worldspawn" => Some("World entity cannot be placed as a point example"),
        "info_overlay" | "info_overlay_transition" | "infodecal" => {
            Some("Requires surface-specific authoring; excluded from bulk examples")
        }
        "func_instance" => Some("Requires an explicit instance source; use Instances"),
        _ => None,
    }
}

fn checked_bounds(bounds: (DVec3, DVec3)) -> Result<(DVec3, DVec3), String> {
    if [bounds.0, bounds.1]
        .iter()
        .any(|point| !point.is_finite() || point.abs().max_element() > crate::spatial::MAX_EXTENT)
    {
        return Err("Gallery visual bounds must remain finite and within ±65536 units".into());
    }
    Ok(bounds)
}

fn model_present(document: &Document, id: u32) -> bool {
    document
        .entity(id)
        .and_then(|entity| entity.properties.get("model"))
        .is_some_and(|model| !model.trim().is_empty())
}

fn helper_summary(report: super::fgd_helpers::Report) -> Value {
    let mut report = json!(report);
    if let Some(fields) = report.as_object_mut() {
        for key in [
            "limits",
            "sphere_limits",
            "line_limits",
            "frustum_limits",
            "projector_limits",
            "icon_limits",
            "sprite_resource_limits",
            "model_guide_limits",
        ] {
            fields.remove(key);
        }
    }
    report
}

impl Session {
    pub fn entity_gallery_snapshot(&self) -> String {
        format!(
            "gallery:{}:{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.definitions_serial,
            self.scene_serial,
            self.geometry.entity_gallery.serial
        )
    }

    pub fn browse_entity_gallery(&self, search: &str, page: usize) -> Result<Value, String> {
        if search.len() > MAX_QUERY_BYTES {
            return Err("Gallery search must be at most 256 bytes".into());
        }
        let search = search.to_ascii_lowercase();
        let classes = self
            .definitions
            .classes()
            .filter(|class| {
                class.kind == ClassKind::Point
                    && (class.name.to_ascii_lowercase().contains(&search)
                        || class.description.to_ascii_lowercase().contains(&search))
            })
            .collect::<Vec<_>>();
        let start = page
            .checked_mul(PAGE_SIZE)
            .ok_or("Gallery page is too large")?;
        if start > classes.len() {
            return Err("Gallery page is outside the filtered catalog".into());
        }
        let rows = classes.iter().skip(start).take(PAGE_SIZE).map(|class| {
            json!({"classname":class.name,"description":class.description,
                "placeable":unsupported(&class.name).is_none(),"reason":unsupported(&class.name),
                "defaults":class.defaults(),"helpers":class.helpers.iter().map(|helper|json!({"name":helper.name,"arguments":helper.arguments})).collect::<Vec<_>>()})
        }).collect::<Vec<_>>();
        Ok(
            json!({"snapshot":self.entity_gallery_snapshot(),"page":page,"page_size":PAGE_SIZE,
            "total":classes.len(),"has_next":start.saturating_add(rows.len()) < classes.len(),
            "classes":rows,"definitions_error":self.definitions_error(),
            "max_examples":MAX_EXAMPLES,"limits":"Loaded FGD point classes only. No runtime simulation. Select at most 32 classes per reviewed batch; missing assets retain explicit helper diagnostics."}),
        )
    }

    fn entity_gallery_context(&self) -> Result<(), String> {
        self.ensure_not_closing()?;
        let own = self
            .geometry
            .entity_gallery
            .document()
            .is_some_and(|document| std::ptr::eq(self.render_document(), document));
        if !std::ptr::eq(self.render_document(), &self.document) && !own {
            return Err(
                "Apply or cancel the geometry preview before opening a gallery preview".into(),
            );
        }
        Ok(())
    }

    fn checked_entity_gallery(&self, token: u64) -> Result<&Draft, String> {
        let draft = self
            .geometry
            .entity_gallery
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Entity gallery preview was closed or replaced")?;
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
            || draft.definitions != self.definitions_serial
            || draft.scene != self.scene_serial
        {
            return Err("Map, definitions or scene changed; preview the gallery again".into());
        }
        self.entity_gallery_context()?;
        Ok(draft)
    }

    fn owned_entity_gallery(&self, actor: Actor, token: u64) -> Result<&Draft, String> {
        let draft = self.checked_entity_gallery(token)?;
        if draft.owner != actor {
            return Err("Review the other actor's entity gallery before changing it".into());
        }
        Ok(draft)
    }

    pub fn get_entity_gallery(&self) -> Value {
        let draft = self.geometry.entity_gallery.draft.as_ref().map(|draft| {
            let error = self.checked_entity_gallery(draft.token).err();
            let evaluation = self.working_evaluation(&draft.document);
            let models = &evaluation.models;
            let examples = draft.examples.iter().enumerate().map(|(index, example)| {
                let ObjectId::Entity(id) = example.id else { unreachable!() };
                let (helper, helper_error) = match self.entity_helper_report(id) {
                    Ok(helper) => (Some(helper_summary(helper)), None),
                    Err(error) => (None, Some(error)),
                };
                let needs_model = self.definitions.class(&example.classname).is_some_and(|class| class.property("model").is_some());
                let model_state = if model_present(&draft.document, id) { if models.issues.contains_key(&id) { "unavailable" } else { "configured" } } else if needs_model { "not_configured" } else { "not_required" };
                let model = models.instances.iter().find(|model| model.id == example.id && model.has_geometry());
                json!({"index":index,"id":crate::identity::id_string(example.id),
                    "classname":example.classname,"origin":example.origin.to_array(),
                    "row":index / draft.request.columns,"column":index % draft.request.columns,
                    "properties":draft.document.entity(id).map(|entity| &entity.properties.0),
                    "bounds":{"mins":example.bounds.0.to_array(),"maxs":example.bounds.1.to_array()},
                    "visible":evaluation.objects.contains(&example.id),"model_displayed":model.is_some() && evaluation.objects.contains(&example.id),"model_state":model_state,"model_error":models.issues.get(&id),"helper":helper,"helper_error":helper_error})
            }).collect::<Vec<_>>();
            json!({"token":draft.token,"owner":draft.owner.label(),"valid":error.is_none(),"error":error,
                "generation":draft.generation,"revision":draft.revision,"definitions":draft.definitions,
                "origin":draft.request.origin.to_array(),"columns":draft.request.columns,"gap":draft.request.gap,
                "cell_size":draft.cell_size.to_array(),"examples":examples,
                "bounds":{"mins":draft.bounds.0.to_array(),"maxs":draft.bounds.1.to_array()}})
        });
        json!({"snapshot":self.entity_gallery_snapshot(),"draft":draft,"max_examples":MAX_EXAMPLES,
            "limits":"Row-major XY example arrangement. Editor helpers and mounted static/reference-pose models/icons use existing view switches and shared asset budgets. Missing/unsupported visuals retain helper diagnostics. Spheres, lines and projectors may overlap cells. No collision resolution, runtime effects or game-safety guarantee; review examples before placing."})
    }

    pub fn preview_entity_gallery(
        &mut self,
        actor: Actor,
        request: Request,
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.entity_gallery_snapshot() {
            return Err("Gallery context changed; inspect the gallery again".into());
        }
        self.entity_gallery_context()?;
        self.inspection_viewport_ready("previewing the gallery")?;
        if self
            .geometry
            .entity_gallery
            .draft
            .as_ref()
            .is_some_and(|draft| draft.owner != actor)
        {
            return Err("Review the other actor's entity gallery before replacing it".into());
        }
        if request.examples.is_empty() || request.examples.len() > MAX_EXAMPLES {
            return Err("Choose from 1 to 32 point-entity classes".into());
        }
        if request.columns == 0 || request.columns > MAX_EXAMPLES {
            return Err("Gallery columns must be from 1 to 32".into());
        }
        if !request.gap.is_finite() || !(MIN_GAP..=MAX_GAP).contains(&request.gap) {
            return Err("Gallery gap must be finite and from 16 to 4096 units".into());
        }
        checked_bounds((request.origin, request.origin))?;
        let mut classes = BTreeSet::new();
        let mut planned = Vec::new();
        let mut document = self.document.preview_snapshot();
        for example in &request.examples {
            let class = self
                .definitions
                .class(&example.classname)
                .filter(|class| class.kind == ClassKind::Point)
                .ok_or_else(|| format!("{} is not a loaded FGD point class", example.classname))?;
            if let Some(reason) = unsupported(&class.name) {
                return Err(format!("{}: {reason}", class.name));
            }
            if !classes.insert(class.name.to_ascii_lowercase()) {
                return Err(format!("Duplicate gallery class {}", class.name));
            }
            if example
                .properties
                .keys()
                .any(|key| key.eq_ignore_ascii_case("origin"))
            {
                return Err(
                    "Gallery positions come from the arrangement; remove origin overrides".into(),
                );
            }
            let (classname, properties) =
                self.entity_properties(&class.name, ClassKind::Point, example.properties.clone())?;
            let id = document
                .edit("preview entity gallery example", |tx| {
                    edit::create_entity(
                        tx,
                        &classname,
                        Some(request.origin),
                        &properties,
                        Vec::new(),
                    )
                })
                .map_err(|error| error.to_string())?;
            planned.push(Planned {
                id,
                classname,
                properties,
                origin: DVec3::ZERO,
                bounds: (DVec3::ZERO, DVec3::ZERO),
            });
        }
        let evaluation = self.working_evaluation(&document);
        let guides = self.fgd_model_guides(&document, &evaluation, true);
        let images = self.fgd_icon_instances(
            &document,
            &evaluation,
            true,
            self.view.additive_sprite_guides,
        );
        let mut local = Vec::new();
        let mut cell_size = DVec3::ZERO;
        for example in &planned {
            let mut bounds = self
                .object_bounds_in(&document, &[example.id])
                .ok_or("Example has no finite helper bounds")?;
            bounds = (bounds.0 - request.origin, bounds.1 - request.origin);
            for guide in guides.iter().filter(|guide| guide.id == example.id) {
                let visual = guide.bounds();
                bounds = (
                    bounds.0.min(visual.0 - request.origin),
                    bounds.1.max(visual.1 - request.origin),
                );
            }
            let ObjectId::Entity(id) = example.id else {
                unreachable!()
            };
            if let Some(image) = images.get(&id) {
                let radius = f64::from(image.world_size[0].max(image.world_size[1]));
                bounds = (
                    bounds.0.min(DVec3::splat(-radius)),
                    bounds.1.max(DVec3::splat(radius)),
                );
            }
            checked_bounds(bounds)?;
            cell_size = cell_size.max((bounds.1 - bounds.0) + DVec3::splat(request.gap));
            local.push(bounds);
        }
        for (index, (example, bounds)) in planned.iter_mut().zip(local).enumerate() {
            let cell = DVec3::new(
                (index % request.columns) as f64 * cell_size.x,
                (index / request.columns) as f64 * cell_size.y,
                0.0,
            );
            let center = (bounds.0 + bounds.1) * 0.5;
            let origin = request.origin + cell - DVec3::new(center.x, center.y, bounds.0.z);
            checked_bounds((origin, origin))?;
            example.bounds = checked_bounds((bounds.0 + origin, bounds.1 + origin))?;
            example.origin = origin;
            document
                .edit("arrange entity gallery example", |tx| {
                    let ObjectId::Entity(id) = example.id else {
                        unreachable!()
                    };
                    edit::set_properties(
                        tx,
                        id,
                        &[(
                            "origin".into(),
                            Some(source_formats::vmf::format_vec3(origin)),
                        )],
                    )
                })
                .map_err(|error| error.to_string())?;
        }
        let bounds =
            query::union(planned.iter().map(|example| example.bounds)).ok_or("Gallery is empty")?;
        let evaluation = self.working_evaluation(&document);
        for example in &planned {
            if !evaluation.objects.contains(&example.id) {
                return Err(format!(
                    "{} is outside the working region or hidden; move the gallery origin or show that region",
                    example.classname
                ));
            }
        }
        self.clear_geometry_previews_except(&[previews::Kind::EntityGallery]);
        self.cancel_block_preview();
        self.cancel_area_selection();
        self.clear_volume_selection();
        self.geometry.entity_gallery.serial = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        self.scene_serial += 1;
        self.view_serial += 1;
        self.geometry.entity_gallery.draft = Some(Draft {
            token: self.geometry.entity_gallery.serial,
            owner: actor,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            scene: self.scene_serial,
            request,
            document,
            examples: planned,
            bounds,
            cell_size,
        });
        Ok(self.get_entity_gallery())
    }

    pub fn review_entity_gallery(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.checked_entity_gallery(token)?;
        self.geometry.entity_gallery.serial = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let draft = self.geometry.entity_gallery.draft.as_mut().unwrap();
        draft.token = self.geometry.entity_gallery.serial;
        draft.owner = actor;
        self.view_serial += 1;
        Ok(self.get_entity_gallery())
    }

    pub fn frame_entity_gallery(
        &mut self,
        token: u64,
        index: Option<usize>,
        aspect: f32,
    ) -> Result<(), String> {
        super::inspection_framing::validate_aspect(aspect)?;
        self.inspection_viewport_ready("framing the gallery")?;
        let draft = self.checked_entity_gallery(token)?;
        let bounds = match index {
            Some(index) => {
                draft
                    .examples
                    .get(index)
                    .ok_or("Gallery example index is out of range")?
                    .bounds
            }
            None => draft.bounds,
        };
        let camera = self.inspection_frame_camera(
            bounds.0.as_vec3(),
            bounds.1.as_vec3(),
            aspect,
            "Gallery",
        )?;
        self.set_camera(camera);
        Ok(())
    }

    pub fn apply_entity_gallery(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self.owned_entity_gallery(actor, token)?;
        let expected = draft
            .examples
            .iter()
            .map(|example| {
                let ObjectId::Entity(id) = example.id else {
                    unreachable!()
                };
                (
                    example.classname.clone(),
                    example.properties.clone(),
                    example.origin,
                    example.id,
                    draft.document.entity(id).unwrap().clone(),
                )
            })
            .collect::<Vec<_>>();
        let ids = self.edit(actor, "place entity gallery", |tx| {
            let mut ids = Vec::new();
            for (classname, properties, origin, expected_id, expected_entity) in &expected {
                let id = edit::create_entity(tx, classname, Some(*origin), properties, Vec::new())?;
                if id != *expected_id
                    || tx
                        .vmf()
                        .entities
                        .iter()
                        .find(|entity| entity.id == expected_entity.id)
                        != Some(expected_entity)
                {
                    return Err(EditError::Invalid(
                        "Gallery replay changed; preview again".into(),
                    ));
                }
                ids.push(id);
            }
            Ok(ids)
        })?;
        self.select(&ids, SelectMode::Replace)?;
        Ok(
            json!({"created":ids.into_iter().map(crate::identity::id_string).collect::<Vec<_>>(),"state":self.get_entity_gallery()}),
        )
    }

    pub fn cancel_entity_gallery(&mut self, actor: Actor, token: u64) -> Result<(), String> {
        let draft = self
            .geometry
            .entity_gallery
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Entity gallery preview was closed or replaced")?;
        if draft.owner != actor {
            return Err("Review the other actor's entity gallery before cancelling it".into());
        }
        self.clear_entity_gallery();
        Ok(())
    }

    pub(super) fn clear_entity_gallery(&mut self) {
        if self.geometry.entity_gallery.draft.take().is_some() {
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn ensure_entity_gallery_resolved(&self) -> Result<(), String> {
        if self.geometry.entity_gallery.draft.is_some() {
            return Err(
                "Apply or cancel the entity gallery before saving, compiling or playing".into(),
            );
        }
        Ok(())
    }

    pub(super) fn append_entity_gallery_guides(&self, mesh: &mut render::SceneMesh) {
        let Some(draft) = &self.geometry.entity_gallery.draft else {
            return;
        };
        if self.checked_entity_gallery(draft.token).is_err() {
            return;
        }
        for example in &draft.examples {
            mesh.add_bounds_lines(example.bounds.0, example.bounds.1, [80, 200, 255, 255]);
        }
    }
}

impl State {
    pub(super) fn document(&self) -> Option<&Document> {
        self.draft.as_ref().map(|draft| &draft.document)
    }

    pub(super) fn close_draft(&self) -> Option<Value> {
        self.draft.as_ref().map(|draft| {
            json!([
                self.serial,
                draft.token,
                draft.owner.label(),
                draft.examples.len()
            ])
        })
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            serial: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            draft: None,
        }
    }
}
