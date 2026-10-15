use super::{Actor, Session};
use crate::prefabs::{self, Store};
use glam::DVec3;
use map_core::{ObjectId, transfer};
use serde_json::{Value, json};
use std::path::PathBuf;

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
    json!({"name":name,"objects":payload.ids().len(),"groups":payload.document().vmf().world.groups.len(),"layers":map_core::layers::Graph::read(payload.document()).map(|graph|graph.records.len()).ok(),"faces":payload.document().vmf().side_ids().count(),"bytes":payload.text().len(),"bounds":map_core::query::bounds_of(payload.document(),payload.ids()).map(|(a,b)|[a.to_array(),b.to_array()]),"placement_anchor":"Bounds center; placement uses an exact world-space offset"})
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
    pub fn prefab_report(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
        selected: Option<&str>,
    ) -> Value {
        let mut result = json!({"limits":{"entries":prefabs::MAX_ENTRIES,"file_bytes":transfer::MAX_BYTES,"library_bytes":prefabs::MAX_TOTAL_BYTES,"scanned_directory_entries":prefabs::MAX_SCAN,"name_bytes":prefabs::MAX_NAME_BYTES},"selected_objects":self.selected_ids().iter().copied().map(crate::mcp::id_string).collect::<Vec<_>>(),"directory":self.prefabs.store.as_ref().ok().map(|store|store.directory.to_string_lossy())});
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
    pub fn preview_prefab(
        &mut self,
        actor: Actor,
        name: &str,
        position: DVec3,
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
        let options = transfer::Options {
            offset: position - (a + b) * 0.5,
            keep_external,
            texture_lock: true,
            restore_names: false,
        };
        let mut probe = self.document.preview_snapshot();
        probe
            .edit("check prefab placement", |tx| {
                transfer::paste(tx, &payload, options, &self.definitions)
            })
            .map_err(|error| error.to_string())?;
        self.checked_prefabs(stamp)?;
        let detail = info(name, &payload);
        self.import_clipboard(actor, Some(payload.text()), &self.clipboard_stamp())?;
        let paste = self.preview_paste(options, &self.clipboard_stamp())?;
        self.log(actor, format!("preview prefab {name}"));
        Ok(json!({"prefab":detail,"paste":paste}))
    }
}

#[cfg(test)]
pub(crate) mod tests;
