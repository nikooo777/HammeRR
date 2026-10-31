use super::*;
use crate::color::Input;
use crate::color_palette::{self as palette, Authority, PaletteChange, PaletteSource};
use map_core::color_palette::{Entry, Metadata};
use serde_json::{Value, json};

pub(super) enum Binding {
    Independent(Authority),
    Application(Authority),
}

impl Default for Binding {
    fn default() -> Self {
        Self::Independent(Authority::default())
    }
}

impl Binding {
    fn authority(&self) -> &Authority {
        match self {
            Self::Independent(authority) | Self::Application(authority) => authority,
        }
    }
}

impl Session {
    pub(crate) fn use_application_color_palette(&mut self, authority: Authority) {
        self.color_palette = Binding::Application(authority);
    }

    pub(super) fn configure_color_palette(&mut self, directory: Result<PathBuf, String>) {
        if let Binding::Independent(authority) = &self.color_palette {
            let _ = authority.configure(directory);
        }
    }

    pub fn color_palette_state(&self) -> serde_json::Value {
        self.color_palette_state_for(PaletteSource::User)
    }

    pub fn color_palette_state_for(&self, source: PaletteSource) -> serde_json::Value {
        let mut report = match source {
            PaletteSource::User => self.color_palette.authority().report(),
            PaletteSource::Map => {
                let parsed = self.map_color_palette();
                let (entries, error) = match parsed {
                    Ok((_, entries)) => (json!(palette::entries_report(&entries)), None),
                    Err(error) => (Value::Null, Some(error)),
                };
                json!({"snapshot":self.map_color_palette_snapshot(),"entries":entries,
                    "ready":error.is_none(),"error":error,
                    "stored":self.document.vmf().extra.iter().any(|node|node.key == map_core::color_palette::KEY),
                    "path":self.path.as_ref().map(|path|path.display().to_string()),
                    "reload_available":true,"write_committed":false,
                    "limits":{"entries":palette::MAX_ENTRIES,"name_characters":palette::MAX_NAME_CHARACTERS,"name_bytes":palette::MAX_NAME_BYTES},
                    "policy":"This map's exact normalized RGB colors. Save, Replace, Rename and Delete are independent undoable map edits; save the map to persist them. Cancelling a color/property draft does not undo palette edits. Use changes only the local color draft. Brightness is independent. Invalid or unsupported metadata is preserved and unavailable."})
            }
        };
        report["source"] = json!(source.name());
        report
    }

    fn map_color_palette_snapshot(&self) -> String {
        format!(
            "map-palette-v1:{}:{}:{}",
            self.map_palette_scope,
            self.document_generation,
            self.document.revision()
        )
    }

    fn map_color_palette(&self) -> Result<(Metadata, palette::Entries), String> {
        let metadata = Metadata::parse(self.document.vmf())?;
        let entries = palette::validated_entries(
            metadata
                .entries
                .iter()
                .map(|entry| (entry.name.clone(), entry.rgb1)),
        )?;
        Ok((metadata, entries))
    }

    fn edit_map_color_palette(
        &mut self,
        actor: Actor,
        action: &str,
        metadata: Metadata,
        property_token: Option<u64>,
    ) -> Result<(), String> {
        if self.viewport_pick_conflict()
            || self.measurement_picking()
            || self.linked_path_picking()
            || self.block_preview.is_some()
            || self.clip_drag.is_some()
            || self.geometry.entity_placement.has_draft()
            || self.geometry.instance_collapse.worker.busy()
            || !std::ptr::eq(self.render_document(), &self.document)
        {
            return Err("Finish or cancel active geometry drafts and viewport gestures before changing the map palette".into());
        }
        if let Some(kind) = self
            .document_close_review()
            .first_draft_except("properties")
        {
            return Err(format!(
                "Apply or cancel the active {kind} draft before changing the map palette"
            ));
        }
        if self.property_draft.is_some() || property_token.is_some() {
            self.check_property_draft(property_token.ok_or("A property draft is open; change the map palette from its color picker or supply its property_token")?)?;
        }
        self.edit(actor, &format!("{action} map palette color"), |tx| {
            metadata.write(tx)
        })?;
        if let Some(token) = property_token {
            let revision = self.document.revision();
            if let Some(draft) = &mut self.property_draft
                && draft.token == token
            {
                draft.revision = revision;
                self.management_serial += 1;
            }
        }
        Ok(())
    }

    pub fn change_color_palette_for(
        &mut self,
        actor: Actor,
        source: PaletteSource,
        change: PaletteChange<'_>,
    ) -> Result<serde_json::Value, String> {
        self.ensure_not_closing()?;
        let PaletteChange {
            action,
            snapshot,
            name,
            color,
            new_name,
            property_token,
        } = change;
        if source == PaletteSource::User {
            if property_token.is_some() {
                return Err("The user palette does not accept property_token".into());
            }
            return self
                .color_palette
                .authority()
                .change_named(action, snapshot, name, color, new_name);
        }
        let converted = palette::prepare_change(action, name, color, new_name)?;
        if action == "reload" {
            if property_token.is_some() {
                return Err("Refresh palette does not accept property_token".into());
            }
            self.map_color_palette()?;
            return Ok(self.color_palette_state_for(source));
        }
        if snapshot != self.map_color_palette_snapshot() {
            return Err("Map changed; refresh the saved color choices before retrying".into());
        }
        let (mut metadata, mut entries) = self.map_color_palette()?;
        if action == "use" {
            if property_token.is_some() {
                return Err("Use does not accept property_token".into());
            }
            let rgb1 = entries
                .get(name.unwrap())
                .ok_or("The saved color no longer exists")?;
            let color = crate::color::convert(Input {
                space: crate::color::Space::Rgb1,
                channels: *rgb1,
            })?;
            return Ok(json!({"color":color,"palette":self.color_palette_state_for(source)}));
        }
        let name = name.expect("Palette action has a name");
        palette::change_entries(&mut entries, action, name, converted, new_name)?;
        if action == "rename" {
            let entry = metadata
                .entries
                .iter_mut()
                .find(|entry| entry.name == name)
                .expect("Renamed entry exists");
            entry.name = new_name.unwrap().into();
        }
        metadata
            .entries
            .retain(|entry| entries.contains_key(&entry.name));
        for (name, rgb1) in entries {
            if let Some(entry) = metadata.entries.iter_mut().find(|entry| entry.name == name) {
                entry.rgb1 = rgb1;
            } else {
                metadata.entries.push(Entry {
                    name,
                    rgb1,
                    extra: Vec::new(),
                });
            }
        }
        self.edit_map_color_palette(actor, action, metadata, property_token)?;
        Ok(self.color_palette_state_for(source))
    }

    pub fn change_color_palette(
        &mut self,
        action: &str,
        snapshot: &str,
        name: Option<&str>,
        color: Option<Input>,
    ) -> Result<serde_json::Value, String> {
        self.ensure_not_closing()?;
        self.color_palette
            .authority()
            .change(action, snapshot, name, color)
    }

    pub fn rename_color_palette(
        &mut self,
        snapshot: &str,
        name: &str,
        new_name: &str,
    ) -> Result<serde_json::Value, String> {
        self.ensure_not_closing()?;
        self.color_palette
            .authority()
            .rename(snapshot, name, new_name)
    }
}
