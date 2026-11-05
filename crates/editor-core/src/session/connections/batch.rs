use super::*;
use map_core::{EditError, ObjectId};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const MAX_SOURCES: usize = 64;
const MAX_ROWS: usize = 256;
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_TARGET_MATCHES: usize = 4096;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionBatchAction {
    Add,
    Edit,
    Remove,
}

#[derive(Debug, Clone, Default, Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectionPatch {
    pub output: Option<String>,
    pub target: Option<String>,
    pub input: Option<String>,
    pub parameter: Option<String>,
    pub delay: Option<String>,
    pub times: Option<String>,
}

impl ConnectionPatch {
    fn empty(&self) -> bool {
        [
            &self.output,
            &self.target,
            &self.input,
            &self.parameter,
            &self.delay,
            &self.times,
        ]
        .iter()
        .all(|value| value.is_none())
    }

    fn apply(&self, fields: &ConnectionFields) -> ConnectionFields {
        ConnectionFields {
            output: self.output.clone().unwrap_or_else(|| fields.output.clone()),
            target: self.target.clone().unwrap_or_else(|| fields.target.clone()),
            input: self.input.clone().unwrap_or_else(|| fields.input.clone()),
            parameter: self
                .parameter
                .clone()
                .unwrap_or_else(|| fields.parameter.clone()),
            delay: self.delay.clone().unwrap_or_else(|| fields.delay.clone()),
            times: self.times.clone().unwrap_or_else(|| fields.times.clone()),
        }
    }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectionRowTarget {
    pub source: u32,
    pub identity: String,
}

#[derive(Debug, Clone, Serialize)]
struct CapturedRow {
    source: u32,
    index: usize,
    identity: String,
    output: String,
    value: String,
}

impl CapturedRow {
    fn fields(&self) -> ConnectionFields {
        ConnectionFields::from_raw(&Connection {
            output: self.output.clone(),
            value: self.value.clone(),
        })
    }

