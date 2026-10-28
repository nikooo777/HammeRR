use super::{Actor, Session};

use crate::prefabs::{self, Authority};
use glam::DVec3;
use map_core::{ObjectId, transfer};
use serde_json::{Value, json};
use std::path::PathBuf;

mod thumbnails;
pub use thumbnails::{Thumbnail, ThumbnailStyle};
mod gallery;
pub use gallery::{Gallery, GalleryEntry, GalleryReview, MAX_GALLERY_ENTRIES};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub position: DVec3,
    pub rotation: DVec3,
    pub scale: DVec3,
    pub pivot: Option<DVec3>,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            position: DVec3::ZERO,
            rotation: DVec3::ZERO,
            scale: DVec3::ONE,
            pivot: None,
        }
    }
}

enum Binding {
    Independent(Authority),
    Application(Authority),
}

impl Binding {
    fn authority(&self) -> &Authority {
        match self {
            Self::Independent(authority) | Self::Application(authority) => authority,
        }
    }
}

pub(super) struct State {
    binding: Binding,
    thumbnails: thumbnails::State,
}
impl Default for State {
    fn default() -> Self {
        Self {
            binding: Binding::Independent(Authority::default()),
            thumbnails: thumbnails::State::default(),
        }
    }
}
fn info(name: &str, payload: &transfer::Payload) -> Value {
    json!({"name":name,"objects":payload.ids().len(),"groups":payload.document().vmf().world.groups.len(),"layers":map_core::layers::Graph::read(payload.document()).map(|graph|graph.records.len()).ok(),"faces":payload.document().vmf().side_ids().count(),"bytes":payload.text().len(),"bounds":map_core::query::bounds_of(payload.document(),payload.ids()).map(|(a,b)|[a.to_array(),b.to_array()]),"placement_anchor":"Original bounds center by default; an optional source-world pivot maps to the placement position"})
}
impl Session {
    pub fn with_prefabs(
        game: Result<source_formats::gamefs::GameFs, String>,
        authority: Authority,
    ) -> Self {
        let mut session = Self::new(game);
        session.use_application_prefabs(authority);
        session
    }

    pub(crate) fn use_application_prefabs(&mut self, authority: Authority) {
        self.prefabs.binding = Binding::Application(authority);
    }

    pub(super) fn configure_prefabs(&mut self, directory: Result<PathBuf, String>) {
        if let Binding::Independent(authority) = &self.prefabs.binding
            && let Err(error) = authority.configure(directory)
        {
            self.log(Actor::System, error);
        }
    }

    pub fn prefab_registry_serial(&self) -> u64 {
        self.prefabs.binding.authority().serial()
    }

    fn prefab_stamp(&self, snapshot: &prefabs::RegistrySnapshot) -> String {
        format!(
            "{}:{}:{}",
            self.clipboard_stamp(),
            snapshot.revision,
            snapshot.stamp
        )
    }

    fn prefab_snapshot(&self) -> Result<(prefabs::RegistrySnapshot, String), String> {
        let snapshot = self.prefabs.binding.authority().snapshot();
        if let Some(error) = &snapshot.error {
            return Err(error.clone());
        }
        let stamp = self.prefab_stamp(&snapshot);
        Ok((snapshot, stamp))
    }

    fn checked_prefabs(&self, stamp: &str) -> Result<prefabs::RegistrySnapshot, String> {
        let (snapshot, current) = self.prefab_snapshot()?;
        if current != stamp {
            return Err(
                "Map, selection, clipboard or prefab library changed; refresh before continuing"
                    .into(),
            );
        }
        Ok(snapshot)
    }

    pub(super) fn check_prefab_preview_source(
        &self,
        source: &prefabs::Entry,
    ) -> Result<(), String> {
        self.prefabs.binding.authority().check_preview(source)
    }
    pub fn prefab_report(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
        selected: Option<&str>,
    ) -> Value {
        let snapshot = self.prefabs.binding.authority().snapshot();
        self.prefab_report_from_snapshot(snapshot, query, offset, limit, selected)
    }

