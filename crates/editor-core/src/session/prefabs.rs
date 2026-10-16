use super::{Actor, Session};
use crate::prefabs::{self, Store};
use glam::DVec3;
use map_core::{ObjectId, transfer};
use serde_json::{Value, json};
use std::path::PathBuf;

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

pub(super) struct State {
    store: Result<Store, String>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            store: Err("Prefab storage is not configured".into()),
        }
    }
}
fn info(name: &str, payload: &transfer::Payload) -> Value {
    json!({"name":name,"objects":payload.ids().len(),"groups":payload.document().vmf().world.groups.len(),"layers":map_core::layers::Graph::read(payload.document()).map(|graph|graph.records.len()).ok(),"faces":payload.document().vmf().side_ids().count(),"bytes":payload.text().len(),"bounds":map_core::query::bounds_of(payload.document(),payload.ids()).map(|(a,b)|[a.to_array(),b.to_array()]),"placement_anchor":"Original bounds center by default; an optional source-world pivot maps to the placement position"})
}
impl Session {
    pub(super) fn configure_prefabs(&mut self, directory: Result<PathBuf, String>) {
        let directory = std::env::var_os("HAMMERR_PREFABS_DIR")
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(|| directory.map(|path| path.join("prefabs")));
        self.prefabs.store = directory.map(|directory| Store { directory });
    }
    fn prefab_snapshot(&self) -> Result<(prefabs::Snapshot, String), String> {
        let store = self.prefabs.store.as_ref().map_err(Clone::clone)?;
        let snapshot = store.snapshot()?;
        let stamp = format!("{}:{}", self.clipboard_stamp(), snapshot.stamp);
        Ok((snapshot, stamp))
    }
    fn checked_prefabs(&self, stamp: &str) -> Result<prefabs::Snapshot, String> {
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
        self.prefabs
            .store
            .as_ref()
            .map_err(Clone::clone)?
            .check_preview(source)
    }
    pub fn prefab_report(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
        selected: Option<&str>,
    ) -> Value {
        let mut result = json!({"limits":{"entries":prefabs::MAX_ENTRIES,"file_bytes":transfer::MAX_BYTES,"library_bytes":prefabs::MAX_TOTAL_BYTES,"scanned_directory_entries":prefabs::MAX_SCAN,"name_bytes":prefabs::MAX_NAME_BYTES},"selected_objects":self.selected_ids().iter().copied().map(crate::identity::id_string).collect::<Vec<_>>(),"directory":self.prefabs.store.as_ref().ok().map(|store|store.directory.to_string_lossy())});
        match self.prefab_snapshot() {
            Err(error) => {
                result["error"] = json!(error);
                result["entries"] = json!([]);
            }
            Ok((snapshot, stamp)) => {
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
                    let loaded = self
                        .prefabs
                        .store
                        .as_ref()
                        .unwrap()
                        .read(name, &snapshot.stamp)
                        .and_then(|text| {
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
        self.prefabs.store.as_ref().map_err(Clone::clone)?.save(
            name,
            payload.text(),
            &snapshot.stamp,
        )?;
        self.view_serial += 1;
        self.log(actor, format!("saved prefab {name}"));
        Ok(self.prefab_report("", 0, 64, Some(name)))
    }
    pub fn delete_prefab(
        &mut self,
        actor: Actor,
        name: &str,
        stamp: &str,
    ) -> Result<Value, String> {
        let snapshot = self.checked_prefabs(stamp)?;
        self.prefabs
            .store
            .as_ref()
            .map_err(Clone::clone)?
            .remove(name, &snapshot.stamp)?;
        self.view_serial += 1;
        self.log(actor, format!("deleted prefab {name}"));
        Ok(self.prefab_report("", 0, 64, None))
    }
    pub fn rename_prefab(
        &mut self,
        actor: Actor,
        name: &str,
        new_name: &str,
        stamp: &str,
    ) -> Result<Value, String> {
        let snapshot = self.checked_prefabs(stamp)?;
        self.prefabs.store.as_ref().map_err(Clone::clone)?.rename(
            name,
            new_name,
            &snapshot.stamp,
        )?;
        self.stale_prefab_preview(name, new_name);
        self.view_serial += 1;
        self.log(actor, format!("renamed prefab {name} to {new_name}"));
        Ok(self.prefab_report("", 0, 64, Some(new_name)))
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
        let text = self
            .prefabs
            .store
            .as_ref()
            .map_err(Clone::clone)?
            .read(name, &snapshot.stamp)?;
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
        let source = snapshot
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or("Prefab no longer exists")?
            .clone();
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
        self.checked_prefabs(stamp)?;
        Ok(self.install_prefab_paste(actor, payload, prepared))
    }
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) mod fixtures;

#[cfg(test)]
pub(crate) mod tests;
