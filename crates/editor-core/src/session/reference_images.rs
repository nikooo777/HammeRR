use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use map_core::reference_images::{self as metadata, Image, Metadata};
use render::reference_images::{Asset, Plane};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Actor, Session};
use crate::bounded_file::Source;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const INITIAL_LONG_AXIS: f64 = 256.0;

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub name: String,
    pub position: [f64; 3],
    pub rotation: [f64; 3],
    pub size: [f64; 2],
    pub opacity: f64,
    pub flip: [bool; 2],
    pub views: [bool; 4],
}

struct Loaded {
    source: Source,
    asset: Arc<Asset>,
}
struct Cached {
    path: String,
    sha256: String,
    dimensions: [u32; 2],
    loaded: Result<Loaded, String>,
}

#[cfg(feature = "mcp")]
pub(crate) struct RenderGuard {
    generation: u64,
    catalog: Metadata,
    sources: Vec<Source>,
}

pub(super) struct State {
    serial: u64,
    scope: String,
    entries: BTreeMap<u32, Cached>,
    revision: Option<(u64, u64)>,
    last_poll: Option<Instant>,
    error: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            serial: 0,
            scope: crate::recovery::unique_id(),
            entries: BTreeMap::new(),
            revision: None,
            last_poll: None,
            error: None,
        }
    }
}

fn load(path: &str) -> Result<Loaded, String> {
    let path = Path::new(path);
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("png"))
    {
        return Err("Choose a static PNG reference image".into());
    }
    let source = Source::capture(path, render::reference_images::MAX_FILE_BYTES as u64)
        .map_err(|error| format!("Could not open reference image: {error}"))?;
    let bytes = source
        .clone()
        .read()
        .map_err(|error| format!("Could not read reference image: {error}"))?;
    let asset = Arc::new(render::reference_images::decode_png(&bytes)?);
    source
        .verify()
        .map_err(|error| format!("Reference image changed during decoding: {error}"))?;
    Ok(Loaded { source, asset })
}

fn absolute_path(path: &Path) -> Result<String, String> {
    let path = std::path::absolute(path).map_err(|error| error.to_string())?;
    let path = path
        .to_str()
        .ok_or("Reference image path must be UTF-8")?
        .to_owned();
    metadata::validate_path(&path)?;
    Ok(path)
}

fn expected(image: &Image) -> Result<Loaded, String> {
    let loaded = load(&image.path)?;
    if loaded.asset.sha256 != image.sha256
        || [loaded.asset.image.width, loaded.asset.image.height] != image.dimensions
    {
        return Err("Reference image differs from its saved identity; use Reload image to accept the new source explicitly".into());
    }
    Ok(loaded)
}

fn plane(image: &Image, asset: Arc<Asset>) -> Plane {
    Plane {
        corners: image.corners().map(|point| point.as_vec3().to_array()),
        center: image.position.map(|value| value as f32),
        opacity: image.opacity as f32,
        flip: image.flip,
        views: image.views,
        asset,
    }
}

fn visible(image: &Image, projection: render::Projection) -> bool {
    image.views[match projection {
        render::Projection::Top => 0,
        render::Projection::Front => 1,
        render::Projection::Side => 2,
        render::Projection::Perspective => 3,
    }] && image.opacity > 0.0
}

impl Session {
    pub fn reference_images_snapshot(&self) -> String {
        format!(
            "reference-images-v1:{}:{}:{}",
            self.reference_images.scope,
            self.document_generation,
            self.document.revision()
        )
    }

    fn reference_image_edit_admission(&self, snapshot: &str) -> Result<(), String> {
        self.ensure_not_closing()?;
        if snapshot != self.reference_images_snapshot() {
            return Err("Map changed; refresh reference images before applying this action".into());
        }
        self.inspection_viewport_ready("changing reference images")?;
        if let Some(kind) = self.document_close_review().first_draft_except("") {
            return Err(format!(
                "Apply or cancel the active {kind} draft before changing reference images"
            ));
        }
        if self.geometry.instance_collapse.worker.busy()
            || !std::ptr::eq(self.render_document(), &self.document)
        {
            return Err("Finish active geometry work before changing reference images".into());
        }
        Ok(())
    }

