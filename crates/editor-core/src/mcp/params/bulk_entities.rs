use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BulkEntityTargetsArgs {
    /// Captured whole entity IDs; omitted uses the exact current selection.
    #[schemars(length(min = 1, max = 64))]
    pub ids: Option<Vec<String>>,
    /// Supply generation and revision together to reject a changed source map.
    pub generation: Option<u64>,
    pub revision: Option<u64>,
    pub definitions: Option<u64>,
}

impl BulkEntityTargetsArgs {
    pub(in crate::mcp) fn targets(self, session: &Session) -> Result<Vec<ObjectId>, String> {
        match (self.generation, self.revision) {
            (Some(generation), Some(revision))
                if generation == session.document_generation()
                    && revision == session.document().revision() => {}
            (None, None) => {}
            (Some(_), Some(_)) => return Err("Map changed; inspect the targets again".into()),
            _ => return Err("Supply generation and revision together".into()),
        }
        if self
            .definitions
            .is_some_and(|serial| serial != session.definitions_serial())
        {
            return Err("Entity definitions changed; inspect the targets again".into());
        }
        if self
            .ids
            .as_ref()
            .is_some_and(|ids| ids.is_empty() || ids.len() > 64)
        {
            return Err("Choose between 1 and 64 whole entities".into());
        }
        let ids = ids_or_selection(session, self.ids.as_deref())?;
        session.whole_entity_targets(&ids)?;
        Ok(ids)
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BulkEntityClassArgs {
    pub classname: String,
    #[schemars(length(min = 1, max = 64))]
    pub ids: Option<Vec<String>>,
    pub generation: Option<u64>,
    pub revision: Option<u64>,
    pub definitions: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BulkEntityTokenArgs {
    pub token: u64,
}
