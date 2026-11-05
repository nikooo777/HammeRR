use source_formats::vmf::{Entity, Vmf};

use crate::fog::{MAX_PARENT_ENTITIES, boolean_property, number_property, raw_property};

pub const MAX_TRIGGER_SOURCES: usize = 128;
pub const MAX_TRIGGER_SOURCE_ACTIONS: usize = 512;
pub const MAX_TRIGGER_SOURCE_BYTES: usize = 1024 * 1024;

const MAX_SOURCE_PROPERTIES: usize = 256;
const MAX_SOURCE_SOLIDS: usize = 4096;
const MAX_PROPERTY_VISITS: usize = 1_048_576;
const MAX_ACTION_FIELD_BYTES: usize = 255;
const CLIENTS_ONLY_FLAGS: u32 = 1;
const UNLIMITED_FIRINGS: i32 = -1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerOutput {
    StartTouch,
    StartTouchAll,
    EndTouch,
    EndTouchAll,
}

impl TriggerOutput {
    pub fn name(self) -> &'static str {
        match self {
            Self::StartTouch => "OnStartTouch",
            Self::StartTouchAll => "OnStartTouchAll",
            Self::EndTouch => "OnEndTouch",
            Self::EndTouchAll => "OnEndTouchAll",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "OnStartTouch" => Ok(Self::StartTouch),
            "OnStartTouchAll" => Ok(Self::StartTouchAll),
            "OnEndTouch" => Ok(Self::EndTouch),
            "OnEndTouchAll" => Ok(Self::EndTouchAll),
            _ => Err(format!(
                "Trigger output {value:?} is outside the four supported touch outputs"
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceAction {
    pub connection_index: usize,
    pub output: TriggerOutput,
    pub controller_name: String,
    pub delay: f32,
    pub times_to_fire: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TriggerSource {
    pub id: u32,
    pub actions: Vec<SourceAction>,
}

fn checked_add(total: usize, count: usize, limit: usize, label: &str) -> Result<usize, String> {
    total
        .checked_add(count)
        .filter(|value| *value <= limit)
        .ok_or_else(|| format!("Fog trigger source exceeds its {label} limit of {limit}"))
}

fn source_bytes(entity: &Entity) -> Result<usize, String> {
    if entity.properties.0.len() > MAX_SOURCE_PROPERTIES {
        return Err(format!(
            "Fog trigger source exceeds {MAX_SOURCE_PROPERTIES} properties"
        ));
    }
    let mut bytes = 0;
    for (key, value) in entity.properties.iter() {
        bytes = checked_add(
            bytes,
            key.len(),
            MAX_TRIGGER_SOURCE_BYTES,
            "property/action bytes",
        )?;
        bytes = checked_add(
            bytes,
            value.len(),
            MAX_TRIGGER_SOURCE_BYTES,
            "property/action bytes",
        )?;
    }
    let connections = entity.connections.as_deref().unwrap_or_default();
    if connections.len() > MAX_TRIGGER_SOURCE_ACTIONS {
        return Err(format!(
            "Fog trigger source exceeds {MAX_TRIGGER_SOURCE_ACTIONS} action rows"
        ));
    }
    for connection in connections {
        bytes = checked_add(
            bytes,
            connection.output.len(),
            MAX_TRIGGER_SOURCE_BYTES,
            "property/action bytes",
        )?;
        bytes = checked_add(
            bytes,
            connection.value.len(),
            MAX_TRIGGER_SOURCE_BYTES,
            "property/action bytes",
        )?;
    }
    Ok(bytes)
}

fn admit(entity: &Entity) -> Result<(), String> {
    if raw_property(entity, "classname")? != Some("trigger_multiple") {
        return Err("Fog trigger source must have the exact classname trigger_multiple".into());
    }
    if entity.hidden {
        return Err("Authored-hidden fog trigger is unavailable; show it before previewing".into());
    }
    if entity.solids.is_empty() || entity.solids.len() > MAX_SOURCE_SOLIDS {
        return Err(format!(
            "Fog trigger needs between 1 and {MAX_SOURCE_SOLIDS} authored brushes"
        ));
    }
    if entity.solids.iter().any(|solid| solid.hidden) {
        return Err("Fog trigger has authored-hidden brushes; show them before previewing".into());
    }
    if !entity.extra.is_empty() {
        return Err("Fog trigger source has unsupported extra entity blocks".into());
    }
    if entity.properties.iter().any(|(key, _)| {
        key.get(..2)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("on"))
    }) {
        return Err("Fog trigger output properties must use the connections block".into());
    }
    for key in ["parentname", "filtername", "globalname", "model"] {
        if raw_property(entity, key)?.is_some_and(|value| !value.is_empty()) {
            return Err(format!("Fog trigger {key} bindings are unavailable"));
        }
    }
    if raw_property(entity, "parentattachment")?
        .is_some_and(|value| !value.is_empty() && value != "0")
    {
        return Err("Fog trigger parent attachments are unavailable".into());
    }
    let flags = raw_property(entity, "spawnflags")?
        .unwrap_or("0")
        .trim()
        .parse::<u32>()
        .map_err(|_| "Fog trigger spawnflags must be an unsigned integer")?;
    if flags != CLIENTS_ONLY_FLAGS {
        return Err("Fog trigger requires exactly spawnflags 1 (clients only)".into());
    }
    if boolean_property(entity, "StartDisabled")? {
        return Err("Fog trigger must be initially enabled (StartDisabled 0)".into());
    }
    if number_property(entity, "wait", Some(0.0))? < 0.0 {
        return Err("Negative trigger wait can remove the source and is unavailable".into());
    }
    if number_property(entity, "health", Some(0.0))? != 0.0 {
        return Err("Fog trigger health must be zero".into());
    }
    Ok(())
}

fn action(
    connection: &source_formats::vmf::Connection,
    connection_index: usize,
) -> Result<SourceAction, String> {
    if connection.fields().take(6).count() != 5 {
        return Err("Expected target, input, parameter, delay and firing count".into());
    }
    if connection
        .fields()
        .any(|field| field.len() > MAX_ACTION_FIELD_BYTES)
    {
        return Err(format!(
            "Fog trigger action fields must fit {MAX_ACTION_FIELD_BYTES} native bytes"
        ));
    }
    let mut spec = crate::connections::parse(connection)?;
    if spec.times == 0 {
        spec.times = UNLIMITED_FIRINGS;
    }
    spec.validate(crate::connections::separator(connection))?;
    let output = TriggerOutput::parse(&spec.output)?;
    if spec.target != "!activator" || spec.input != "SetFogController" {
        return Err("Fog trigger actions require exact !activator and SetFogController".into());
    }
    let name = &spec.parameter;
    if name.is_empty()
        || name != name.trim()
        || name.starts_with('!')
        || name.contains(['*', '?', ',', '"', '[', ']'])
        || name.chars().any(char::is_control)
    {
        return Err("Fog trigger parameter needs an exact nonempty literal controller targetname; special, wildcard and attachment forms are unavailable".into());
    }
    Ok(SourceAction {
        connection_index,
        output,
        controller_name: spec.parameter,
        delay: spec.delay,
        times_to_fire: spec.times,
    })
}

pub fn parse_trigger_source(entity: &Entity) -> Result<TriggerSource, String> {
    source_bytes(entity)?;
    admit(entity).map_err(|error| format!("entity:{}: {error}", entity.id))?;
    let connections = entity.connections.as_deref().unwrap_or_default();
    let mut actions = Vec::new();
    actions
        .try_reserve(connections.len())
        .map_err(|_| "Fog trigger action allocation failed")?;
    for (connection_index, connection) in connections.iter().enumerate() {
        actions.push(action(connection, connection_index).map_err(|error| {
            format!(
                "entity:{} connection {connection_index}: {error}",
                entity.id
            )
        })?);
    }
    Ok(TriggerSource {
        id: entity.id,
        actions,
    })
}

fn is_trigger_source(entity: &Entity) -> bool {
    entity.properties.iter().any(|(key, value)| {
        key.eq_ignore_ascii_case("classname") && value.eq_ignore_ascii_case("trigger_multiple")
    })
}

pub fn admit_trigger_source_catalog(vmf: &Vmf) -> Result<(), String> {
    if vmf.entities.len() > MAX_PARENT_ENTITIES {
        return Err(format!(
            "Fog trigger source scan exceeds {MAX_PARENT_ENTITIES} entities"
        ));
    }
    let mut sources = 0;
    let mut properties = 0;
    let mut bytes = 0;
    let mut actions = 0;
    for entity in &vmf.entities {
        properties = checked_add(
            properties,
            entity.properties.0.len(),
            MAX_PROPERTY_VISITS,
            "property visits",
        )?;
        if !is_trigger_source(entity) {
            continue;
        }
        sources = checked_add(sources, 1, MAX_TRIGGER_SOURCES, "trigger count")?;
        bytes = checked_add(
            bytes,
            source_bytes(entity)?,
            MAX_TRIGGER_SOURCE_BYTES,
            "property/action bytes",
        )?;
        actions = checked_add(
            actions,
            entity.connections.as_ref().map_or(0, Vec::len),
            MAX_TRIGGER_SOURCE_ACTIONS,
            "action rows",
        )?;
    }
    Ok(())
}

pub fn parse_trigger_sources(vmf: &Vmf) -> Result<Vec<TriggerSource>, String> {
    admit_trigger_source_catalog(vmf)?;
    let mut sources = Vec::new();
    for entity in &vmf.entities {
        if !is_trigger_source(entity) {
            continue;
        }
        let source = parse_trigger_source(entity)?;
        sources
            .try_reserve(1)
            .map_err(|_| "Fog trigger source allocation failed")?;
        sources.push(source);
    }
    Ok(sources)
}
