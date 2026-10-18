use super::*;

impl Session {
    pub(crate) fn verify_named_save_source(&self) -> Result<(), String> {
        self.ensure_not_closing()?;
        let path = self
            .path
            .as_ref()
            .ok_or("The map needs Save as before its source can be verified")?;
        let source = self
            .file_claims
            .source
            .as_ref()
            .ok_or("The named map has no retained source identity; reopen or use Save as")?;
        source
            .destination()
            .verify()
            .map_err(|error| format!("{}: {error}", path.display()))
    }
}
