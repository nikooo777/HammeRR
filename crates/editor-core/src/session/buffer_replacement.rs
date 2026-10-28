use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferReplacementReview {
    scope: String,
    generation: u64,
    revision: u64,
    save_serial: u64,
    partition: Option<u64>,
}

impl BufferReplacementReview {
    pub fn token(&self) -> String {
        format!(
            "buffer-v1:{}:{}:{}:{}:{}",
            self.scope,
            self.generation,
            self.revision,
            self.save_serial,
            self.partition
                .map_or_else(|| "none".into(), |token| token.to_string())
        )
    }
}

impl Session {
    pub fn buffer_replacement_review(&self) -> BufferReplacementReview {
        BufferReplacementReview {
            scope: self.buffer_review_scope.clone(),
            generation: self.document_generation,
            revision: self.document.revision(),
            save_serial: self.history_save_serial,
            partition: self.partition_preview_token(),
        }
    }

    pub fn buffer_replacement_review_from_token(
        &self,
        token: &str,
    ) -> Result<BufferReplacementReview, String> {
        let review = self.buffer_replacement_review();
        if token != review.token() {
            return Err(Self::stale_replacement_review());
        }
        Ok(review)
    }

    pub fn ensure_buffer_replacement_review(
        &self,
        review: &BufferReplacementReview,
    ) -> Result<(), String> {
        if review != &self.buffer_replacement_review() {
            return Err(Self::stale_replacement_review());
        }
        Ok(())
    }

    fn stale_replacement_review() -> String {
        "Map changed since replacement was reviewed; review the current map and try again".into()
    }

    pub fn open_reviewed(
        &mut self,
        actor: Actor,
        path: &Path,
        discard_changes: bool,
        review: &BufferReplacementReview,
    ) -> Result<String, String> {
        self.ensure_buffer_replacement_review(review)?;
        self.open(actor, path, discard_changes)
    }

    pub fn new_map_reviewed(
        &mut self,
        actor: Actor,
        discard_changes: bool,
        review: &BufferReplacementReview,
    ) -> Result<(), String> {
        self.ensure_buffer_replacement_review(review)?;
        self.new_map(actor, discard_changes)
    }

    pub fn open_recent_map_reviewed(
        &mut self,
        actor: Actor,
        path: &Path,
        discard_changes: bool,
        review: &BufferReplacementReview,
    ) -> Result<String, String> {
        self.ensure_buffer_replacement_review(review)?;
        self.open_recent_map(actor, path, discard_changes)
    }

    pub fn restore_recovery_snapshot_reviewed(
        &mut self,
        actor: Actor,
        id: &str,
        discard_changes: bool,
        expected: Option<(u64, u64)>,
        review: &BufferReplacementReview,
    ) -> Result<(), String> {
        self.ensure_buffer_replacement_review(review)?;
        self.restore_recovery_snapshot(actor, id, discard_changes, expected)
    }

    pub fn ensure_document_generation(&self, expected: Option<u64>) -> Result<(), String> {
        if expected.is_some_and(|expected| expected != self.document_generation) {
            return Err("The map was replaced while choosing a file; try again".into());
        }
        Ok(())
    }

    pub fn save_for_generation(
        &mut self,
        actor: Actor,
        path: Option<&Path>,
        expected_generation: Option<u64>,
    ) -> Result<PathBuf, String> {
        self.ensure_document_generation(expected_generation)?;
        self.save(actor, path)
    }

    pub fn begin_compile_for_generation(
        &mut self,
        actor: Actor,
        path: Option<&Path>,
        expected_generation: Option<u64>,
    ) -> Result<compilation::CompileRequest, String> {
        self.ensure_document_generation(expected_generation)?;
        self.begin_compile(actor, path)
    }
}