    fn refresh_reference_images(&mut self, force: bool) {
        let revision = (self.document_generation, self.document.revision());
        let now = Instant::now();
        if !force
            && self.reference_images.revision == Some(revision)
            && self
                .reference_images
                .last_poll
                .is_some_and(|last| now.duration_since(last) < POLL_INTERVAL)
        {
            return;
        }
        self.reference_images.last_poll = Some(now);
        self.reference_images.revision = Some(revision);
        let catalog = match Metadata::parse(self.document.vmf()) {
            Ok(catalog) => catalog,
            Err(error) => {
                if self.reference_images.error.as_ref() != Some(&error)
                    || !self.reference_images.entries.is_empty()
                {
                    self.reference_images.serial += 1;
                }
                self.reference_images.error = Some(error);
                self.reference_images.entries.clear();
                return;
            }
        };
        let mut changed = self.reference_images.error.take().is_some();
        let before = self.reference_images.entries.len();
        self.reference_images
            .entries
            .retain(|id, _| catalog.images.iter().any(|image| image.id == *id));
        changed |= before != self.reference_images.entries.len();
        for image in &catalog.images {
            let fresh = self
                .reference_images
                .entries
                .get(&image.id)
                .is_none_or(|cached| {
                    cached.path != image.path
                        || cached.sha256 != image.sha256
                        || cached.dimensions != image.dimensions
                });
            if fresh {
                self.reference_images.entries.insert(
                    image.id,
                    Cached {
                        path: image.path.clone(),
                        sha256: image.sha256.clone(),
                        dimensions: image.dimensions,
                        loaded: expected(image),
                    },
                );
                changed = true;
            } else if let Some(cached) = self.reference_images.entries.get_mut(&image.id)
                && let Ok(loaded) = &cached.loaded
                && let Err(error) = loaded.source.verify()
            {
                cached.loaded = Err(format!(
                    "Reference image is unavailable: {error}. Use Reload image"
                ));
                changed = true;
            }
        }
        if changed {
            self.reference_images.serial += 1;
        }
    }

    pub fn poll_reference_images(&mut self) {
        self.refresh_reference_images(false);
    }

    pub(super) fn reference_images_serial(&self) -> u64 {
        self.reference_images.serial
    }

    pub fn reference_images_summary(&self) -> String {
        if let Some(error) = &self.reference_images.error {
            return format!("Reference images unavailable: {error}");
        }
        let failed = self
            .reference_images
            .entries
            .values()
            .filter(|entry| entry.loaded.is_err())
            .count();
        if failed > 0 {
            format!("{failed} reference image(s) unavailable; open Reference images…")
        } else {
            format!("{} reference image(s)", self.reference_images.entries.len())
        }
    }

    pub fn reference_images_state(&mut self) -> Value {
        self.refresh_reference_images(true);
        let catalog = Metadata::parse(self.document.vmf());
        let (images, error) = match catalog {
            Ok(catalog) => (json!(catalog.images.iter().map(|image| {
                let cached = self.reference_images.entries.get(&image.id);
                let error = cached.and_then(|cached| cached.loaded.as_ref().err());
                json!({"id":image.id,"name":image.name,"path":image.path,"sha256":image.sha256,"dimensions":image.dimensions,
                    "settings":Settings { name:image.name.clone(),position:image.position,rotation:image.rotation,size:image.size,opacity:image.opacity,flip:image.flip,views:image.views },
                    "corners":image.corners().map(|point| point.to_array()),"ready":cached.is_some_and(|cached| cached.loaded.is_ok()),"error":error})
            }).collect::<Vec<_>>()), None),
            Err(error) => (Value::Null, Some(error)),
        };
        let ready = error.is_none()
            && self
                .reference_images
                .entries
                .values()
                .all(|entry| entry.loaded.is_ok());
        json!({"snapshot":self.reference_images_snapshot(),"images":images,"metadata_ready":error.is_none(),"ready":ready,"error":error,"summary":self.reference_images_summary(),
            "limits":{"images":metadata::MAX_IMAGES,"axis":metadata::MAX_AXIS,"pixels_per_image":metadata::MAX_PIXELS,"total_pixels":metadata::MAX_TOTAL_PIXELS,"file_bytes":render::reference_images::MAX_FILE_BYTES},
            "policy":"Editor-only static PNG planes. Rotation is XYZ Euler degrees from an XY plane, image up +Y, right +X; views order Top/Front/Side/Perspective. Actual RGBA8 pixels, two-sided, unlit, depth-tested alpha blending. Absolute source paths and exact SHA256 are saved; missing/changed/invalid sources are unavailable until explicit Reload image. No substitution, pixel embedding, game geometry or automatic asset packaging. Undo changes map metadata, not external image files."})
    }

