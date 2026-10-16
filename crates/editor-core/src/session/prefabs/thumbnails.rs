use std::{cell::RefCell, collections::VecDeque, sync::Arc};

use glam::Vec3;
use map_core::transfer;
use render::{Camera, MaterialLibrary, SceneMesh, SurfaceView, ViewOptions};
use serde_json::{Value, json};
use source_formats::vtf::RgbaImage;

use super::{Session, prefabs};
use crate::model_preview::PreviewRenderer;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const MAX_CACHE: usize = 8;
const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
const MAX_BRUSHES: usize = 128;
const MAX_SIDES: usize = 1024;
const MAX_BRUSH_SIDES: usize = 64;
const MAX_DISPLACEMENT_VERTICES: usize = 65_536;
const MAX_MODEL_PATHS: usize = 8;
const MAX_MODEL_INSTANCES: usize = 16;
const MAX_VERTICES: usize = 200_000;
const MAX_INDICES: usize = 600_000;
const MAX_MATERIALS: usize = 64;

#[derive(Clone, Debug)]
pub struct Thumbnail {
    pub image: Arc<RgbaImage>,
    pub report: Value,
}

struct Cached {
    source: prefabs::Entry,
    text: String,
    epoch: (u64, u64),
    thumbnail: Thumbnail,
}

#[derive(Default)]
pub(super) struct State {
    cache: RefCell<VecDeque<Cached>>,
    renderer: PreviewRenderer,
    error: Option<String>,
}

struct Source {
    entry: prefabs::Entry,
    text: String,
}

struct Prepared {
    mesh: SceneMesh,
    materials: MaterialLibrary,
    camera: Camera,
    report: Value,
}

fn limits() -> Value {
    json!({"source_bytes":MAX_SOURCE_BYTES,"brushes":MAX_BRUSHES,"sides":MAX_SIDES,"sides_per_brush":MAX_BRUSH_SIDES,"displacement_vertices":MAX_DISPLACEMENT_VERTICES,"model_paths":MAX_MODEL_PATHS,"model_instances":MAX_MODEL_INSTANCES,"vertices":MAX_VERTICES,"indices":MAX_INDICES,"materials":MAX_MATERIALS,"cached_images":MAX_CACHE})
}

impl Session {
    fn thumbnail_source(&self, name: &str) -> Result<Source, String> {
        prefabs::name(name)?;
        let store = self.prefabs.store.as_ref().map_err(Clone::clone)?;
        let snapshot = store.snapshot()?;
        let entry = snapshot
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or("Prefab no longer exists; refresh the library")?;
        if entry.bytes > MAX_SOURCE_BYTES as u64 {
            return Err(
                "Prefab thumbnail source exceeds 2 MiB; placement remains available".into(),
            );
        }
        let text = store.read(name, &snapshot.stamp)?;
        Ok(Source {
            entry: entry.clone(),
            text,
        })
    }

    fn cached_thumbnail(&self, source: &Source) -> Option<Thumbnail> {
        let epoch = (self.content_epoch, self.definitions_serial);
        let mut cache = self.prefabs.thumbnails.cache.borrow_mut();
        cache.retain(|cached| {
            cached.epoch == epoch
                && (cached.source.name != source.entry.name
                    || (cached.source == source.entry && cached.text == source.text))
        });
        let index = cache
            .iter()
            .position(|cached| cached.source == source.entry && cached.text == source.text)?;
        let cached = cache.remove(index)?;
        let mut thumbnail = cached.thumbnail.clone();
        thumbnail.report["cached"] = json!(true);
        cache.push_back(cached);
        Some(thumbnail)
    }

