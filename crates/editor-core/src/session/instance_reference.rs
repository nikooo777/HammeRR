use compiler::instances::{self, ReferencePath, SourceLocation};
use serde_json::{Value, json};
use source_formats::vmf::Entity;

use super::*;

#[derive(Clone, Debug, Default)]
pub struct Changes {
    pub file: Option<String>,
    pub origin: Option<String>,
    pub angles: Option<String>,
}

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    form: Option<Form>,
}

struct Form {
    token: u64,
    owner: Actor,
    entity: Option<u32>,
    generation: u64,
    revision: u64,
    definitions: u64,
    saved: u64,
    path: Option<PathBuf>,
    location: Option<SourceLocation>,
    source_error: Option<String>,
    original: [String; 3],
    values: [String; 3],
    checked: Option<ReferencePath>,
    candidate_error: Option<String>,
}

impl Session {
    fn instance_reference_context(&self) -> Result<(), String> {
        if self.history_navigation_blocked().is_some()
            || self.property_draft.is_some()
            || self.rename_draft.is_some()
            || self.connection_draft.is_some()
        {
            return Err("Finish the active build, launch or editing draft before authoring an instance reference".into());
        }
        Ok(())
    }

    fn instance_reference_form(&self, token: u64, actor: Option<Actor>) -> Result<&Form, String> {
        let form = self
            .instance_reference
            .form
            .as_ref()
            .filter(|form| form.token == token)
            .ok_or("Instance reference form changed; reopen it")?;
        if actor.is_some_and(|actor| actor != form.owner) {
            return Err("Review the shared instance reference form before changing it".into());
        }
        if form.generation != self.document_generation
            || form.revision != self.document.revision()
            || form.definitions != self.definitions_serial
            || form.saved != self.history_save_serial
            || form.path != self.path
        {
            return Err("Map, saved source or definitions changed; cancel and reopen the instance reference form".into());
        }
        if let Some(location) = &form.location {
            location
                .verify_metadata()
                .map_err(|error| format!("Saved instance source changed: {error}"))?;
        }
        Ok(form)
    }

