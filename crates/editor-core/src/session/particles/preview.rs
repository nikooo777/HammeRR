use std::{collections::HashMap, sync::Arc};

use map_core::ObjectId;
use render::particle_preview::{self, Asset, Emitter, Scope};
use render::scene::SceneIssue;
use render::{SceneMesh, ViewOptions};
use source_formats::vmf::{Entity, Vmf};

use super::{Session, catalog, is_particle_entity, validate_particle_entity};

const MAX_ASSETS: usize = 32;
const MAX_REJECTIONS: usize = 64;
const MAX_BYTES: usize = 24 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Cache {
    entries: HashMap<String, Arc<Asset>>,
    rejections: HashMap<String, String>,
    bytes: usize,
}

impl Cache {
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.rejections.clear();
        self.bytes = 0;
    }
}

pub(crate) struct Target {
    id: u32,
    source: Result<Source, String>,
}

struct Source {
    effect: String,
    origin: [f32; 3],
}

fn source(entity: &Entity) -> Result<Source, String> {
    validate_particle_entity(entity)?;
    if entity.properties.iter().any(|(key, value)| {
        let value = value.trim();
        if value.is_empty() {
            return false;
        }
        if key.eq_ignore_ascii_case("parentname") {
            return true;
        }
        let key = key.to_ascii_lowercase();
        let Some(control_point) = key.strip_prefix("cpoint") else {
            return false;
        };
        !control_point.strip_suffix("_parent").is_some_and(|index| {
            index.parse::<u8>().is_ok_and(|id| (1..=7).contains(&id))
                && value.parse::<u8>() == Ok(0)
        })
    }) {
        return Err(
            "Particle preview requires an unparented source with only control point 0".into(),
        );
    }
    if entity
        .properties
        .get("flag_as_weather")
        .is_some_and(|value| !value.trim().is_empty() && value.trim().parse::<u8>() != Ok(0))
    {
        return Err("Particle preview does not support weather sources".into());
    }
    let effect = entity
        .properties
        .get("effect_name")
        .filter(|name| !name.trim().is_empty() && name.len() <= 255)
        .ok_or("Set a literal Particle System Name")?;
    let origin = entity
        .origin()
        .filter(|origin| {
            origin.is_finite()
                && origin.abs().max_element() <= map_core::checks::SOURCE_COORDINATE_LIMIT
        })
        .ok_or("Particle preview needs an origin within Source map coordinates")?;
    Ok(Source {
        effect: effect.to_owned(),
        origin: origin.as_vec3().to_array(),
    })
}

pub(crate) fn targets(
    vmf: &Vmf,
    view: ViewOptions,
    contains: impl Fn(ObjectId) -> bool,
    selected: impl Fn(ObjectId) -> bool,
) -> Result<Vec<Target>, String> {
    if view.particle_preview == Scope::Off {
        return Ok(Vec::new());
    }
    if !view.particle_surfaces() {
        return Err("Particle preview requires Legacy Textured view".into());
    }
    let mut targets = Vec::new();
    for entity in &vmf.entities {
        let id = ObjectId::Entity(entity.id);
        if entity.hidden
            || !is_particle_entity(entity)
            || !contains(id)
            || view.particle_preview == Scope::Selected && !selected(id)
        {
            continue;
        }
        if targets.len() == particle_preview::MAX_ENTITIES {
            return Err(format!(
                "Particle preview supports at most {} visible sources",
                particle_preview::MAX_ENTITIES
            ));
        }
        targets.push(Target {
            id: entity.id,
            source: source(entity),
        });
    }
    Ok(targets)
}

impl Session {
    fn particle_preview_asset(&mut self, name: &str) -> Result<Arc<Asset>, String> {
        let key = name.to_ascii_lowercase();
        if let Some(asset) = self.particles.previews.entries.get(&key) {
            return Ok(asset.clone());
        }
        if let Some(error) = self.particles.previews.rejections.get(&key) {
            return Err(error.clone());
        }
        if self.particles.previews.entries.len() == MAX_ASSETS {
            return Err(
                "Particle preview asset cache is full. Reload particles to clear it".into(),
            );
        }
        let asset = self.load_particle_preview(&key).and_then(|asset| {
            let bytes = asset.retained_bytes();
            if self.particles.previews.bytes + bytes > MAX_BYTES {
                return Err("Particle preview exceeds its 24 MiB asset budget".into());
            }
            self.particles.previews.bytes += bytes;
            Ok(asset)
        });
        match &asset {
            Ok(asset) => {
                self.particles.previews.entries.insert(key, asset.clone());
            }
            Err(error) if self.particles.previews.rejections.len() < MAX_REJECTIONS => {
                self.particles
                    .previews
                    .rejections
                    .insert(key, error.clone());
            }
            Err(_) => {}
        }
        asset
    }

    fn load_particle_preview(&mut self, key: &str) -> Result<Arc<Asset>, String> {
        let entry = {
            let catalog = self.particle_catalog()?;
            if catalog.ambiguous.contains(key) {
                return Err("Particle system name matches more than one mounted definition".into());
            }
            catalog
                .aliases
                .get(key)
                .map(|&index| &catalog.entries[index])
                .ok_or("Particle system is outside the active manifest")?
                .clone()
        };
        let data = catalog::read(
            &self.game,
            &entry.source,
            source_formats::particle::MAX_FILE_BYTES,
        )?;
        if catalog::fingerprint(&data) != entry.fingerprint {
            return Err(format!(
                "{} changed. Reload particles before previewing it",
                entry.source
            ));
        }
        let program = source_formats::particle::program(&data, &entry.definition.name)
            .map_err(|error| format!("{}: {error}", entry.source))?;
        Asset::load(&self.game, &program)
    }

    pub(crate) fn append_particle_preview(
        &mut self,
        mesh: &mut SceneMesh,
        targets: Result<Vec<Target>, String>,
    ) {
        let targets = match targets {
            Ok(targets) => targets,
            Err(message) => {
                mesh.issues.push(SceneIssue::Particle {
                    entity: None,
                    message,
                });
                return;
            }
        };
        for Target { id, source } in targets {
            let emitter = source.and_then(|source| {
                self.particle_preview_asset(&source.effect)
                    .map(|asset| Emitter {
                        asset,
                        origin: source.origin,
                        seed: id.max(1),
                    })
                    .map_err(|error| format!("{}: {error}", source.effect))
            });
            match emitter {
                Ok(emitter) => mesh.particles.push(emitter),
                Err(message) => mesh.issues.push(SceneIssue::Particle {
                    entity: Some(id),
                    message,
                }),
            }
        }
    }
}