    pub fn add_reference_image(
        &mut self,
        actor: Actor,
        path: &Path,
        name: &str,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.reference_image_edit_admission(snapshot)?;
        metadata::validate_name(name)?;
        let mut catalog = Metadata::parse(self.document.vmf())?;
        if catalog.images.len() == metadata::MAX_IMAGES {
            return Err("At most 8 reference images are supported".into());
        }
        let path = absolute_path(path)?;
        let loaded = load(&path)?;
        let dimensions = [loaded.asset.image.width, loaded.asset.image.height];
        let id = catalog.next_id()?;
        catalog.images.push(Image {
            id,
            name: name.into(),
            path: path.clone(),
            sha256: loaded.asset.sha256.clone(),
            dimensions,
            position: [0.0; 3],
            rotation: [0.0; 3],
            size: dimensions.map(|axis| {
                INITIAL_LONG_AXIS * f64::from(axis) / f64::from(dimensions[0].max(dimensions[1]))
            }),
            opacity: 0.5,
            flip: [false; 2],
            views: [true; 4],
            extra: Vec::new(),
        });
        catalog.validate()?;
        self.edit(actor, "add reference image", |tx| catalog.write(tx))?;
        self.reference_images.entries.insert(
            id,
            Cached {
                path,
                sha256: loaded.asset.sha256.clone(),
                dimensions,
                loaded: Ok(loaded),
            },
        );
        let mut report = self.reference_images_state();
        report["created_id"] = json!(id);
        Ok(report)
    }

    pub fn update_reference_image(
        &mut self,
        actor: Actor,
        id: u32,
        settings: Settings,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.reference_image_edit_admission(snapshot)?;
        let mut catalog = Metadata::parse(self.document.vmf())?;
        let image = catalog
            .images
            .iter_mut()
            .find(|image| image.id == id)
            .ok_or("Reference image no longer exists")?;
        image.name = settings.name;
        image.position = settings.position;
        image.rotation = settings.rotation;
        image.size = settings.size;
        image.opacity = settings.opacity;
        image.flip = settings.flip;
        image.views = settings.views;
        catalog.validate()?;
        self.edit(actor, "edit reference image", |tx| catalog.write(tx))?;
        Ok(self.reference_images_state())
    }

    pub fn reload_reference_image(
        &mut self,
        actor: Actor,
        id: u32,
        path: Option<&Path>,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.reference_image_edit_admission(snapshot)?;
        let mut catalog = Metadata::parse(self.document.vmf())?;
        let image = catalog
            .images
            .iter_mut()
            .find(|image| image.id == id)
            .ok_or("Reference image no longer exists")?;
        let path = match path {
            Some(path) => absolute_path(path)?,
            None => image.path.clone(),
        };
        let loaded = match load(&path) {
            Ok(loaded) => loaded,
            Err(error) => {
                self.reference_images.entries.insert(
                    id,
                    Cached {
                        path: image.path.clone(),
                        sha256: image.sha256.clone(),
                        dimensions: image.dimensions,
                        loaded: Err(error.clone()),
                    },
                );
                self.reference_images.serial += 1;
                self.view_serial += 1;
                return Err(error);
            }
        };
        image.path = path.clone();
        image.sha256 = loaded.asset.sha256.clone();
        image.dimensions = [loaded.asset.image.width, loaded.asset.image.height];
        catalog.validate()?;
        self.edit(actor, "reload reference image", |tx| catalog.write(tx))?;
        self.reference_images.entries.insert(
            id,
            Cached {
                path,
                sha256: loaded.asset.sha256.clone(),
                dimensions: [loaded.asset.image.width, loaded.asset.image.height],
                loaded: Ok(loaded),
            },
        );
        self.reference_images.serial += 1;
        self.view_serial += 1;
        Ok(self.reference_images_state())
    }