    fn instance_reference_candidate(&self, form: &Form) -> Result<Entity, String> {
        let mut entity = if let Some(id) = form.entity {
            let entity = self
                .document
                .entity(id)
                .ok_or("Instance helper is missing")?;
            if !entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("func_instance"))
            {
                return Err("Select an existing func_instance helper".into());
            }
            entity.clone()
        } else {
            let properties = BTreeMap::from([
                ("file".into(), form.values[0].clone()),
                ("origin".into(), form.values[1].clone()),
                ("angles".into(), form.values[2].clone()),
            ]);
            let (classname, properties) =
                self.entity_properties("func_instance", ClassKind::Point, properties)?;
            let mut entity = Entity::new(0, &classname);
            for (key, value) in properties {
                entity.properties.set(key, value);
            }
            entity
        };
        if form.entity.is_some() {
            for (index, key) in ["file", "origin", "angles"].into_iter().enumerate() {
                if form.values[index] != form.original[index] {
                    self.validate_property(
                        entity.classname().unwrap_or("func_instance"),
                        key,
                        Some(&form.values[index]),
                    )?;
                    entity.properties.set(key, &form.values[index]);
                }
            }
        }
        instances::helper_admission(&entity)?;
        Ok(entity)
    }

    pub fn begin_instance_reference(
        &mut self,
        actor: Actor,
        entity: Option<u32>,
    ) -> Result<Value, String> {
        self.instance_reference_context()?;
        if self
            .instance_reference
            .form
            .as_ref()
            .is_some_and(|form| form.owner != actor)
        {
            return Err(
                "Review or cancel the shared instance reference form before replacing it".into(),
            );
        }
        let values = if let Some(id) = entity {
            let entity = self
                .document
                .entity(id)
                .filter(|entity| {
                    entity
                        .classname()
                        .is_some_and(|name| name.eq_ignore_ascii_case("func_instance"))
                })
                .ok_or("Select an existing func_instance helper")?;
            [
                entity.properties.get("file").unwrap_or("").into(),
                entity.properties.get("origin").unwrap_or("0 0 0").into(),
                entity.properties.get("angles").unwrap_or("0 0 0").into(),
            ]
        } else {
            let point = self.placement_origin();
            [
                String::new(),
                format!("{} {} {}", point.x, point.y, point.z),
                "0 0 0".into(),
            ]
        };
        if values
            .iter()
            .any(|value| value.len() > instances::MAX_FIELD_BYTES)
        {
            return Err(
                "Instance reference fields exceed 512 bytes; correct the raw properties first"
                    .into(),
            );
        }
        let location = self
            .path
            .as_deref()
            .ok_or_else(|| {
                "Save the parent VMF before creating or editing instance references".into()
            })
            .and_then(|path| SourceLocation::capture(path).map_err(|error| error.to_string()));
        let (location, source_error) = match location {
            Ok(location) => (Some(location), None),
            Err(error) => (None, Some(error)),
        };
        self.instance_reference.serial = self.instance_reference.serial.wrapping_add(1);
        let token = self.instance_reference.serial;
        self.instance_reference.form = Some(Form {
            token,
            owner: actor,
            entity,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            saved: self.history_save_serial,
            path: self.path.clone(),
            location,
            source_error,
            original: values.clone(),
            values,
            checked: None,
            candidate_error: None,
        });
        let error = self
            .instance_reference_candidate(self.instance_reference.form.as_ref().unwrap())
            .err();
        self.instance_reference
            .form
            .as_mut()
            .unwrap()
            .candidate_error = error;
        self.instance_reference.serial += 1;
        Ok(self.get_instance_reference())
    }

    pub fn get_instance_reference(&self) -> Value {
        let Some(form) = &self.instance_reference.form else {
            return json!({"token":null,"valid":false,"contents_validation":"not_checked"});
        };
        let context = self.instance_reference_form(form.token, None);
        let error = self
            .instance_reference_context()
            .err()
            .or_else(|| context.err())
            .or_else(|| form.source_error.clone())
            .or_else(|| form.candidate_error.clone());
        let checked = form
            .checked
            .as_ref()
            .map(|reference| reference.verify_metadata());
        let presence_error = checked
            .as_ref()
            .and_then(|result| result.as_ref().err())
            .map(ToString::to_string);
        let components = |raw: &str| {
            source_formats::vmf::parse_vec3(raw)
                .filter(|value| value.is_finite() && value.as_vec3().is_finite())
                .map(|_| {
                    raw.split_whitespace()
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
        };
        json!({
            "token":form.token,"owner":form.owner.label(),"entity":form.entity.map(|id|format!("entity:{id}")),
            "mode":if form.entity.is_some(){"edit"}else{"create"},"valid":error.is_none() && presence_error.is_none(),"error":error,
            "file":form.values[0],"origin":form.values[1],"angles":form.values[2],
            "origin_components":components(&form.values[1]),"angle_components":components(&form.values[2]),
            "source":form.path.as_ref().map(|path|path.display().to_string()),
            "directory":form.path.as_ref().and_then(|path|path.parent()).map(|path|path.display().to_string()),
            "source_admitted":form.location.is_some(),"presence_checked":checked.as_ref().is_some_and(Result::is_ok),
            "presence_error":presence_error,"bytes":form.checked.as_ref().map(ReferencePath::bytes),
            "contents_validation":"not_checked",
            "limits":"Path and file presence only: explicit relative .vmf inside the saved referring parent tree, no child symlinks/search/substitution, at most 4 MiB. Full child semantics and aggregate/transformed geometry limits are checked only by explicit Refresh instance contents or Compile. No child editing, nested/runtime entities or replacements.",
        })
    }

    pub fn update_instance_reference(
        &mut self,
        actor: Actor,
        token: u64,
        changes: Changes,
    ) -> Result<Value, String> {
        self.instance_reference_context()?;
        self.instance_reference_form(token, Some(actor))?;
        if [&changes.file, &changes.origin, &changes.angles]
            .into_iter()
            .flatten()
            .any(|value| value.len() > instances::MAX_FIELD_BYTES || value.contains('\0'))
        {
            return Err("Instance reference fields must fit 512 UTF-8 bytes without NUL".into());
        }
        let form = self.instance_reference.form.as_mut().unwrap();
        for (index, value) in [changes.file, changes.origin, changes.angles]
            .into_iter()
            .enumerate()
        {
            if let Some(value) = value {
                if index == 0 && form.values[index] != value {
                    form.checked = None;
                }
                form.values[index] = value;
            }
        }
        let error = self
            .instance_reference_candidate(self.instance_reference.form.as_ref().unwrap())
            .err();
        self.instance_reference
            .form
            .as_mut()
            .unwrap()
            .candidate_error = error;
        Ok(self.get_instance_reference())
    }

    pub fn check_instance_reference(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.instance_reference_context()?;
        let form = self.instance_reference_form(token, Some(actor))?;
        self.instance_reference_candidate(form)?;
        let location = form.location.as_ref().ok_or_else(|| {
            form.source_error
                .clone()
                .unwrap_or("Save the parent first".into())
        })?;
        let reference = location
            .reference(&form.values[0])
            .map_err(|error| error.to_string())?;
        self.instance_reference.form.as_mut().unwrap().checked = Some(reference);
        self.instance_reference.serial += 1;
        Ok(self.get_instance_reference())
    }

    pub fn instance_reference_file(
        &mut self,
        actor: Actor,
        token: u64,
        selected: &Path,
    ) -> Result<Value, String> {
        self.instance_reference_context()?;
        let form = self.instance_reference_form(token, Some(actor))?;
        let location = form.location.as_ref().ok_or_else(|| {
            form.source_error
                .clone()
                .unwrap_or("Save the parent first".into())
        })?;
        let reference = location
            .reference_from_path(selected)
            .map_err(|error| error.to_string())?;
        let form = self.instance_reference.form.as_mut().unwrap();
        form.values[0] = reference.relative().into();
        form.checked = Some(reference);
        let error = self
            .instance_reference_candidate(self.instance_reference.form.as_ref().unwrap())
            .err();
        self.instance_reference
            .form
            .as_mut()
            .unwrap()
            .candidate_error = error;
        self.instance_reference.serial += 1;
        Ok(self.get_instance_reference())
    }

    pub fn review_instance_reference(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        if self
            .instance_reference
            .form
            .as_ref()
            .is_none_or(|form| form.token != token)
        {
            return Err("Instance reference form changed; inspect it again".into());
        }
        self.instance_reference.serial = self.instance_reference.serial.wrapping_add(1);
        let form = self.instance_reference.form.as_mut().unwrap();
        form.token = self.instance_reference.serial;
        form.owner = actor;
        Ok(self.get_instance_reference())
    }

    pub fn cancel_instance_reference(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        if self
            .instance_reference
            .form
            .as_ref()
            .is_none_or(|form| form.token != token || form.owner != actor)
        {
            return Err("Instance reference form belongs to another owner or was replaced".into());
        }
        self.instance_reference.form = None;
        Ok(json!({"cancelled":true}))
    }

    pub fn apply_instance_reference(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.instance_reference_context()?;
        let form = self.instance_reference_form(token, Some(actor))?;
        let candidate = self.instance_reference_candidate(form)?;
        let location = form.location.as_ref().ok_or_else(|| {
            form.source_error
                .clone()
                .unwrap_or("Save the parent first".into())
        })?;
        let reference = match &form.checked {
            Some(reference) => reference.clone(),
            None => location
                .reference(&form.values[0])
                .map_err(|error| error.to_string())?,
        };
        reference
            .verify_metadata()
            .map_err(|error| error.to_string())?;
        let id = if let Some(id) = form.entity {
            let changes = ["file", "origin", "angles"]
                .into_iter()
                .enumerate()
                .filter(|(index, _)| form.values[*index] != form.original[*index])
                .map(|(_, key)| (key.into(), candidate.properties.get(key).map(str::to_owned)))
                .collect::<Vec<_>>();
            self.set_entity_properties(actor, id, &changes)?;
            ObjectId::Entity(id)
        } else {
            let properties = candidate
                .properties
                .iter()
                .filter(|(key, _)| !key.eq_ignore_ascii_case("classname"))
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect::<Vec<_>>();
            let classname = candidate
                .classname()
                .ok_or("Instance classname is missing")?;
            let id = self.edit(actor, "create instance reference", |tx| {
                map_core::edit::create_entity(tx, classname, None, &properties, Vec::new())
            })?;
            self.select(&[id], SelectMode::Replace)?;
            id
        };
        self.instance_reference.form = None;
        Ok(
            json!({"id":crate::identity::id_string(id),"applied":true,"presence_checked":true,"contents_validation":"not_checked"}),
        )
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.form.as_ref().map(|form| {
            serde_json::json!([
                self.serial,
                form.token,
                form.owner.label(),
                form.values,
                form.checked.is_some(),
                form.candidate_error
            ])
        })
    }
}

#[cfg(test)]
mod tests;
