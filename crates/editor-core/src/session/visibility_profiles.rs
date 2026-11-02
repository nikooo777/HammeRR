use map_core::visibility_profiles::{self as profiles, Entry, Metadata};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Actor, ObjectFilter, Session, working_region};

const MAX_DIAGNOSTIC_BYTES: usize = 1024;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    pub filter: Option<ObjectFilter>,
    #[serde(default)]
    pub exclusions: Vec<ObjectFilter>,
}

impl Rules {
    fn validate(&self) -> Result<(), String> {
        working_region::Options {
            filter: self.filter.clone(),
            exclusions: self.exclusions.clone(),
            ..working_region::Options::default()
        }
        .validate()
    }
}

#[derive(Debug, Clone)]
pub enum Action {
    Save { name: String, replace: bool },
    Rename { name: String, new_name: String },
    Delete { name: String },
    Apply { name: String },
}

impl Session {
    fn parsed_visibility_profiles(&self) -> Result<(Metadata, Vec<Rules>), String> {
        let metadata = Metadata::parse(self.document.vmf())?;
        let rules = metadata
            .entries
            .iter()
            .map(|entry| {
                let rules: Rules = serde_json::from_str(&entry.payload).map_err(|error| {
                    format!(
                        "Visibility profile {} is invalid: {}",
                        entry.name,
                        map_core::entity_helpers::bounded_preview(
                            &error.to_string(),
                            MAX_DIAGNOSTIC_BYTES
                        )
                    )
                })?;
                rules.validate().map_err(|error| {
                    format!("Visibility profile {} is invalid: {error}", entry.name)
                })?;
                Ok(rules)
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok((metadata, rules))
    }

    pub fn visibility_profiles(&self) -> Result<Value, String> {
        let (metadata, rules) = self.parsed_visibility_profiles()?;
        let entries = metadata
            .entries
            .iter()
            .zip(rules)
            .map(|(entry, rules)| json!({"name":entry.name,"rules":rules}))
            .collect::<Vec<_>>();
        Ok(json!({
            "snapshot":self.visibility_rules_snapshot(),
            "entries":entries,
            "stored":self.document.vmf().extra.iter().any(|node|node.key == profiles::KEY),
            "limits":{"profiles":profiles::MAX_PROFILES,"metadata_bytes":profiles::MAX_METADATA_BYTES,
                "name_characters":profiles::MAX_NAME_CHARACTERS,"name_bytes":profiles::MAX_NAME_BYTES,
                "exclusions":working_region::options::MAX_EXCLUSIONS}
        }))
    }

    fn visibility_profile_edit_admission(&self) -> Result<(), String> {
        self.inspection_viewport_ready("changing visibility profiles")?;
        if let Some(kind) = self.document_close_review().first_draft_except("") {
            return Err(format!(
                "Apply or cancel the active {kind} draft before changing visibility profiles"
            ));
        }
        if self.geometry.instance_collapse.worker.busy()
            || !std::ptr::eq(self.render_document(), &self.document)
        {
            return Err("Finish active geometry work before changing visibility profiles".into());
        }
        Ok(())
    }

    pub fn change_visibility_profile(
        &mut self,
        actor: Actor,
        action: Action,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.check_visibility_rules_snapshot(snapshot)?;
        let (mut metadata, rules) = self.parsed_visibility_profiles()?;
        let name = match &action {
            Action::Save { name, .. }
            | Action::Rename { name, .. }
            | Action::Delete { name }
            | Action::Apply { name } => name,
        };
        profiles::validate_name(name)?;
        let key = profiles::name_key(name);
        let existing = metadata
            .entries
            .iter()
            .position(|entry| profiles::name_key(&entry.name) == key);
        if matches!(action, Action::Apply { .. }) {
            let index = existing.ok_or("Choose an existing visibility profile to apply")?;
            let rules = &rules[index];
            let mut options = self.working_region.options.clone();
            options.filter = rules.filter.clone();
            options.exclusions = rules.exclusions.clone();
            let snapshot = self.working_region_stamp();
            self.set_working_region(
                actor,
                working_region::Action::Configure { options },
                &snapshot,
            )?;
            self.log(
                actor,
                format!("Apply visibility profile {}", metadata.entries[index].name),
            );
            return self.visibility_profiles();
        }
        self.visibility_profile_edit_admission()?;
        let label = match action {
            Action::Save { name, replace } => {
                let rules = Rules {
                    filter: self.working_region.options.filter.clone(),
                    exclusions: self.working_region.options.exclusions.clone(),
                };
                rules.validate()?;
                let payload = serde_json::to_string(&rules).map_err(|error| error.to_string())?;
                if replace {
                    let index =
                        existing.ok_or("Choose an existing visibility profile to replace")?;
                    metadata.entries[index].payload = payload;
                    "Replace visibility profile"
                } else {
                    if existing.is_some() {
                        return Err(
                            "This visibility profile name exists; choose Replace explicitly".into(),
                        );
                    }
                    metadata.entries.push(Entry {
                        name,
                        payload,
                        extra: Vec::new(),
                    });
                    "Save visibility profile"
                }
            }
            Action::Rename { new_name, .. } => {
                let index = existing.ok_or("Choose an existing visibility profile to rename")?;
                profiles::validate_name(&new_name)?;
                if metadata.entries[index].name == new_name {
                    return Err("Choose a different visibility profile name".into());
                }
                let key = profiles::name_key(&new_name);
                if metadata
                    .entries
                    .iter()
                    .enumerate()
                    .any(|(other, entry)| other != index && profiles::name_key(&entry.name) == key)
                {
                    return Err("This visibility profile name already exists".into());
                }
                metadata.entries[index].name = new_name;
                "Rename visibility profile"
            }
            Action::Delete { .. } => {
                let index = existing.ok_or("Choose an existing visibility profile to delete")?;
                metadata.entries.remove(index);
                "Delete visibility profile"
            }
            Action::Apply { .. } => unreachable!(),
        };
        metadata.validate()?;
        self.edit(actor, label, |tx| metadata.write(tx))?;
        self.visibility_profiles()
    }
}
