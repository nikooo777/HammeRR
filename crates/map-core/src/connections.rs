use source_formats::vmf::Connection;

use crate::{Document, EditError, Transaction};

#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub output: String,
    pub target: String,
    pub input: String,
    pub parameter: String,
    pub delay: f32,
    pub times: i32,
}

pub fn parse(connection: &Connection) -> Result<Spec, String> {
    let fields: Vec<_> = connection.fields().collect();
    if fields.len() != 5 {
        return Err("Expected target, input, parameter, delay and firing count".into());
    }
    Ok(Spec {
        output: connection.output.clone(),
        target: fields[0].into(),
        input: fields[1].into(),
        parameter: fields[2].into(),
        delay: fields[3].trim().parse().map_err(|_| "Invalid delay")?,
        times: fields[4]
            .trim()
            .parse()
            .map_err(|_| "Invalid firing count")?,
    })
}

impl Spec {
    pub fn validate(&self, separator: char) -> Result<(), String> {
        for (label, value, required) in [
            ("Output", &self.output, true),
            ("Target", &self.target, true),
            ("Input", &self.input, true),
            ("Parameter", &self.parameter, false),
        ] {
            if required && value.trim().is_empty() {
                return Err(format!("{label} is required"));
            }
            if value.contains(separator)
                || value.contains('"')
                || value.chars().any(char::is_control)
            {
                return Err(format!(
                    "{label} cannot contain {separator:?}, quotes or control characters"
                ));
            }
        }
        if !self.delay.is_finite() || self.delay < 0.0 {
            return Err("Delay must be a finite, nonnegative number of seconds".into());
        }
        if self.times != -1 && self.times < 1 {
            return Err("Firing count must be -1 (unlimited) or a positive integer".into());
        }
        Ok(())
    }
}

pub fn separator(connection: &Connection) -> char {
    if connection.value.contains('\u{1b}') {
        '\u{1b}'
    } else {
        ','
    }
}

pub fn set(
    tx: &mut Transaction,
    entity: u32,
    index: Option<usize>,
    spec: &Spec,
) -> Result<usize, EditError> {
    let entity = tx.entity_mut(entity)?;
    let previous = match index {
        Some(index) => Some(
            entity
                .connections
                .as_deref()
                .unwrap_or_default()
                .get(index)
                .ok_or_else(|| EditError::Invalid("Connection no longer exists".into()))?,
        ),
        None => None,
    };
    let separator = previous.map_or(',', separator);
    spec.validate(separator).map_err(EditError::Invalid)?;
    if previous.is_some_and(|connection| parse(connection).as_ref() == Ok(spec)) {
        return Ok(index.unwrap());
    }
    let connection = Connection {
        output: spec.output.clone(),
        value: format!(
            "{}{separator}{}{separator}{}{separator}{}{separator}{}",
            spec.target, spec.input, spec.parameter, spec.delay, spec.times
        ),
    };
    let connections = entity.connections.get_or_insert_with(Vec::new);
    if let Some(index) = index {
        connections[index] = connection;
        Ok(index)
    } else {
        let index = connections.len();
        connections.push(connection);
        Ok(index)
    }
}

pub fn delete(tx: &mut Transaction, entity: u32, index: usize) -> Result<(), EditError> {
    let connections = tx
        .entity_mut(entity)?
        .connections
        .as_mut()
        .ok_or_else(|| EditError::Invalid("Connection no longer exists".into()))?;
    if index >= connections.len() {
        return Err(EditError::Invalid("Connection no longer exists".into()));
    }
    connections.remove(index);
    Ok(())
}

pub fn targets(document: &Document, source: u32, target: &str) -> Vec<u32> {
    if target == "!self" {
        return document
            .entity(source)
            .filter(|entity| !entity.hidden)
            .map(|_| vec![source])
            .unwrap_or_default();
    }
    if target.is_empty() || target.starts_with('!') {
        return Vec::new();
    }
    document
        .vmf()
        .entities
        .iter()
        .filter(|entity| !entity.hidden)
        .filter(|entity| {
            let name = entity.properties.get("targetname").unwrap_or_default();
            name_matches(name, target)
        })
        .map(|entity| entity.id)
        .collect()
}

pub fn name_matches(name: &str, target: &str) -> bool {
    if name.is_empty() || target.is_empty() || target.starts_with('!') {
        return false;
    }
    match target.split_once('*') {
        Some((prefix, _)) => name
            .get(..prefix.len())
            .is_some_and(|name| name.eq_ignore_ascii_case(prefix)),
        None => name.eq_ignore_ascii_case(target),
    }
}

pub fn incoming(document: &Document, target: u32) -> Vec<(u32, usize)> {
    let Some(target) = document.entity(target).filter(|entity| !entity.hidden) else {
        return Vec::new();
    };
    let target_name = target.properties.get("targetname").unwrap_or_default();
    document
        .vmf()
        .entities
        .iter()
        .filter(|entity| !entity.hidden)
        .flat_map(|entity| {
            entity.connections.iter().flatten().enumerate().filter_map(
                move |(index, connection)| {
                    let name = connection.fields().next().unwrap_or_default();
                    (if name == "!self" {
                        entity.id == target.id
                    } else {
                        name_matches(target_name, name)
                    })
                    .then_some((entity.id, index))
                },
            )
        })
        .collect()
}
