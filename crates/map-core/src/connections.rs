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

#[cfg(test)]
mod tests {
    use super::*;
    use source_formats::{keyvalues::WriteOptions, vmf::Vmf};

    fn fixture() -> Document {
        Document::new(
            Vmf::parse(
                r#"world { "id" "1" "classname" "worldspawn" }
entity { "id" "2" "classname" "logic_relay" "targetname" "relay"
 connections { "OnTrigger" "Door,Open,,0.00,-1" "Custom" "malformed" } custom { "keep" "this" } }
entity { "id" "3" "classname" "func_door" "targetname" "Door" }
entity { "id" "4" "classname" "func_door" "targetname" "door_extra" }
"#,
            )
            .unwrap(),
        )
    }

    #[test]
    fn edits_preserve_duplicates_imported_data_and_undo() {
        let mut doc = fixture();
        let original = doc.vmf().write(&WriteOptions::default());
        let mut spec = parse(&doc.entity(2).unwrap().connections.as_ref().unwrap()[0]).unwrap();
        doc.edit("unchanged", |tx| set(tx, 2, Some(0), &spec))
            .unwrap();
        assert_eq!(doc.revision(), 0);
        spec.delay = 0.5;
        doc.edit("edit", |tx| set(tx, 2, Some(0), &spec)).unwrap();
        doc.edit("add", |tx| set(tx, 2, None, &spec)).unwrap();
        let list = doc.entity(2).unwrap().connections.as_ref().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[1].value, "malformed");
        assert_eq!(list[0].output, list[2].output);
        doc.edit("delete", |tx| delete(tx, 2, 1)).unwrap();
        let saved = doc.vmf().write(&WriteOptions::default());
        assert_eq!(
            Vmf::parse(&saved).unwrap().entities[0]
                .connections
                .as_ref()
                .unwrap()
                .len(),
            2
        );
        for _ in 0..3 {
            doc.undo().unwrap();
        }
        assert_eq!(doc.vmf().write(&WriteOptions::default()), original);
        for _ in 0..3 {
            doc.redo().unwrap();
        }
        assert_eq!(doc.vmf().write(&WriteOptions::default()), saved);
    }

    #[test]
    fn validation_rolls_back_and_retains_escaped_imports() {
        let mut doc = fixture();
        let mut spec = parse(&doc.entity(2).unwrap().connections.as_ref().unwrap()[0]).unwrap();
        for (delay, times) in [(f32::NAN, -1), (-1.0, -1), (0.0, 0)] {
            spec.delay = delay;
            spec.times = times;
            assert!(doc.edit("bad", |tx| set(tx, 2, None, &spec)).is_err());
            assert_eq!(doc.revision(), 0);
        }
        let raw = Connection {
            output: "OnTrigger".into(),
            value: "relay\u{1b}Custom\u{1b}one,two\u{1b}0\u{1b}1".into(),
        };
        doc.edit("import", |tx| {
            tx.entity_mut(2)?
                .connections
                .as_mut()
                .unwrap()
                .push(raw.clone());
            Ok(())
        })
        .unwrap();
        spec = parse(&raw).unwrap();
        spec.delay = 2.0;
        doc.edit("edit escaped", |tx| set(tx, 2, Some(2), &spec))
            .unwrap();
        assert_eq!(
            separator(&doc.entity(2).unwrap().connections.as_ref().unwrap()[2]),
            '\u{1b}'
        );
        assert!(doc.edit("new comma", |tx| set(tx, 2, None, &spec)).is_err());
    }

    #[test]
    fn references_resolve_names_prefixes_and_self() {
        let mut doc = fixture();
        assert_eq!(targets(&doc, 2, "door"), vec![3]);
        assert_eq!(targets(&doc, 2, "DOOR*"), vec![3, 4]);
        assert_eq!(targets(&doc, 2, "!self"), vec![2]);
        assert!(targets(&doc, 2, "!activator").is_empty());
        assert_eq!(incoming(&doc, 3), vec![(2, 0)]);
        doc.edit("delete target", |tx| {
            crate::edit::delete(tx, &[crate::ObjectId::Entity(3)])
        })
        .unwrap();
        assert!(targets(&doc, 2, "Door").is_empty());
        doc.undo().unwrap();
        assert_eq!(incoming(&doc, 3), vec![(2, 0)]);
    }
}