    fn raw(&self) -> Connection {
        Connection {
            output: self.output.clone(),
            value: self.value.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionBatchDraft {
    pub action: ConnectionBatchAction,
    pub sources: Vec<u32>,
    rows: Vec<CapturedRow>,
    pub patch: ConnectionPatch,
    pub definitions: u64,
}

struct PlannedRow {
    source: u32,
    index: Option<usize>,
    spec: Option<Spec>,
    warnings: Vec<String>,
}

fn entity_id(id: u32) -> String {
    crate::identity::id_string(ObjectId::Entity(id))
}

fn field_values(fields: &ConnectionFields) -> [&str; 6] {
    [
        &fields.output,
        &fields.target,
        &fields.input,
        &fields.parameter,
        &fields.delay,
        &fields.times,
    ]
}

impl Session {
    pub fn connection_batch_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.buffer_review_scope,
            self.document_generation,
            self.document.revision(),
            self.definitions_serial
        )
    }

    pub(super) fn connection_row_identity(
        &self,
        source: u32,
        index: usize,
        raw: &Connection,
    ) -> String {
        let mut digest = Sha256::new();
        digest.update(self.connection_batch_stamp().as_bytes());
        digest.update(source.to_le_bytes());
        digest.update(index.to_le_bytes());
        digest.update(raw.output.len().to_le_bytes());
        digest.update(raw.output.as_bytes());
        digest.update(raw.value.as_bytes());
        format!("connection:{:x}", digest.finalize())
    }

    fn batch_sources(&self, sources: Option<&[u32]>) -> Result<Vec<u32>, String> {
        self.document
            .vmf()
            .validate_ids()
            .map_err(|error| error.to_string())?;
        let sources = match sources {
            Some(sources) => sources.to_vec(),
            None => self
                .selected_ids()
                .into_iter()
                .map(|id| match id {
                    ObjectId::Entity(id) => Ok(id),
                    _ => Err("Capture whole entities for connection batches"),
                })
                .collect::<Result<Vec<_>, _>>()?,
        };
        let unique = sources.iter().copied().collect::<BTreeSet<_>>();
        if sources.is_empty() || sources.len() > MAX_SOURCES || sources.len() != unique.len() {
            return Err("Capture 1–64 distinct whole source entities".into());
        }
        for source in &unique {
            if self.document.entity(*source).is_none() {
                return Err(format!("Source entity:{source} no longer exists"));
            }
        }
        Ok(unique.into_iter().collect())
    }

    pub fn connection_batch_state(&self, sources: Option<&[u32]>) -> Result<Value, String> {
        let sources = self.batch_sources(sources)?;
        let count = sources
            .iter()
            .map(|source| {
                self.document
                    .entity(*source)
                    .unwrap()
                    .connections
                    .iter()
                    .flatten()
                    .count()
            })
            .sum::<usize>();
        if count > MAX_ROWS {
            return Err("Captured source entities exceed the 256-row connection limit".into());
        }
        let mut bytes = 0usize;
        let mut rows = Vec::new();
        for source in &sources {
            let entity = self.document.entity(*source).unwrap();
            for (index, raw) in entity.connections.iter().flatten().enumerate() {
                bytes = bytes
                    .checked_add(raw.output.len() + raw.value.len())
                    .filter(|bytes| *bytes <= MAX_TEXT_BYTES)
                    .ok_or("Captured connections exceed the 64 KiB text limit")?;
                rows.push(json!({"source":entity_id(*source),"index":index,"identity":self.connection_row_identity(*source,index,raw),"output":raw.output,"raw":raw.value,"fields":ConnectionFields::from_raw(raw)}));
            }
        }
        Ok(
            json!({"snapshot":self.connection_batch_stamp(),"sources":sources.into_iter().map(entity_id).collect::<Vec<_>>(),"rows":rows,"draft":self.connection_draft_report(),
            "limits":{"sources":MAX_SOURCES,"rows":MAX_ROWS,"text_bytes":MAX_TEXT_BYTES,"target_matches":MAX_TARGET_MATCHES},
            "policy":"Captured whole source entities and source-qualified row identities. Mixed fields remain unchanged unless explicitly replaced. Custom or unresolved I/O produces per-row warnings; no row is silently skipped."}),
        )
    }

    pub fn begin_connection_batch(
        &mut self,
        action: ConnectionBatchAction,
        sources: Option<&[u32]>,
        rows: &[ConnectionRowTarget],
        stamp: &str,
    ) -> Result<(), String> {
        if stamp != self.connection_batch_stamp() {
            return Err("Map or definitions changed; capture connections again".into());
        }
        let mut captured = Vec::new();
        let sources = if action == ConnectionBatchAction::Add {
            if !rows.is_empty() {
                return Err("Add uses captured source entities, not existing row targets".into());
            }
            self.batch_sources(sources)?
        } else {
            if sources.is_some() || rows.is_empty() || rows.len() > MAX_ROWS {
                return Err(
                    "Edit or Remove requires 1–256 exact source and row-identity pairs".into(),
                );
            }
            let mut seen = BTreeSet::new();
            let mut bytes = 0usize;
            for row in rows {
                if row.identity.len() > 128 || !seen.insert((row.source, row.identity.clone())) {
                    return Err("Connection row targets must be distinct exact identities".into());
                }
                let entity = self
                    .document
                    .entity(row.source)
                    .ok_or("A captured source entity no longer exists")?;
                let (index, raw) = entity
                    .connections
                    .iter()
                    .flatten()
                    .enumerate()
                    .find(|(index, raw)| {
                        self.connection_row_identity(row.source, *index, raw) == row.identity
                    })
                    .ok_or("A connection row identity is stale or belongs to another source")?;
                bytes = bytes
                    .checked_add(raw.output.len() + raw.value.len())
                    .filter(|bytes| *bytes <= MAX_TEXT_BYTES)
                    .ok_or("Captured connections exceed the 64 KiB text limit")?;
                captured.push(CapturedRow {
                    source: row.source,
                    index,
                    identity: row.identity.clone(),
                    output: raw.output.clone(),
                    value: raw.value.clone(),
                });
            }
            captured.sort_by_key(|row| (row.source, row.index));
            self.batch_sources(Some(
                &captured
                    .iter()
                    .map(|row| row.source)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))?
        };
        let fields = ConnectionFields {
            output: String::new(),
            target: String::new(),
            input: String::new(),
            parameter: String::new(),
            delay: "0".into(),
            times: "-1".into(),
        };
        self.connection_serial += 1;
        self.connection_draft = Some(ConnectionDraft {
            token: self.connection_serial,
            source: sources[0],
            index: None,
            fields,
            generation: self.document_generation,
            revision: self.document.revision(),
            batch: Some(ConnectionBatchDraft {
                action,
                sources,
                rows: captured,
                patch: ConnectionPatch::default(),
                definitions: self.definitions_serial,
            }),
        });
        Ok(())
    }

    pub fn update_connection_batch(
        &mut self,
        token: u64,
        patch: ConnectionPatch,
    ) -> Result<(), String> {
        self.check_connection_draft(token)?;
        let current = self
            .connection_draft
            .as_ref()
            .ok_or("Begin a connection batch first")?;
        self.check_connection_snapshot(current.generation, current.revision)?;
        let batch = current
            .batch
            .as_ref()
            .ok_or("The current connection draft is not a batch")?;
        if batch.definitions != self.definitions_serial {
            return Err(
                "Definitions changed; cancel and capture the connection batch again".into(),
            );
        }
        [
            &patch.output,
            &patch.target,
            &patch.input,
            &patch.parameter,
            &patch.delay,
            &patch.times,
        ]
        .into_iter()
        .flatten()
        .try_fold(0usize, |bytes, value| {
            bytes
                .checked_add(value.len())
                .filter(|bytes| *bytes <= MAX_TEXT_BYTES)
                .ok_or("Connection batch patch exceeds the 64 KiB text limit")
        })?;
        let fields =
            (batch.action == ConnectionBatchAction::Add).then(|| patch.apply(&current.fields));
        if let Some(fields) = &fields {
            field_values(fields)
                .into_iter()
                .try_fold(0usize, |bytes, value| {
                    bytes
                        .checked_add(value.len())
                        .filter(|bytes| *bytes <= MAX_TEXT_BYTES)
                        .ok_or("Connection batch fields exceed the 64 KiB text limit")
                })?;
        }
        let draft = self
            .connection_draft
            .as_mut()
            .ok_or("Begin a connection batch first")?;
        let batch = draft
            .batch
            .as_mut()
            .ok_or("The current connection draft is not a batch")?;
        if batch.action == ConnectionBatchAction::Remove {
            return Err("Remove has no replacement fields".into());
        }
        if let Some(fields) = fields {
            draft.fields = fields;
        }
        batch.patch = patch;
        self.connection_serial += 1;
        Ok(())
    }

    fn plan_connection_batch(&self) -> Result<Vec<PlannedRow>, String> {
        let draft = self
            .connection_draft
            .as_ref()
            .ok_or("Begin a connection batch first")?;
        let batch = draft
            .batch
            .as_ref()
            .ok_or("The current connection draft is not a batch")?;
        self.check_connection_snapshot(draft.generation, draft.revision)?;
        if batch.definitions != self.definitions_serial {
            return Err(
                "Definitions changed; cancel and capture the connection batch again".into(),
            );
        }
        self.batch_sources(Some(&batch.sources))?;
        if batch.action == ConnectionBatchAction::Edit && batch.patch.empty() {
            return Err(
                "Choose at least one field to replace; mixed fields are otherwise unchanged".into(),
            );
        }
        let requests = if batch.action == ConnectionBatchAction::Add {
            batch
                .sources
                .iter()
                .map(|source| (*source, None, draft.fields.clone(), ','))
                .collect::<Vec<_>>()
        } else {
            batch.rows.iter().map(|row| {
                let raw = self.document.entity(row.source).and_then(|entity| entity.connections.as_ref()).and_then(|rows|rows.get(row.index)).ok_or("A captured connection row no longer exists")?;
                if raw != &row.raw() || self.connection_row_identity(row.source,row.index,raw) != row.identity { return Err("A captured connection row changed; capture the batch again"); }
                if batch.action == ConnectionBatchAction::Edit && raw.fields().count() != 5 {
                    return Err("Repair malformed imported connection field counts in the single-row editor before batch editing");
                }
                Ok((row.source,Some(row.index),batch.patch.apply(&row.fields()),connections::separator(raw)))
            }).collect::<Result<Vec<_>,_>>()?
        };
        let mut planned = Vec::new();
        let mut matches = 0usize;
        let mut bytes = 0usize;
        for (source, index, fields, separator) in requests {
            let (spec, targets) = if batch.action == ConnectionBatchAction::Remove {
                (None, Vec::new())
            } else {
                for field in field_values(&fields) {
                    bytes = bytes
                        .checked_add(field.len())
                        .filter(|bytes| *bytes <= MAX_TEXT_BYTES)
                        .ok_or("Connection batch exceeds the 64 KiB text limit")?;
                }
                let mut spec = fields
                    .spec()
                    .map_err(|error| format!("entity:{source}: {error}"))?;
                if batch.action == ConnectionBatchAction::Edit {
                    let original = batch
                        .rows
                        .iter()
                        .find(|row| row.source == source && Some(row.index) == index)
                        .ok_or("A planned connection row lost its captured identity")?
                        .fields();
                    if batch.patch.output.is_none() {
                        spec.output = original.output;
                    }
                    if batch.patch.target.is_none() {
                        spec.target = original.target;
                    }
                    if batch.patch.input.is_none() {
                        spec.input = original.input;
                    }
                }
                spec.validate(separator)
                    .map_err(|error| format!("entity:{source}: {error}"))?;
                let targets = connections::targets_bounded(
                    &self.document,
                    source,
                    &spec.target,
                    MAX_TARGET_MATCHES - matches,
                )
                .map_err(|error| format!("entity:{source}: {error}"))?;
                matches += targets.len();
                (Some(spec), targets)
            };
            let warnings = spec.as_ref().map_or_else(Vec::new, |spec| {
                self.connection_feedback_with_targets(source, spec, true, &targets)
            });
            for warning in &warnings {
                bytes = bytes
                    .checked_add(warning.len())
                    .filter(|bytes| *bytes <= MAX_TEXT_BYTES)
                    .ok_or("Connection batch diagnostics exceed the 64 KiB text limit")?;
            }
            planned.push(PlannedRow {
                source,
                index,
                spec,
                warnings,
            });
        }
        Ok(planned)
    }

    pub(super) fn connection_batch_feedback(&self) -> (Option<String>, Vec<String>) {
        match self.plan_connection_batch() {
            Ok(rows) => (
                None,
                rows.into_iter()
                    .flat_map(|row| {
                        row.warnings.into_iter().map(move |warning| {
                            format!("entity:{} row {:?}: {warning}", row.source, row.index)
                        })
                    })
                    .collect(),
            ),
            Err(error) => (Some(error), Vec::new()),
        }
    }

    pub fn connection_batch_report(&self) -> Value {
        let Some(draft) = &self.connection_draft else {
            return Value::Null;
        };
        let Some(batch) = &draft.batch else {
            return Value::Null;
        };
        let planned = self.plan_connection_batch();
        let original = batch
            .rows
            .iter()
            .map(CapturedRow::fields)
            .collect::<Vec<_>>();
        let common = |index: usize| {
            original
                .first()
                .map(|fields| field_values(fields)[index].to_owned())
                .filter(|value| {
                    original
                        .iter()
                        .all(|fields| field_values(fields)[index] == value.as_str())
                })
        };
        let error = planned.as_ref().err().cloned();
        let warnings = planned
            .as_ref()
            .ok()
            .into_iter()
            .flatten()
            .flat_map(|row| {
                row.warnings.iter().map(move |warning| {
                    format!("entity:{} row {:?}: {warning}", row.source, row.index)
                })
            })
            .collect::<Vec<_>>();
        let current = self
            .check_connection_snapshot(draft.generation, draft.revision)
            .is_ok()
            && batch.definitions == self.definitions_serial;
        json!({"handle":Self::edit_handle("connection_edit",current.then_some(draft.token)),"token":draft.token,"batch":true,"action":batch.action,
            "generation":draft.generation,"revision":draft.revision,"definitions":batch.definitions,"sources":batch.sources.iter().copied().map(entity_id).collect::<Vec<_>>(),
            "rows":batch.rows.iter().map(|row|json!({"source":entity_id(row.source),"index":row.index,"identity":row.identity,"output":row.output,"value":row.value})).collect::<Vec<_>>(),"fields":(batch.action == ConnectionBatchAction::Add).then_some(&draft.fields),"patch":batch.patch,
            "common":{"output":common(0),"target":common(1),"input":common(2),"parameter":common(3),"delay":common(4),"times":common(5)},
            "affected_rows":if batch.action==ConnectionBatchAction::Add{batch.sources.len()}else{batch.rows.len()},"error":error,"warnings":warnings,"can_apply":planned.is_ok(),
            "planned_rows":planned.ok().map(|rows|rows.into_iter().map(|row|json!({"source":entity_id(row.source),"index":row.index,"fields":row.spec.map(|spec|json!({"output":spec.output,"target":spec.target,"input":spec.input,"parameter":spec.parameter,"delay":spec.delay,"times":spec.times})),"warnings":row.warnings})).collect::<Vec<_>>())})
    }

    pub fn commit_connection_batch(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.check_connection_draft(token)?;
        let planned = self.plan_connection_batch()?;
        let draft = self
            .connection_draft
            .clone()
            .ok_or("Begin a connection batch first")?;
        let batch = draft
            .batch
            .as_ref()
            .ok_or("Begin a connection batch first")?;
        let action = batch.action;
        let count = planned.len();
        let previous_revision = self.document.revision();
        self.edit(actor, "apply captured entity connections", |tx| {
            for row in &batch.rows {
                let raw = tx
                    .document()
                    .entity(row.source)
                    .and_then(|entity| entity.connections.as_ref())
                    .and_then(|rows| rows.get(row.index));
                if raw != Some(&row.raw()) {
                    return Err(EditError::Invalid(
                        "A captured connection row changed before Apply".into(),
                    ));
                }
            }
            if action == ConnectionBatchAction::Remove {
                for row in planned.iter().rev() {
                    connections::delete(
                        tx,
                        row.source,
                        row.index.ok_or_else(|| {
                            EditError::Invalid(
                                "Captured connection row is missing its index".into(),
                            )
                        })?,
                    )?;
                }
            } else {
                for row in &planned {
                    connections::set(
                        tx,
                        row.source,
                        row.index,
                        row.spec.as_ref().ok_or_else(|| {
                            EditError::Invalid(
                                "Captured connection row is missing its fields".into(),
                            )
                        })?,
                    )?;
                }
            }
            Ok(())
        })?;
        self.cancel_connection_edit();
        Ok(
            json!({"handle":null,"applied":true,"action":action,"affected_rows":count,"sources":batch.sources.iter().copied().map(entity_id).collect::<Vec<_>>(),"changed":self.document.revision()!=previous_revision}),
        )
    }
}
