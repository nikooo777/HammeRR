use map_core::references::{Change, Location};
use serde::Serialize;

use super::Session;

#[derive(Serialize)]
pub struct RenameReferenceView {
    pub label: String,
    pub change: String,
    pub explanation: String,
    pub details: String,
}

fn words(value: &str) -> String {
    let mut result = String::new();
    let mut previous_lower = false;
    for ch in value.chars() {
        if ch == '_' {
            result.push(' ');
        } else {
            if ch.is_uppercase() && previous_lower {
                result.push(' ');
            }
            result.extend(ch.to_lowercase());
        }
        previous_lower = ch.is_lowercase();
    }
    if let Some(first) = result.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    result
}

fn event(name: &str) -> String {
    match name {
        "OnTrigger" => "When triggered".into(),
        "OnMapSpawn" => "When the map starts".into(),
        "OnPressed" => "When pressed".into(),
        "OnStartTouch" => "When something enters".into(),
        "OnEndTouch" => "When something leaves".into(),
        _ => format!("{} event", words(name.strip_prefix("On").unwrap_or(name))),
    }
}

impl Session {
    pub fn rename_entity_label(&self, id: u32) -> String {
        let Some(entity) = self.document().entity(id) else {
            return "Missing entity".into();
        };
        let name = entity
            .properties
            .get("targetname")
            .filter(|name| !name.is_empty());
        let class = crate::inspector::title(entity.classname().unwrap_or("Custom entity"));
        let label = name.map_or_else(|| format!("Unnamed {class}"), words);
        format!("{label}{}", if entity.hidden { " (hidden)" } else { "" })
    }

    pub fn rename_reference_view(&self, change: &Change, new_name: &str) -> RenameReferenceView {
        let reference = &change.reference;
        let source = self.document().entity(reference.source).unwrap();
        let class = source.classname().unwrap_or("Custom entity");
        let (role, mut explanation, detail) = match &reference.location {
            Location::Property { key, .. } => {
                let label = self
                    .definitions()
                    .class(class)
                    .and_then(|class| class.property(key))
                    .filter(|property| !property.label.is_empty())
                    .map(|property| property.label.clone())
                    .unwrap_or_else(|| words(key));
                let explanation = if key.eq_ignore_ascii_case("parentname") {
                    let attachment = reference.value.split_once(',').map(|(_, value)| value);
                    attachment.map_or_else(|| "Keep this entity linked to the renamed parent.".into(),
                        |attachment| format!("Keep this entity linked to the renamed parent. Attachment “{attachment}” stays the same."))
                } else {
                    format!("Keep “{label}” pointing to the renamed entity.")
                };
                (label, explanation, key.clone())
            }
            Location::Connection {
                index,
                output,
                field,
            } => {
                let connection = &source.connections.as_ref().unwrap()[*index];
                let fields: Vec<_> = connection.fields().collect();
                let input = fields.get(1).copied().unwrap_or_default();
                let action = words(input);
                let explanation = if *field == 0 {
                    format!("Keep sending the “{action}” action to the renamed entity.")
                } else if input.eq_ignore_ascii_case("SetParent") {
                    "Use the renamed entity as the parent when this action runs.".into()
                } else {
                    format!("Use the renamed entity as the reference parameter for “{action}”.")
                };
                (
                    format!(
                        "{} · {}",
                        event(output),
                        if *field == 0 {
                            "recipient"
                        } else {
                            "action parameter"
                        }
                    ),
                    explanation,
                    format!(
                        "{output} → {input} · output {} · {}",
                        index + 1,
                        if *field == 0 { "target" } else { "parameter" }
                    ),
                )
            }
        };
        if change.review {
            explanation = if reference.target.contains('*') && reference.certain {
                if map_core::connections::name_matches(new_name, &reference.target) {
                    "This pattern will now include the renamed entity. Check whether that is intended.".into()
                } else {
                    "This pattern will no longer include the renamed entity. Check whether that is intended.".into()
                }
            } else if !reference.declared {
                "This text matches the old name. Check whether it should use the new name; it will stay unchanged here.".into()
            } else if !reference.certain {
                let property = match &reference.location {
                    Location::Property { key, .. } => self
                        .definitions()
                        .class(class)
                        .and_then(|class| class.property(key)),
                    _ => None,
                };
                if property.is_some_and(|property| property.read_only) {
                    "The entity definition marks this field read-only. It will keep the old reference; check its effect.".into()
                } else if property.is_some_and(|property| property.kind == "target_name_or_class") {
                    "This may refer to an entity class instead of a specific entity. Check its meaning before keeping it unchanged.".into()
                } else {
                    "This imported connection cannot be safely rewritten. It will stay unchanged and needs manual inspection.".into()
                }
            } else {
                "This reference already uses the new name. It will start addressing the renamed entity; check whether that is intended.".into()
            };
        } else if !change.changed {
            explanation = if !reference.declared {
                "This text matches the entity’s name. It may be an intended link.".into()
            } else if !reference.certain {
                "This field matches the name, but its reference needs verification.".into()
            } else if reference.target.starts_with('!') {
                "This recipient is chosen when the game runs; renaming does not change it.".into()
            } else if reference.target.contains('*') {
                "This pattern still includes the renamed entity. No edit is needed.".into()
            } else {
                "Currently points to this entity.".into()
            };
        }
        RenameReferenceView {
            label: format!("{} · {role}", self.rename_entity_label(reference.source)),
            change: if change.changed {
                format!("{} → {new_name}", reference.target)
            } else if change.review {
                format!("Keep “{}” unchanged", reference.target)
            } else {
                reference.target.clone()
            },
            explanation,
            details: format!(
                "{} · {class} · entity:{} · {detail}",
                source.properties.get("targetname").unwrap_or("Unnamed"),
                reference.source
            ),
        }
    }
}
