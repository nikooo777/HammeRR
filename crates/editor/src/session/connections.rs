use std::collections::BTreeMap;

use map_core::connections::{self, Spec};
use serde::Serialize;
use source_formats::fgd::{Io, Property};

use super::{Actor, Session};
use source_formats::vmf::Connection;

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionFields {
    pub output: String,
    pub target: String,
    pub input: String,
    pub parameter: String,
    pub delay: String,
    pub times: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use map_core::ObjectId;
    use source_formats::fgd::Catalog;

    const FGD: &str = r#"
@BaseClass = Named [ targetname(string) : "Name" input Enable(void) : "Enable this entity" ]
@PointClass base(Named) = logic_relay [ input Trigger(void) : "Trigger relay" output OnTrigger(void) : "When triggered" ]
@PointClass base(Named) = math_counter [ input SetValue(float) : "Set value" ]
"#;

    fn fixture() -> (Session, u32, u32) {
        let mut session = Session::new(Err("no assets".into()));
        session.set_definitions(Actor::User, Catalog::parse(FGD).unwrap());
        let ObjectId::Entity(source) = session
            .create_entity(
                Actor::User,
                "logic_relay",
                Some(DVec3::ZERO),
                BTreeMap::new(),
            )
            .unwrap()
        else {
            panic!()
        };
        let ObjectId::Entity(target) = session
            .create_entity(
                Actor::User,
                "math_counter",
                Some(DVec3::X * 64.0),
                BTreeMap::from([("targetname".into(), "counter".into())]),
            )
            .unwrap()
        else {
            panic!()
        };
        (session, source, target)
    }

    fn fields() -> ConnectionFields {
        ConnectionFields {
            output: "OnTrigger".into(),
            target: "counter".into(),
            input: "SetValue".into(),
            parameter: "7.5".into(),
            delay: "0.25".into(),
            times: "1".into(),
        }
    }

    #[test]
    fn both_actors_share_drafts_cancel_undo_and_references() {
        for actor in [Actor::User, Actor::Agent] {
            let (mut session, source, target) = fixture();
            let before = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            session.begin_connection_edit(source, None).unwrap();
            session.update_connection_draft(fields()).unwrap();
            assert_eq!(session.connection_draft_feedback(), (None, vec![]));
            assert_eq!(session.document.vmf(), &before);
            session.cancel_connection_edit();
            assert_eq!(session.document.undo_labels().count(), history);
            session.begin_connection_edit(source, None).unwrap();
            session.update_connection_draft(fields()).unwrap();
            assert_eq!(session.commit_connection_edit(actor).unwrap(), 0);
            assert_eq!(session.document.undo_labels().count(), history + 1);
            assert_eq!(
                session.incoming_connections(target).unwrap()[0].source,
                source
            );
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &before);
            session.redo(actor).unwrap();
            session.delete_entity_connection(actor, source, 0).unwrap();
            assert!(session.incoming_connections(target).unwrap().is_empty());
            session.undo(actor).unwrap();
            assert_eq!(session.connection_records(source).unwrap().len(), 1);
        }
    }

    #[test]
    fn invalid_fields_and_stale_indices_never_change_the_map() {
        let (mut session, source, target) = fixture();
        session.begin_connection_edit(source, None).unwrap();
        let mut draft = fields();
        draft.delay = "NaN".into();
        session.update_connection_draft(draft).unwrap();
        let before = session.document.vmf().clone();
        assert!(session.connection_draft_feedback().0.is_some());
        assert!(session.commit_connection_edit(Actor::Agent).is_err());
        assert_eq!(session.document.vmf(), &before);
        session.update_connection_draft(fields()).unwrap();
        session
            .edit(Actor::Agent, "move target", |tx| {
                map_core::edit::translate(tx, &[ObjectId::Entity(target)], DVec3::X, true)
            })
            .unwrap();
        session.undo(Actor::Agent).unwrap();
        assert!(
            session
                .commit_connection_edit(Actor::User)
                .unwrap_err()
                .contains("Map changed")
        );
        assert_eq!(session.document.vmf(), &before);
        session.new_map(Actor::Agent, true).unwrap();
        assert!(session.connection_draft().is_none());
    }

    #[test]
    fn choices_are_inherited_and_warnings_allow_custom_logic() {
        let (mut session, source, _) = fixture();
        assert!(
            session
                .connection_inputs(source, "counter")
                .iter()
                .any(|input| input.name == "Enable")
        );
        assert!(
            session
                .connection_targets("math_counter")
                .iter()
                .any(|(name, _)| name == "counter")
        );
        session.begin_connection_edit(source, None).unwrap();
        let mut draft = fields();
        draft.parameter = "not a float".into();
        session.update_connection_draft(draft.clone()).unwrap();
        assert!(session.connection_draft_feedback().1[0].contains("expects float"));
        draft.output = "CustomOutput".into();
        draft.target = "spawned_in_game".into();
        draft.input = "CustomInput".into();
        session.update_connection_draft(draft).unwrap();
        assert!(session.connection_draft_feedback().0.is_none());
        assert_eq!(session.connection_draft_feedback().1.len(), 2);
        session.commit_connection_edit(Actor::Agent).unwrap();
        assert_eq!(
            session.connection_records(source).unwrap()[0]
                .warnings
                .len(),
            2
        );
    }

    #[test]
    fn brush_entity_connections_preserve_geometry_and_unknown_properties() {
        let (mut session, _, target) = fixture();
        let brush = session
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let ObjectId::Entity(source) = session
            .tie_to_entity(
                Actor::User,
                &[brush],
                "func_button",
                BTreeMap::from([("custom".into(), "keep".into())]),
            )
            .unwrap()
        else {
            panic!()
        };
        let before = session.document.entity(source).unwrap().clone();
        session.begin_connection_edit(source, None).unwrap();
        let mut draft = fields();
        draft.output = "OnPressed".into();
        session.update_connection_draft(draft).unwrap();
        session.commit_connection_edit(Actor::Agent).unwrap();
        let after = session.document.entity(source).unwrap();
        assert_eq!(after.solids, before.solids);
        assert_eq!(after.properties.get("custom"), Some("keep"));
        assert_eq!(
            session.incoming_connections(target).unwrap()[0].source,
            source
        );
        session.undo(Actor::User).unwrap();
        assert_eq!(session.document.entity(source).unwrap(), &before);
    }
}

