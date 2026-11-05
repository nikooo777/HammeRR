use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct BeginRenameBatchArgs {
    /// Complete entity IDs; omit to capture the current complete entity selection.
    pub ids: Option<Vec<String>>,
    /// Exact map identity from get_map_info.
    pub generation: u64,
    pub revision: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct RenameBatchName {
    pub entity: String,
    pub new_name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct RenameBatchNamesArgs {
    pub token: u64,
    /// Exactly one explicit new name for every captured entity, including unchanged members.
    #[schemars(length(min = 1, max = 256))]
    pub names: Vec<RenameBatchName>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct RenameBatchTokenArgs {
    pub token: u64,
}

pub(in crate::mcp) fn rename_batch_json(session: &Session, token: u64) -> Result<Value, String> {
    let plan = session.entity_rename_batch_preview(token)?;
    let changed = plan
        .names
        .iter()
        .any(|name| name.old_name.as_deref() != Some(name.new_name.as_str()));
    Ok(json!({
        "kind":"entity_rename_batch", "token":token,
        "generation":session.document_generation(), "revision":plan.revision,
        "definitions_serial":session.definitions_serial(),
        "names":plan.names.iter().map(|name| json!({
            "entity":id_string(ObjectId::Entity(name.entity)),
            "old_name":name.old_name, "new_name":name.new_name,
        })).collect::<Vec<_>>(),
        "references":plan.references.iter().map(|change| json!({
            "reference":reference_json(&change.reference),
            "after":change.after, "changed":change.changed, "reason":change.reason,
        })).collect::<Vec<_>>(),
        "can_apply":plan.error.is_none() && changed, "error":plan.error,
        "limit":map_core::references::MAX_BULK_RENAME_ENTITIES,
    }))
}
