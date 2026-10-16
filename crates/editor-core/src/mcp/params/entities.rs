use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PropertyTargetArgs {
    /// World settings when true. Otherwise use entity IDs or the selection.
    #[serde(default)]
    pub world: bool,
    pub ids: Option<Vec<String>>,
    /// Optional guards from get_property_inspector or get_object_report.
    pub generation: Option<u64>,
    pub revision: Option<u64>,
}

impl PropertyTargetArgs {
    pub(in crate::mcp) fn target(self, session: &Session) -> Result<PropertyTarget, String> {
        if self
            .generation
            .is_some_and(|value| value != session.document_generation())
            || self
                .revision
                .is_some_and(|value| value != session.document().revision())
        {
            return Err("Map changed; inspect properties again".into());
        }
        if self.world {
            if self.ids.is_some() {
                return Err("Use world or entity IDs, not both".into());
            }
            return Ok(PropertyTarget::World);
        }
        let ids = ids_or_selection(session, self.ids.as_deref())?
            .into_iter()
            .map(|id| match id {
                ObjectId::Entity(id) => Ok(id),
                _ => {
                    Err("Property batches require entities; use world=true for map settings".into())
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(PropertyTarget::Entities(ids))
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PropertyEditArgs {
    pub token: u64,
    /// Replaces the pending change list. Nothing is written until commit.
    pub changes: Vec<PropertyChange>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PropertyTokenArgs {
    pub token: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateEntityArgs {
    pub classname: String,
    /// Position [x, y, z]. Overrides properties.origin; otherwise uses the current workplane or a point ahead of the camera, with the shared snapping settings.
    pub origin: Option<[f64; 3]>,
    /// Extra key/values, e.g. {"targetname": "door1", "angles": "0 90 0"}.
    pub properties: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EntityClassArgs {
    pub classname: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchEntityClassesArgs {
    /// Case-insensitive substring of the classname or description.
    pub query: Option<String>,
    /// "point" or "solid"; omitted searches both.
    pub kind: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DefinitionsArgs {
    /// Root FGD filename; includes resolve relative to each containing FGD.
    pub path: String,
}

pub(in crate::mcp) fn class_json(class: &Class) -> Value {
    let properties: Vec<Value> = class.properties.iter().map(|property| json!({
        "name": property.name, "type": property.kind, "label": property.label,
        "description": property.description, "default": property.default_value(),
        "read_only": property.read_only,
        "choices": property.choices.iter().map(|choice| json!({"value": choice.value, "label": choice.label, "default_selected": choice.selected})).collect::<Vec<_>>()
    })).collect();
    let io = |members: &[source_formats::fgd::Io]| {
        members.iter().map(|member| json!({"name": member.name, "type": member.kind, "description": member.description})).collect::<Vec<_>>()
    };
    json!({"classname": class.name, "kind": class.kind.label(), "description": class.description,
        "bases": class.bases, "defaults": class.defaults(), "properties": properties,
        "inputs": io(&class.inputs), "outputs": io(&class.outputs),
        "helpers": class.helpers.iter().map(|helper| json!({"name": helper.name, "arguments": helper.arguments})).collect::<Vec<_>>()})
}

pub(in crate::mcp) fn definitions_json(session: &Session) -> Value {
    json!(session.entity_definitions_report())
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EntityArgs {
    /// Entity id; defaults to the single selected entity.
    pub id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ConnectionArgs {
    pub id: Option<String>,
    /// Omit to add a connection; otherwise use the zero-based outgoing index.
    pub index: Option<usize>,
    /// Optional stale-map guard; supply both values from get_entity_connections.
    pub generation: Option<u64>,
    pub revision: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ConnectionChoicesArgs {
    pub id: Option<String>,
    /// Target name, prefix wildcard or !self, to discover matching inputs.
    pub target: Option<String>,
    pub query: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RenameArgs {
    /// Entity ID; defaults to the single selected entity.
    pub id: Option<String>,
    /// Optional stale-map guard from get_entity_references; supply both values.
    pub generation: Option<u64>,
    pub revision: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RenameFieldsArgs {
    /// Identity guard from begin_entity_rename.
    pub token: Option<u64>,
    pub new_name: Option<String>,
    /// Explicitly rename all entities sharing the old name, including hidden ones.
    pub rename_group: Option<bool>,
    /// Acknowledge references left unchanged. Set after inspecting the preview;
    /// changing the name or group resets this acknowledgment.
    pub accept_review: Option<bool>,
    /// Show raw engine keys, class names and IDs in the shared human preview.
    pub show_details: Option<bool>,
}

pub(in crate::mcp) fn reference_json(reference: &map_core::references::Reference) -> Value {
    use map_core::references::Location;
    let location = match &reference.location {
        Location::Property { index, key } => json!({"kind":"property", "index":index, "key":key}),
        Location::Connection {
            index,
            output,
            field,
        } => {
            json!({"kind":"connection", "index":index, "output":output, "field":if *field == 0 {"target"} else {"parameter"}})
        }
    };
    json!({"source":id_string(ObjectId::Entity(reference.source)), "location":location,
        "value":reference.value, "target":reference.target, "declared":reference.declared, "certain":reference.certain,
        "reason":reference.reason, "hidden":reference.hidden})
}

pub(in crate::mcp) fn rename_plan_json(
    session: &Session,
    plan: &map_core::references::RenamePlan,
) -> Value {
    json!({"entity":id_string(ObjectId::Entity(plan.entity)), "old_name":plan.old_name, "new_name":plan.new_name,
        "entities":plan.entities.iter().map(|id| id_string(ObjectId::Entity(*id))).collect::<Vec<_>>(),
        "references":plan.references.iter().map(|change| json!({"reference":reference_json(&change.reference),
            "after":change.after, "changed":change.changed,
            "review":change.review, "reason":change.reason,
            "display":session.rename_reference_view(change, &plan.new_name)})).collect::<Vec<_>>(), "error":plan.error})
}

pub(in crate::mcp) fn rename_draft_json(session: &Session) -> Value {
    let Some(draft) = session.rename_draft() else {
        return Value::Null;
    };
    let (preview, error, review, can_apply) = match session.rename_preview() {
        Ok(plan) => (
            rename_plan_json(session, &plan),
            plan.error.clone(),
            plan.references.iter().any(|change| change.review),
            session.rename_can_apply(&plan),
        ),
        Err(error) => (Value::Null, Some(error), false, false),
    };
    json!({"entity":id_string(ObjectId::Entity(draft.entity)), "token":draft.token,
        "new_name":draft.new_name, "rename_group":draft.rename_group, "accept_review":draft.accept_review,
        "show_details":draft.show_details,
        "generation":draft.generation, "revision":draft.revision,
        "preview":preview, "error":error, "requires_review":review,
        "can_apply":can_apply})
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ConnectionFieldsArgs {
    /// Optional identity guard from begin_connection_edit; rejects a replaced draft.
    pub token: Option<u64>,
    pub output: Option<String>,
    pub target: Option<String>,
    pub input: Option<String>,
    /// Empty forwards the output value. Commas are unavailable in new CS:S connections.
    pub parameter: Option<String>,
    /// Seconds; strings allow unfinished numeric drafts shared with the UI.
    pub delay: Option<String>,
    /// -1 for unlimited, 1 for once, or a positive firing count.
    pub times: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ConnectionDraftArgs {
    /// Optional identity guard from the shared connection or rename draft.
    pub token: Option<u64>,
}

pub(in crate::mcp) fn connection_json(record: ConnectionRecord) -> Value {
    json!({"source": id_string(ObjectId::Entity(record.source)), "index": record.index,
        "output": record.raw.output, "raw": record.raw.value,
        "fields": record.spec.map(|spec| json!({"output":spec.output,"target":spec.target,"input":spec.input,"parameter":spec.parameter,"delay":spec.delay,"times":spec.times})),
        "targets": record.targets.into_iter().map(|id| id_string(ObjectId::Entity(id))).collect::<Vec<_>>(), "warnings":record.warnings})
}

pub(in crate::mcp) fn connection_draft_json(session: &Session) -> Value {
    let (error, warnings) = session.connection_draft_feedback();
    session.connection_draft().map_or(Value::Null, |draft| {
        json!({
            "source": id_string(ObjectId::Entity(draft.source)), "index":draft.index,"token":draft.token,
            "fields":draft.fields,"generation":draft.generation,"revision":draft.revision,
            "error":error,"warnings":warnings
        })
    })
}

pub(in crate::mcp) fn connection_guard(
    session: &Session,
    args: &ConnectionArgs,
) -> Result<(), String> {
    match (args.generation, args.revision) {
        (Some(generation), Some(revision)) => {
            session.check_connection_snapshot(generation, revision)
        }
        (None, None) => Ok(()),
        _ => Err("Supply both generation and revision for the stale-map guard".into()),
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EntityColorArgs {
    /// Entity id; defaults to the single selected entity.
    pub id: Option<String>,
    /// FGD color key, e.g. _light, _ambient or rendercolor.
    pub key: String,
    /// Red, green and blue channels in the range 0..255.
    pub color: [u8; 3],
    /// Nonnegative brightness; omitted preserves the existing component.
    pub intensity: Option<f64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CameraViewRenameArgs {
    pub id: u32,
    pub name: String,
    pub generation: Option<u64>,
    pub revision: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RenameBuildRecipeArgs {
    pub id: String,
    pub name: String,
    pub revision: Option<u64>,
}