impl ConnectionFields {
    pub fn spec(&self) -> Result<Spec, String> {
        Ok(Spec {
            output: self.output.trim().into(),
            target: self.target.trim().into(),
            input: self.input.trim().into(),
            parameter: self.parameter.clone(),
            delay: self
                .delay
                .trim()
                .parse()
                .map_err(|_| "Enter a delay in seconds")?,
            times: self
                .times
                .trim()
                .parse()
                .map_err(|_| "Enter -1 (unlimited) or a positive firing count")?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionDraft {
    pub token: u64,
    pub source: u32,
    pub index: Option<usize>,
    pub fields: ConnectionFields,
    pub generation: u64,
    pub revision: u64,
}

pub struct ConnectionRecord {
    pub source: u32,
    pub index: usize,
    pub raw: source_formats::vmf::Connection,
    pub spec: Option<Spec>,
    pub targets: Vec<u32>,
    pub warnings: Vec<String>,
}

impl Session {
    pub fn connection_serial(&self) -> u64 {
        self.connection_serial
    }

    pub fn connection_draft(&self) -> Option<&ConnectionDraft> {
        self.connection_draft.as_ref()
    }

    pub fn connection_outputs(&self, source: u32) -> Vec<Io> {
        self.document
            .entity(source)
            .and_then(|entity| entity.classname())
            .and_then(|name| self.definitions.class(name))
            .map(|class| class.outputs.clone())
            .unwrap_or_default()
    }

    pub fn connection_inputs(&self, source: u32, target: &str) -> Vec<Io> {
        let mut inputs = BTreeMap::new();
        for id in connections::targets(&self.document, source, target) {
            if let Some(class) = self
                .document
                .entity(id)
                .and_then(|entity| entity.classname())
                .and_then(|name| self.definitions.class(name))
            {
                for input in &class.inputs {
                    inputs
                        .entry(input.name.to_ascii_lowercase())
                        .or_insert_with(|| input.clone());
                }
            }
        }
        inputs.into_values().collect()
    }

    pub fn connection_targets(&self, query: &str) -> Vec<(String, String)> {
        let mut names: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for entity in &self.document.vmf().entities {
            let name = entity.properties.get("targetname").unwrap_or_default();
            if !entity.hidden && !name.is_empty() {
                names.entry(name.into()).or_default().push(format!(
                    "{} · entity:{}",
                    entity.classname().unwrap_or("unknown class"),
                    entity.id
                ));
            }
        }
        let mut choices: Vec<_> = names
            .into_iter()
            .map(|(name, entities)| (name, entities.join("; ")))
            .collect();
        choices.extend([
            ("!self".into(), "The entity sending this output".into()),
            (
                "!activator".into(),
                "The entity that activated this event; resolved in game".into(),
            ),
            (
                "!caller".into(),
                "The caller of this event; resolved in game".into(),
            ),
            (
                "!player".into(),
                "The local player; resolved in game".into(),
            ),
        ]);
        let query = query.to_ascii_lowercase();
        choices.retain(|(name, description)| {
            name.to_ascii_lowercase().contains(&query)
                || description.to_ascii_lowercase().contains(&query)
        });
        choices
    }

    fn connection_warnings(&self, source: u32, spec: &Spec) -> Vec<String> {
        self.connection_feedback(source, spec, true)
    }

    pub(super) fn connection_problems(&self, source: u32, spec: &Spec) -> Vec<String> {
        self.connection_feedback(source, spec, false)
    }

    fn connection_feedback(&self, source: u32, spec: &Spec, include_context: bool) -> Vec<String> {
        let check_definitions = include_context || self.definitions.classes().next().is_some();
        let mut warnings = Vec::new();
        if check_definitions
            && !self
                .connection_outputs(source)
                .iter()
                .any(|output| output.name.eq_ignore_ascii_case(&spec.output))
        {
            warnings.push(format!(
                "{} is not declared in the source FGD; custom outputs are allowed",
                spec.output
            ));
        }
        let targets = connections::targets(&self.document, source, &spec.target);
        if spec.target.starts_with('!') && spec.target != "!self" {
            if include_context {
                warnings.push(
                    "Target is resolved in game; its inputs cannot be checked against map entities"
                        .into(),
                );
            }
        } else if targets.is_empty() {
            warnings.push(format!(
                "No visible map entity matches {}; it may be created in game",
                spec.target
            ));
        }
        if include_context && targets.len() > 1 {
            warnings.push(format!(
                "This output addresses {} map entities",
                targets.len()
            ));
        }
        for id in targets {
            if !check_definitions {
                continue;
            }
            let class = self
                .document
                .entity(id)
                .and_then(|entity| entity.classname())
                .and_then(|name| self.definitions.class(name));
            let input = class.and_then(|class| {
                class
                    .inputs
                    .iter()
                    .find(|input| input.name.eq_ignore_ascii_case(&spec.input))
            });
            let Some(input) = input else {
                warnings.push(format!(
                    "{} is not declared for entity:{id}; custom inputs are allowed",
                    spec.input
                ));
                continue;
            };
            if !spec.parameter.is_empty() {
                if input.kind == "void" {
                    warnings.push(format!("{} takes no parameter (entity:{id})", input.name));
                } else {
                    let property = Property {
                        name: input.name.clone(),
                        kind: input.kind.clone(),
                        label: input.name.clone(),
                        description: input.description.clone(),
                        default: None,
                        choices: Vec::new(),
                        read_only: false,
                    };
                    if let Err(error) = property.validate(&spec.parameter) {
                        warnings.push(format!("entity:{id}: {error}"));
                    }
                }
            }
        }
        if include_context
            && self
                .document
                .entity(source)
                .is_some_and(|entity| entity.hidden)
        {
            warnings.push("Source entity is hidden and will not be compiled".into());
        }
        warnings
    }

    pub fn connection_records(&self, source: u32) -> Result<Vec<ConnectionRecord>, String> {
        let entity = self
            .document
            .entity(source)
            .ok_or("Source entity no longer exists")?;
        Ok(entity
            .connections
            .iter()
            .flatten()
            .enumerate()
            .map(|(index, raw)| self.connection_record(source, index, raw))
            .collect())
    }

    fn connection_record(&self, source: u32, index: usize, raw: &Connection) -> ConnectionRecord {
        let mut warnings = Vec::new();
        let targets = raw
            .fields()
            .next()
            .map(|target| connections::targets(&self.document, source, target))
            .unwrap_or_default();
        let spec = match connections::parse(raw) {
            Ok(spec) => {
                if let Err(error) = spec.validate(connections::separator(raw)) {
                    warnings.push(error);
                }
                warnings.extend(self.connection_warnings(source, &spec));
                Some(spec)
            }
            Err(error) => {
                warnings.push(format!(
                    "Imported connection: {error}. Edit to repair it, or leave it unchanged."
                ));
                None
            }
        };
        ConnectionRecord {
            source,
            index,
            raw: raw.clone(),
            spec,
            targets,
            warnings,
        }
    }

    pub fn incoming_connections(&self, target: u32) -> Result<Vec<ConnectionRecord>, String> {
        self.document
            .entity(target)
            .ok_or("Target entity no longer exists")?;
        connections::incoming(&self.document, target)
            .into_iter()
            .map(|(source, index)| {
                let raw = self
                    .document
                    .entity(source)
                    .and_then(|entity| entity.connections.as_ref())
                    .and_then(|connections| connections.get(index))
                    .ok_or("Connection no longer exists")?;
                Ok(self.connection_record(source, index, raw))
            })
            .collect()
    }

    pub fn begin_connection_edit(
        &mut self,
        source: u32,
        index: Option<usize>,
    ) -> Result<(), String> {
        let entity = self
            .document
            .entity(source)
            .ok_or("Source entity no longer exists")?;
        let fields = if let Some(index) = index {
            let raw = entity
                .connections
                .as_deref()
                .unwrap_or_default()
                .get(index)
                .ok_or("Connection no longer exists")?;
            let fields: Vec<_> = raw.fields().collect();
            let field = |index: usize| fields.get(index).copied().unwrap_or_default().to_owned();
            ConnectionFields {
                output: raw.output.clone(),
                target: field(0),
                input: field(1),
                parameter: field(2),
                delay: field(3),
                times: field(4),
            }
        } else {
            ConnectionFields {
                output: self
                    .connection_outputs(source)
                    .first()
                    .map(|output| output.name.clone())
                    .unwrap_or_default(),
                target: String::new(),
                input: String::new(),
                parameter: String::new(),
                delay: "0".into(),
                times: "-1".into(),
            }
        };
        self.connection_draft = Some(ConnectionDraft {
            token: self.connection_serial + 1,
            source,
            index,
            fields,
            generation: self.document_generation,
            revision: self.document.revision(),
        });
        self.connection_serial += 1;
        Ok(())
    }

    pub fn update_connection_draft(&mut self, fields: ConnectionFields) -> Result<(), String> {
        self.connection_draft
            .as_mut()
            .ok_or("Begin a connection edit first")?
            .fields = fields;
        self.connection_serial += 1;
        Ok(())
    }

    pub fn connection_draft_feedback(&self) -> (Option<String>, Vec<String>) {
        let Some(draft) = &self.connection_draft else {
            return (None, Vec::new());
        };
        let result = self
            .check_connection_snapshot(draft.generation, draft.revision)
            .and_then(|()| {
                let spec = draft.fields.spec()?;
                let separator = draft
                    .index
                    .and_then(|index| {
                        self.document
                            .entity(draft.source)?
                            .connections
                            .as_ref()?
                            .get(index)
                    })
                    .map_or(',', connections::separator);
                spec.validate(separator)?;
                Ok(spec)
            });
        match result {
            Ok(spec) => (None, self.connection_warnings(draft.source, &spec)),
            Err(error) => (Some(error), Vec::new()),
        }
    }

    pub fn check_connection_snapshot(&self, generation: u64, revision: u64) -> Result<(), String> {
        if generation != self.document_generation || revision != self.document.revision() {
            return Err(
                "Map changed. Cancel and choose the connection again before editing.".into(),
            );
        }
        Ok(())
    }

    pub fn check_connection_draft(&self, token: u64) -> Result<(), String> {
        if self
            .connection_draft
            .as_ref()
            .is_none_or(|draft| draft.token != token)
        {
            return Err("Connection draft changed. Choose the connection again.".into());
        }
        Ok(())
    }

    pub fn set_entity_connection(
        &mut self,
        actor: Actor,
        source: u32,
        index: Option<usize>,
        spec: &Spec,
    ) -> Result<usize, String> {
        self.edit(
            actor,
            if index.is_some() {
                "edit entity connection"
            } else {
                "add entity connection"
            },
            |tx| connections::set(tx, source, index, spec),
        )
    }

    pub fn delete_entity_connection(
        &mut self,
        actor: Actor,
        source: u32,
        index: usize,
    ) -> Result<(), String> {
        self.edit(actor, "delete entity connection", |tx| {
            connections::delete(tx, source, index)
        })?;
        self.cancel_connection_edit();
        Ok(())
    }

    pub fn commit_connection_edit(&mut self, actor: Actor) -> Result<usize, String> {
        let draft = self
            .connection_draft
            .clone()
            .ok_or("Begin a connection edit first")?;
        self.check_connection_snapshot(draft.generation, draft.revision)?;
        let index =
            self.set_entity_connection(actor, draft.source, draft.index, &draft.fields.spec()?)?;
        self.cancel_connection_edit();
        Ok(index)
    }

    pub fn cancel_connection_edit(&mut self) {
        if self.connection_draft.take().is_some() {
            self.connection_serial += 1;
        }
    }
}