    fn prepare_prefab_thumbnail(&self, source: &Source) -> Result<Prepared, String> {
        let nodes = source_formats::keyvalues::parse_bounded(
            &source.text,
            transfer::MAX_DEPTH,
            transfer::MAX_NODES,
        )
        .map_err(|error| format!("Prefab thumbnail source: {error}"))?;
        let vmf = source_formats::vmf::Vmf::from_nodes(&nodes)
            .map_err(|error| format!("Prefab thumbnail source: {error}"))?;
        let brushes = vmf.solids().count();
        let sides = vmf.side_ids().count();
        if brushes > MAX_BRUSHES
            || sides > MAX_SIDES
            || vmf
                .solids()
                .any(|solid| solid.sides.len() > MAX_BRUSH_SIDES)
        {
            return Err("Prefab thumbnail exceeds 128 brushes, 1024 sides or 64 sides per brush; placement remains available".into());
        }
        let displacement_vertices = vmf
            .solids()
            .flat_map(|solid| &solid.sides)
            .filter_map(|side| side.displacement.as_ref())
            .try_fold(0usize, |total, displacement| {
                let count = 1usize
                    .checked_shl(displacement.power)?
                    .checked_add(1)?
                    .checked_pow(2)?;
                total.checked_add(count)
            })
            .ok_or("Prefab thumbnail displacement size is invalid")?;
        if displacement_vertices > MAX_DISPLACEMENT_VERTICES {
            return Err("Prefab thumbnail exceeds 65536 displacement vertices".into());
        }
        if vmf.entities.iter().any(|entity| {
            entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
        }) {
            return Err("Prefab thumbnail cannot resolve func_instance content; no instance geometry was rendered".into());
        }
        let paths = vmf
            .entities
            .iter()
            .filter(|entity| entity.solids.is_empty())
            .filter_map(|entity| entity.properties.get("model"))
            .filter(|path| !path.trim().is_empty())
            .collect::<std::collections::BTreeSet<_>>();
        let model_count = vmf
            .entities
            .iter()
            .filter(|entity| {
                entity.solids.is_empty()
                    && entity
                        .properties
                        .get("model")
                        .is_some_and(|path| !path.trim().is_empty())
            })
            .count();
        if paths.len() > MAX_MODEL_PATHS || model_count > MAX_MODEL_INSTANCES {
            return Err(
                "Prefab thumbnail exceeds eight model paths or sixteen model instances".into(),
            );
        }
        let payload = transfer::Payload::decode(&source.text, &self.definitions)
            .map_err(|error| format!("Prefab thumbnail source: {error}"))?;
        let mut vmf = payload.document().vmf().clone();
        if let Some(entity) = vmf.entities.iter().find(|entity| {
            entity.solids.is_empty() && entity.origin().is_none_or(|origin| !origin.is_finite())
        }) {
            return Err(format!(
                "Prefab thumbnail entity {} has no finite point origin",
                entity.id
            ));
        }
        let hidden = vmf.solids().filter(|solid| solid.hidden).count()
            + vmf.entities.iter().filter(|entity| entity.hidden).count();
        for solid in &mut vmf.world.solids {
            solid.hidden = false;
        }
        for entity in &mut vmf.entities {
            entity.hidden = false;
            for solid in &mut entity.solids {
                solid.hidden = false;
            }
        }
        vmf.world.properties.remove("skyname");
        let document = map_core::Document::try_new(vmf).map_err(|error| error.to_string())?;
        let models = self.model_instances(&document);
        if let Some((id, error)) = models.issues.iter().next() {
            return Err(format!("Prefab thumbnail model on entity {id}: {error}"));
        }
        if models
            .instances
            .iter()
            .any(|instance| !instance.has_geometry())
        {
            return Err("Prefab thumbnail model bodygroups contain no visible geometry".into());
        }
        let mut names = document
            .vmf()
            .solids()
            .flat_map(|solid| &solid.sides)
            .map(|side| side.material.to_ascii_lowercase())
            .collect::<std::collections::BTreeSet<_>>();
        let mut model_vertices = 0;
        let mut model_indices = 0;
        for instance in &models.instances {
            model_vertices += instance.asset.model.vertices.len();
            model_indices += instance
                .asset
                .model
                .parts_for_body(instance.body)
                .map(|part| part.indices.len())
                .sum::<usize>();
            names.extend(
                instance
                    .asset
                    .materials()?
                    .iter()
                    .map(|name| name.to_ascii_lowercase()),
            );
        }
        if model_vertices > MAX_VERTICES || model_indices > MAX_INDICES {
            return Err(
                "Prefab thumbnail model geometry exceeds 200000 vertices or 600000 indices".into(),
            );
        }
        if names.len() > MAX_MATERIALS {
            return Err("Prefab thumbnail exceeds 64 material bindings".into());
        }
        let mut materials = MaterialLibrary::flat(names);
        let empty = std::collections::HashSet::new();
        let mesh = SceneMesh::build_with_models(
            document.vmf(),
            &self.game,
            &mut materials,
            &empty,
            &std::collections::HashSet::new(),
            &[],
            &models.instances,
        );
        if mesh.stats.invalid_brushes != 0 {
            return Err("Prefab thumbnail contains invalid brush geometry".into());
        }
        if let Some(issue) = mesh.issues.first() {
            return Err(format!("Prefab thumbnail: {}", issue.description()));
        }
        if mesh.vertices.len() > MAX_VERTICES || mesh.indices.len() > MAX_INDICES {
            return Err("Prefab thumbnail exceeds 200000 vertices or 600000 indices".into());
        }
        if mesh.vertices.is_empty() || !mesh.mins.is_finite() || !mesh.maxs.is_finite() {
            return Err("Prefab thumbnail contains no bounded visible geometry".into());
        }
        let camera = Camera::framing(
            mesh.mins,
            mesh.maxs,
            Vec3::new(1.0, 1.0, -0.8),
            45.0,
            WIDTH as f32 / HEIGHT as f32,
        );
        let report = json!({"name":source.entry.name,"width":WIDTH,"height":HEIGHT,"style":"flat_geometry","textures_loaded":false,"hidden_objects_included":hidden,"objects":payload.ids().len(),"brushes":brushes,"sides":sides,"model_instances":models.instances.len(),"point_helpers":mesh.stats.point_entities - mesh.stats.model_entities,"bounds":[mesh.mins.to_array(),mesh.maxs.to_array()],"vertices":mesh.vertices.len(),"indices":mesh.indices.len(),"cached":false,"limits":limits()});
        Ok(Prepared {
            mesh,
            materials,
            camera,
            report,
        })
    }