    pub fn delete_reference_image(
        &mut self,
        actor: Actor,
        id: u32,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.reference_image_edit_admission(snapshot)?;
        let mut catalog = Metadata::parse(self.document.vmf())?;
        if !catalog.images.iter().any(|image| image.id == id) {
            return Err("Reference image no longer exists".into());
        }
        catalog.images.retain(|image| image.id != id);
        self.edit(actor, "delete reference image", |tx| catalog.write(tx))?;
        Ok(self.reference_images_state())
    }

    pub fn frame_reference_image(
        &mut self,
        actor: Actor,
        id: u32,
        snapshot: &str,
        aspect: f32,
    ) -> Result<(), String> {
        self.ensure_not_closing()?;
        self.inspection_viewport_ready("framing a reference image")?;
        if snapshot != self.reference_images_snapshot() {
            return Err("Map changed; refresh reference images before framing".into());
        }
        super::inspection_framing::validate_aspect(aspect)?;
        let catalog = Metadata::parse(self.document.vmf())?;
        let image = catalog
            .images
            .iter()
            .find(|image| image.id == id)
            .ok_or("Reference image no longer exists")?;
        let corners = image.corners();
        let mins = corners
            .iter()
            .copied()
            .reduce(glam::DVec3::min)
            .unwrap()
            .as_vec3();
        let maxs = corners
            .iter()
            .copied()
            .reduce(glam::DVec3::max)
            .unwrap()
            .as_vec3();
        let camera = self.inspection_frame_camera(mins, maxs, aspect, "Reference image")?;
        self.set_camera(camera);
        self.log(actor, "framed reference image");
        Ok(())
    }

    pub fn verify_reference_images_for_render(
        &mut self,
        projection: render::Projection,
    ) -> Result<(), String> {
        self.refresh_reference_images(true);
        let catalog = Metadata::parse(self.document.vmf())?;
        for image in catalog
            .images
            .iter()
            .filter(|image| visible(image, projection))
        {
            let cached = self
                .reference_images
                .entries
                .get(&image.id)
                .ok_or("Reference image is unavailable")?;
            if let Err(error) = &cached.loaded {
                return Err(format!("Reference image {}: {error}", image.name));
            }
        }
        Ok(())
    }

    #[cfg(feature = "mcp")]
    pub(crate) fn reference_images_render_guard(
        &mut self,
        projection: render::Projection,
    ) -> Result<RenderGuard, String> {
        self.verify_reference_images_for_render(projection)?;
        let catalog = Metadata::parse(self.document.vmf())?;
        let sources = catalog
            .images
            .iter()
            .filter(|image| visible(image, projection))
            .map(|image| {
                self.reference_images.entries[&image.id]
                    .loaded
                    .as_ref()
                    .expect("admitted reference image")
                    .source
                    .clone()
            })
            .collect();
        Ok(RenderGuard {
            generation: self.document_generation,
            catalog,
            sources,
        })
    }

    #[cfg(feature = "mcp")]
    pub(crate) fn verify_reference_images_render(
        &mut self,
        guard: &RenderGuard,
    ) -> Result<(), String> {
        if self.document_generation != guard.generation
            || Metadata::parse(self.document.vmf())? != guard.catalog
        {
            return Err("Reference images changed during rendering; render again".into());
        }
        for source in &guard.sources {
            if let Err(error) = source.verify() {
                self.refresh_reference_images(true);
                return Err(format!(
                    "Reference image source changed during rendering: {error}. Reload the image"
                ));
            }
        }
        Ok(())
    }

    pub(super) fn append_reference_images(&mut self, mesh: &mut render::SceneMesh) {
        self.refresh_reference_images(false);
        if let Ok(catalog) = Metadata::parse(self.document.vmf()) {
            for image in &catalog.images {
                if image.opacity == 0.0 || !image.views.iter().any(|shown| *shown) {
                    continue;
                }
                if let Some(cached) = self.reference_images.entries.get(&image.id)
                    && let Ok(loaded) = &cached.loaded
                {
                    mesh.reference_images
                        .push(plane(image, loaded.asset.clone()));
                }
            }
        }
    }
}