    fn prefab_report_from_snapshot(
        &self,
        snapshot: prefabs::RegistrySnapshot,
        query: &str,
        offset: usize,
        limit: usize,
        selected: Option<&str>,
    ) -> Value {
        let mut result = json!({"limits":{"entries":prefabs::MAX_ENTRIES,"file_bytes":transfer::MAX_BYTES,"library_bytes":prefabs::MAX_TOTAL_BYTES,"scanned_directory_entries":prefabs::MAX_SCAN,"name_bytes":prefabs::MAX_NAME_BYTES},"selected_objects":self.selected_ids().iter().copied().map(crate::identity::id_string).collect::<Vec<_>>(),"directory":snapshot.directory.as_ref().map(|path|path.to_string_lossy()),"registry_revision":snapshot.revision,"write_error":snapshot.write_error});
        match snapshot.error.as_ref() {
            Some(error) => {
                result["error"] = json!(error);
                result["entries"] = json!([]);
            }
            None => {
                let stamp = self.prefab_stamp(&snapshot);
                let query = query.to_lowercase();
                let entries = snapshot
                    .entries
                    .iter()
                    .filter(|entry| entry.name.to_lowercase().contains(&query))
                    .collect::<Vec<_>>();
                let limit = limit.clamp(1, 64);
                result["snapshot"] = json!(stamp);
                result["total"] = json!(entries.len());
                result["library_bytes"] = json!(snapshot.bytes);
                result["next_offset"] = json!(
                    (offset.saturating_add(limit) < entries.len())
                        .then_some(offset.saturating_add(limit))
                );
                result["entries"] = json!(
                    entries
                        .into_iter()
                        .skip(offset)
                        .take(limit)
                        .map(|entry| json!({"name":entry.name,"bytes":entry.bytes}))
                        .collect::<Vec<_>>()
                );
                if let Some(name) = selected {
                    result["selected_exists"] =
                        json!(snapshot.entries.iter().any(|entry| entry.name == name));
                    let loaded = self
                        .prefabs
                        .binding
                        .authority()
                        .read(name, snapshot.revision, &snapshot.stamp)
                        .and_then(|(_, text)| {
                            transfer::Payload::decode(&text, &self.definitions)
                                .map_err(|error| error.to_string())
                        });
                    match loaded {
                        Ok(payload) => {
                            result["selected"] = info(name, &payload);
                            result["selected"]["external_links"]=json!(payload.external(&self.definitions,&self.document).map(|links|links.iter().map(|link|json!({"source":link.source,"target":link.target,"destination_matches":link.matches})).collect::<Vec<_>>()).ok());
                        }
                        Err(error) => result["selection_error"] = json!(error),
                    }
                }
            }
        }
        result
    }
    pub fn save_prefab(
        &mut self,
        actor: Actor,
        name: &str,
        ids: &[ObjectId],
        stamp: &str,
    ) -> Result<Value, String> {
        let snapshot = self.checked_prefabs(stamp)?;
        let payload = transfer::Payload::capture(&self.document, ids, &self.definitions)
            .map_err(|error| error.to_string())?;
        let committed = self.prefabs.binding.authority().change(
            snapshot.revision,
            &snapshot.stamp,
            prefabs::Edit::Save {
                name,
                text: payload.text(),
            },
        )?;
        self.log(actor, format!("saved prefab {name}"));
        let mut report = self.prefab_report_from_snapshot(committed, "", 0, 64, Some(name));
        report["write_committed"] = json!(true);
        Ok(report)
    }
    pub fn delete_prefab(
        &mut self,
        actor: Actor,
        name: &str,
        stamp: &str,
    ) -> Result<Value, String> {
        let snapshot = self.checked_prefabs(stamp)?;
        let committed = self.prefabs.binding.authority().change(
            snapshot.revision,
            &snapshot.stamp,
            prefabs::Edit::Delete { name },
        )?;
        self.log(actor, format!("deleted prefab {name}"));
        let mut report = self.prefab_report_from_snapshot(committed, "", 0, 64, None);
        report["write_committed"] = json!(true);
        Ok(report)
    }
    pub fn rename_prefab(
        &mut self,
        actor: Actor,
        name: &str,
        new_name: &str,
        stamp: &str,
    ) -> Result<Value, String> {
        let snapshot = self.checked_prefabs(stamp)?;
        let committed = self.prefabs.binding.authority().change(
            snapshot.revision,
            &snapshot.stamp,
            prefabs::Edit::Rename { name, new_name },
        )?;
        self.stale_prefab_preview(name, new_name);
        self.log(actor, format!("renamed prefab {name} to {new_name}"));
        let mut report = self.prefab_report_from_snapshot(committed, "", 0, 64, Some(new_name));
        report["write_committed"] = json!(true);
        Ok(report)
    }
    pub fn preview_prefab(
        &mut self,
        actor: Actor,
        name: &str,
        position: DVec3,
        keep_external: bool,
        stamp: &str,
    ) -> Result<Value, String> {
        self.preview_prefab_placement(
            actor,
            name,
            Placement {
                position,
                ..Default::default()
            },
            keep_external,
            stamp,
        )
    }
    pub fn preview_prefab_placement(
        &mut self,
        actor: Actor,
        name: &str,
        placement: Placement,
        keep_external: bool,
        stamp: &str,
    ) -> Result<Value, String> {
        let snapshot = self.checked_prefabs(stamp)?;
        self.ensure_partitions_resolved()?;
        let (source, text) =
            self.prefabs
                .binding
                .authority()
                .read(name, snapshot.revision, &snapshot.stamp)?;
        let payload = transfer::Payload::decode(&text, &self.definitions)
            .map_err(|error| error.to_string())?;
        let (a, b) = map_core::query::bounds_of(payload.document(), payload.ids())
            .ok_or("Prefab needs bounded objects")?;
        let pivot = placement.pivot.unwrap_or((a + b) * 0.5);
        let mut detail = info(name, &payload);
        detail["placement"] = json!({"position":placement.position.to_array(),"rotation":placement.rotation.to_array(),"scale":placement.scale.to_array(),"source_pivot":pivot.to_array(),"pivot_policy":if placement.pivot.is_some(){"custom"}else{"bounds_center"},"order":"Scale in world XYZ, then rotate world X/Y/Z, then move the source pivot to position; textures stay attached"});
        let payload = payload
            .transformed(
                placement.rotation,
                placement.scale,
                pivot,
                &self.definitions,
            )
            .map_err(|error| error.to_string())?;
        let options = transfer::Options {
            offset: placement.position - pivot,
            keep_external,
            texture_lock: true,
            restore_names: false,
        };
        let prepared = self.prepare_paste(&payload, options, Some(source))?;
        let paste = self.finish_prefab_placement(actor, payload, prepared, stamp)?;
        self.log(actor, format!("preview prefab {name}"));
        Ok(json!({"prefab":detail,"paste":paste}))
    }

    fn finish_prefab_placement(
        &mut self,
        actor: Actor,
        payload: transfer::Payload,
        prepared: super::clipboard::PreparedPaste,
        stamp: &str,
    ) -> Result<Value, String> {
        let snapshot = self.checked_prefabs(stamp)?;
        self.prefabs
            .binding
            .authority()
            .check(snapshot.revision, &snapshot.stamp)?;
        Ok(self.install_prefab_paste(actor, payload, prepared))
    }
}