    pub fn prefab_thumbnail_report(&self, name: &str) -> Value {
        let result = self.thumbnail_source(name).and_then(|source| {
            if let Some(thumbnail) = self.cached_thumbnail(&source) {
                return Ok(thumbnail.report);
            }
            let prepared = self.prepare_prefab_thumbnail(&source)?;
            let current = self.thumbnail_source(name)?;
            if current.entry != source.entry || current.text != source.text {
                return Err(
                    "Prefab changed while inspecting its thumbnail; refresh and retry".into(),
                );
            }
            Ok(prepared.report)
        });
        let mut report = result.unwrap_or_else(|error| {
            self.prefabs
                .thumbnails
                .cache
                .borrow_mut()
                .retain(|cached| cached.source.name != name);
            json!({"name":name,"error":error,"limits":limits()})
        });
        report["graphics"] = json!({"initialized":self.prefabs.thumbnails.renderer.initialized(),"error":self.prefabs.thumbnails.error});
        report
    }

    pub fn cached_prefab_thumbnail(&self, name: &str) -> Result<Option<Thumbnail>, String> {
        match self.thumbnail_source(name) {
            Ok(source) => Ok(self.cached_thumbnail(&source)),
            Err(error) => {
                self.prefabs
                    .thumbnails
                    .cache
                    .borrow_mut()
                    .retain(|cached| cached.source.name != name);
                Err(error)
            }
        }
    }

    pub fn prefab_thumbnail(&mut self, name: &str, refresh: bool) -> Result<Thumbnail, String> {
        if refresh {
            self.prefabs
                .thumbnails
                .cache
                .borrow_mut()
                .retain(|cached| cached.source.name != name);
        }
        let result = self.render_prefab_thumbnail(name);
        self.prefabs.thumbnails.error = result.as_ref().err().cloned();
        if result.is_err() {
            self.prefabs
                .thumbnails
                .cache
                .borrow_mut()
                .retain(|cached| cached.source.name != name);
        }
        result
    }

    fn render_prefab_thumbnail(&mut self, name: &str) -> Result<Thumbnail, String> {
        let source = self.thumbnail_source(name)?;
        if let Some(thumbnail) = self.cached_thumbnail(&source) {
            return Ok(thumbnail);
        }
        let prepared = self.prepare_prefab_thumbnail(&source)?;
        let pixels = self.prefabs.thumbnails.renderer.render_scene(
            &prepared.mesh,
            &prepared.materials,
            &prepared.camera,
            &ViewOptions {
                surface: SurfaceView::Flat,
                grid_visible: false,
                leak_path: false,
                selection_outlines: false,
                ..ViewOptions::default()
            },
            WIDTH,
            HEIGHT,
        )?;
        let current = self.thumbnail_source(name)?;
        if current.entry != source.entry || current.text != source.text {
            return Err("Prefab changed while rendering its thumbnail; refresh and retry".into());
        }
        let thumbnail = Thumbnail {
            image: Arc::new(RgbaImage {
                width: WIDTH,
                height: HEIGHT,
                pixels,
            }),
            report: prepared.report,
        };
        let mut cache = self.prefabs.thumbnails.cache.borrow_mut();
        while cache.len() >= MAX_CACHE {
            cache.pop_front();
        }
        cache.push_back(Cached {
            source: source.entry,
            text: source.text,
            epoch: (self.content_epoch, self.definitions_serial),
            thumbnail: thumbnail.clone(),
        });
        Ok(thumbnail)
    }
}

#[cfg(test)]
mod tests;
