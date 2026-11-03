use super::{Actor, Session};

use crate::prefabs::{self, Authority};
use glam::DVec3;
use map_core::{ObjectId, transfer};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const MAX_QUERY_BYTES: usize = 256;
const MAX_REPORT_ENTRIES: usize = 64;

#[derive(Clone, Copy, Debug)]
pub struct Filter<'a> {
    pub query: &'a str,
    pub category: Option<&'a str>,
    pub tags: &'a [String],
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FilterKey {
    query: String,
    category: Option<String>,
    tags: Vec<String>,
}

impl FilterKey {
    fn new(filter: Filter<'_>) -> Result<Self, String> {
        if filter.query.len() > MAX_QUERY_BYTES || filter.query.chars().any(char::is_control) {
            return Err(format!(
                "Prefab queries require at most {MAX_QUERY_BYTES} bytes without control characters"
            ));
        }
        if filter.tags.len() > prefabs::MAX_TAGS
            || filter
                .category
                .is_some_and(|category| category.trim().len() > prefabs::MAX_CATEGORY_BYTES)
            || filter
                .tags
                .iter()
                .any(|tag| tag.trim().len() > prefabs::MAX_TAG_BYTES)
        {
            return Err("Prefab category or tag filters exceed organization limits".into());
        }
        let organization = prefabs::Organization {
            category: filter.category.unwrap_or_default().trim().into(),
            tags: filter
                .tags
                .iter()
                .map(|tag| tag.trim().to_owned())
                .collect(),
        }
        .normalized()?;
        let mut tags = organization
            .tags
            .into_iter()
            .map(|tag| tag.to_lowercase())
            .collect::<Vec<_>>();
        tags.sort();
        Ok(Self {
            query: filter.query.to_lowercase(),
            category: filter
                .category
                .map(|_| organization.category.to_lowercase()),
            tags,
        })
    }

    fn matches(&self, entry: &prefabs::Entry) -> bool {
        let name_matches = entry.name.to_lowercase().contains(&self.query);
        let Ok(organization) = &entry.organization else {
            return self.category.is_none() && self.tags.is_empty() && name_matches;
        };
        let category = organization.category.to_lowercase();
        let tags = organization
            .tags
            .iter()
            .map(|tag| tag.to_lowercase())
            .collect::<Vec<_>>();
        self.category
            .as_ref()
            .is_none_or(|filter| filter == &category)
            && self.tags.iter().all(|filter| tags.contains(filter))
            && (name_matches
                || category.contains(&self.query)
                || tags.iter().any(|tag| tag.contains(&self.query)))
    }

    fn report(&self) -> Value {
        json!({"query":self.query,"category":self.category,"tags":self.tags})
    }

    fn uses_organization(&self) -> bool {
        !self.query.is_empty() || self.category.is_some() || !self.tags.is_empty()
    }
}

fn organization_catalog(entries: &[prefabs::Entry]) -> (Vec<String>, Vec<String>) {
    let mut categories = BTreeMap::new();
    let mut tags = BTreeMap::new();
    for entry in entries {
        let Ok(organization) = &entry.organization else {
            continue;
        };
        categories
            .entry(organization.category.to_lowercase())
            .or_insert_with(|| organization.category.clone());
        for tag in &organization.tags {
            tags.entry(tag.to_lowercase())
                .or_insert_with(|| tag.clone());
        }
    }
    (
        categories.into_values().collect(),
        tags.into_values().collect(),
    )
}

fn add_organization_status(
    report: &mut Value,
    entries: &[prefabs::Entry],
    filter: Option<&FilterKey>,
) {
    let errors = entries
        .iter()
        .filter_map(|entry| {
            entry
                .organization
                .as_ref()
                .err()
                .map(|error| json!({"name":entry.name,"error":error}))
        })
        .collect::<Vec<_>>();
    let unknown = errors.len();
    let complete = filter.is_some_and(|filter| unknown == 0 || !filter.uses_organization());
    report["complete"] = json!(complete);
    report["catalog_complete"] = json!(unknown == 0);
    report["unknown_organization_count"] = json!(unknown);
    report["organization_errors"] = json!(errors);
    report["filter_warning"] = json!(if unknown == 0 {
        None
    } else if complete {
        Some(format!(
            "Organization is unavailable for {unknown} prefabs; category and tag catalogs are incomplete. All prefab names remain listed."
        ))
    } else {
        Some(format!(
            "Organization is unavailable for {unknown} prefabs; query, category or tag matches and their catalogs may be incomplete. Name-matching entries remain visible when no category or tag filter is active."
        ))
    });
}

