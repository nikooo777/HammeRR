use std::collections::BTreeSet;

use crate::materials::*;

use super::Session;

impl Session {
    pub fn material_catalog_provenance(&self, name: &str) -> Result<Provenance, String> {
        let name = crate::material_favorites::canonical_name(name)?;
        self.game
            .asset_origin(&source_formats::vmt::material_path(&name))
            .map(|origin| origin.map(provenance).unwrap_or_default())
            .map_err(|error| format!("Material catalog source {name}: {error}"))
    }

    pub fn material_catalog_filtered(
        &mut self,
        query: &str,
        group: &str,
        offset: usize,
        limit: usize,
        filter: &CatalogFilter,
    ) -> Result<Catalog, String> {
        let favorites = self.material_favorites_snapshot();
        self.material_catalog_filtered_with_favorites(
            query, group, offset, limit, filter, &favorites,
        )
    }

    pub fn material_catalog_filtered_with_favorites(
        &mut self,
        query: &str,
        group: &str,
        offset: usize,
        limit: usize,
        filter: &CatalogFilter,
        favorites: &crate::material_favorites::Snapshot,
    ) -> Result<Catalog, String> {
        if query.len() > MAX_QUERY_BYTES
            || group.len() > MAX_GROUP_BYTES
            || !(1..=MAX_MATERIAL_PAGE).contains(&limit)
        {
            return Err(format!(
                "Use a query up to {MAX_QUERY_BYTES} bytes, a group up to {MAX_GROUP_BYTES} bytes and a page of 1 to {MAX_MATERIAL_PAGE} materials"
            ));
        }
        let sources = catalog_sources(&self.game)?;
        validate_source_filter(filter.source.as_deref(), &sources)?;
        if self.material_index.is_none() {
            self.material_index = Some(self.search_materials("", usize::MAX)?);
        }
        let used = self
            .document
            .vmf()
            .solids()
            .flat_map(|solid| &solid.sides)
            .map(|side| {
                crate::material_favorites::canonical_name(&side.material)
                    .unwrap_or_else(|_| side.material.to_ascii_lowercase())
            })
            .collect::<BTreeSet<_>>();
        let names = self
            .material_index
            .as_ref()
            .unwrap()
            .iter()
            .chain(&used)
            .chain(&favorites.materials)
            .chain(&self.material_recent)
            .cloned()
            .collect::<BTreeSet<_>>();
        if names.len() > MAX_CATALOG_ENTRIES {
            return Err(format!(
                "Material catalog exceeds {MAX_CATALOG_ENTRIES} entries"
            ));
        }
        let query = query.trim().to_ascii_lowercase();
        let mut selected = BTreeSet::new();
        for name in names {
            if !name.contains(&query)
                || filter.exclude_generated_map_materials && name.starts_with("maps/")
            {
                continue;
            }
            if !matches!(filter.source.as_deref(), None | Some("all")) {
                let origin = self
                    .game
                    .asset_origin(&source_formats::vmt::material_path(&name))
                    .map_err(|error| format!("Material catalog source {name}: {error}"))?
                    .map(provenance)
                    .unwrap_or_default();
                if !source_matches(filter.source.as_deref(), &origin) {
                    continue;
                }
            }
            selected.insert(name);
        }
        let filtered_used = used.intersection(&selected).cloned().collect();
        let filtered_favorites = favorites
            .materials
            .intersection(&selected)
            .cloned()
            .collect();
        let filtered_recent = self
            .material_recent
            .iter()
            .filter(|name| selected.contains(*name))
            .cloned()
            .collect();
        let mut catalog = browse(
            &selected.into_iter().collect::<Vec<_>>(),
            &filtered_used,
            &filtered_favorites,
            &filtered_recent,
            "",
            group,
            (offset, limit),
        )?;
        for entry in &mut catalog.entries {
            entry.provenance = self
                .game
                .asset_origin(&source_formats::vmt::material_path(&entry.name))
                .map_err(|error| format!("Material catalog source {}: {error}", entry.name))?
                .map(provenance)
                .unwrap_or_default();
            if !source_matches(filter.source.as_deref(), &entry.provenance) {
                return Err(
                    "Material source changed during catalog lookup; retry the request".into(),
                );
            }
        }
        catalog.sources = sources;
        catalog.filter = filter.clone();
        catalog.favorites = favorites.status.clone();
        Ok(catalog)
    }
}