fn require_organization(snapshot: &prefabs::RegistrySnapshot, name: &str) -> Result<(), String> {
    let entry = snapshot
        .entries
        .iter()
        .find(|entry| entry.name == name)
        .ok_or("Prefab no longer exists; refresh the library")?;
    entry
        .organization
        .as_ref()
        .map(|_| ())
        .map_err(|error| format!("Prefab {name} organization is unavailable: {error}"))
}

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
            "prefab-v2:{}:{}:{}:{}",
            self.buffer_review_scope,
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
        self.prefab_report_filtered(query, None, &[], offset, limit, selected)
    }

    pub fn prefab_report_filtered(
        &self,
        query: &str,
        category: Option<&str>,
        tags: &[String],
        offset: usize,
        limit: usize,
        selected: Option<&str>,
    ) -> Value {
        let snapshot = self.prefabs.binding.authority().snapshot();
        self.prefab_report_from_snapshot_filtered(
            snapshot,
            Filter {
                query,
                category,
                tags,
            },
            offset,
            limit,
            selected,
        )
    }

    fn prefab_report_from_snapshot(
        &self,
        snapshot: prefabs::RegistrySnapshot,
        query: &str,
        offset: usize,
        limit: usize,
        selected: Option<&str>,
    ) -> Value {
        self.prefab_report_from_snapshot_filtered(
            snapshot,
            Filter {
                query,
                category: None,
                tags: &[],
            },
            offset,
            limit,
            selected,
        )
    }

    fn prefab_report_from_snapshot_filtered(
        &self,
        snapshot: prefabs::RegistrySnapshot,
        filter: Filter<'_>,
        offset: usize,
        limit: usize,
        selected: Option<&str>,
    ) -> Value {
        let filter = FilterKey::new(filter);
        let (categories, tags) = organization_catalog(&snapshot.entries);
        let mut result = json!({"limits":{"entries":prefabs::MAX_ENTRIES,"file_bytes":transfer::MAX_BYTES,"library_bytes":prefabs::MAX_TOTAL_BYTES,"scanned_directory_entries":prefabs::MAX_SCAN,"name_bytes":prefabs::MAX_NAME_BYTES,"query_bytes":MAX_QUERY_BYTES,"category_bytes":prefabs::MAX_CATEGORY_BYTES,"tags":prefabs::MAX_TAGS,"tag_bytes":prefabs::MAX_TAG_BYTES},"selected_objects":self.selected_ids().iter().copied().map(crate::identity::id_string).collect::<Vec<_>>(),"directory":snapshot.directory.as_ref().map(|path|path.to_string_lossy()),"registry_revision":snapshot.revision,"write_error":snapshot.write_error,"categories":categories,"tags":tags,"catalog_total":snapshot.entries.len(),"entries":[]});
        add_organization_status(&mut result, &snapshot.entries, filter.as_ref().ok());
        match snapshot.error.as_ref() {
            Some(error) => {
                result["error"] = json!(error);
                result["entries"] = json!([]);
                result["complete"] = json!(false);
                result["catalog_complete"] = json!(false);
            }
            None => {
                let stamp = self.prefab_stamp(&snapshot);
                result["snapshot"] = json!(stamp);
                result["library_bytes"] = json!(snapshot.bytes);
                match &filter {
                    Ok(filter) => {
                        let entries = snapshot
                            .entries
                            .iter()
                            .filter(|entry| filter.matches(entry))
                            .collect::<Vec<_>>();
                        let limit = limit.clamp(1, MAX_REPORT_ENTRIES);
                        result["filter"] = filter.report();
                        result["total"] = json!(entries.len());
                        result["next_offset"] = json!(
                            (offset.saturating_add(limit) < entries.len())
                                .then_some(offset.saturating_add(limit))
                        );
                        result["entries"] = json!(entries.into_iter().skip(offset).take(limit)
                            .map(|entry|json!({"name":entry.name,"bytes":entry.bytes,"organization":entry.organization.as_ref().ok(),"organization_error":entry.organization.as_ref().err()})).collect::<Vec<_>>());
                    }
                    Err(error) => result["error"] = json!(error),
                }
            }
        }
        if let Some(name) = selected {
            if let Some(error) = &snapshot.error {
                result["selected_exists"] = Value::Null;
                result["selection_error"] = json!(error);
            } else {
                let entry = snapshot.entries.iter().find(|entry| entry.name == name);
                result["selected_exists"] = json!(entry.is_some());
                result["selected_organization"] =
                    json!(entry.and_then(|entry| entry.organization.as_ref().ok()));
                result["selected_organization_error"] =
                    json!(entry.and_then(|entry| entry.organization.as_ref().err()));
                let loaded = self
                    .prefabs
                    .binding
                    .authority()
                    .read(name, snapshot.revision, &snapshot.stamp)
                    .and_then(|(entry, text)| {
                        transfer::Payload::decode(&text, &self.definitions)
                            .map(|payload| (entry, payload))
                            .map_err(|error| error.to_string())
                    });
                match loaded {
                    Ok((entry, payload)) => {
                        result["selected"] = info(name, &payload);
                        result["selected"]["organization"] =
                            json!(entry.organization.as_ref().ok());
                        result["selected"]["organization_error"] =
                            json!(entry.organization.as_ref().err());
                        match payload.external(&self.definitions,&self.document) {
                            Ok(links) => result["selected"]["external_links"] = json!(links.iter().map(|link|json!({"source":link.source,"target":link.target,"destination_matches":link.matches})).collect::<Vec<_>>()),
                            Err(error) => result["selected"]["external_links_error"] = json!(error.to_string()),
                        }
                    }
                    Err(error) => result["selection_error"] = json!(error),
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
        let payload = self.capture_prefab_payload(ids)?;
        let committed = self.prefabs.binding.authority().change(
            snapshot.revision,
            &snapshot.stamp,
            prefabs::Edit::Save {
                name,
                text: payload.text(),
            },
        )?;
        self.log(actor, format!("saved prefab {name}"));
        let mut report =
            self.prefab_report_from_snapshot(committed, "", 0, MAX_REPORT_ENTRIES, Some(name));
        report["write_committed"] = json!(true);
        Ok(report)
    }

    fn capture_prefab_payload(&self, ids: &[ObjectId]) -> Result<transfer::Payload, String> {
        self.ensure_not_closing()?;
        self.require_root_objects(ids)?;
        transfer::Payload::capture(&self.document, ids, &self.definitions)
            .map_err(|error| error.to_string())
    }

    pub fn replace_prefab(
        &mut self,
        actor: Actor,
        name: &str,
        ids: &[ObjectId],
        stamp: &str,
    ) -> Result<Value, String> {
        let snapshot = self.checked_prefabs(stamp)?;
        require_organization(&snapshot, name)?;
        let payload = self.capture_prefab_payload(ids)?;
        let committed = self.prefabs.binding.authority().change(
            snapshot.revision,
            &snapshot.stamp,
            prefabs::Edit::Replace {
                name,
                text: payload.text(),
            },
        )?;
        self.log(actor, format!("replaced prefab {name}"));
        let mut report =
            self.prefab_report_from_snapshot(committed, "", 0, MAX_REPORT_ENTRIES, Some(name));
        report["write_committed"] = json!(true);
        Ok(report)
    }

    pub fn organize_prefab(
        &mut self,
        actor: Actor,
        name: &str,
        organization: &prefabs::Organization,
        stamp: &str,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        let snapshot = self.checked_prefabs(stamp)?;
        require_organization(&snapshot, name)?;
        let organization = organization.normalized()?;
        let committed = self.prefabs.binding.authority().change(
            snapshot.revision,
            &snapshot.stamp,
            prefabs::Edit::Organize {
                name,
                organization: &organization,
            },
        )?;
        self.log(actor, format!("organized prefab {name}"));
        let mut report =
            self.prefab_report_from_snapshot(committed, "", 0, MAX_REPORT_ENTRIES, Some(name));
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
        let mut report =
            self.prefab_report_from_snapshot(committed, "", 0, MAX_REPORT_ENTRIES, None);
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
        let mut report =
            self.prefab_report_from_snapshot(committed, "", 0, MAX_REPORT_ENTRIES, Some(new_name));
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
        detail["organization"] = json!(source.organization.as_ref().ok());
        detail["organization_error"] = json!(source.organization.as_ref().err());
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
            ..Default::default()
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
