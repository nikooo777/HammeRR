use crate::EditError;
use source_formats::{keyvalues::Node, vmf::Properties};
pub(crate) const MAX_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_DEPTH: usize = 64;
pub(crate) const MAX_NODES: usize = 131_072;
fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}
#[derive(Default)]
pub(crate) struct InputBudget {
    nodes: usize,
    bytes: usize,
}

impl InputBudget {
    pub(crate) fn text(&mut self, key: &str, value: &str) -> std::result::Result<(), EditError> {
        self.nodes = self.nodes.saturating_add(1);
        self.bytes = self
            .bytes
            .saturating_add(key.len())
            .saturating_add(value.len());
        if self.nodes > MAX_NODES || self.bytes > MAX_BYTES {
            return Err(invalid("Metadata exceeds 131072 nodes or 8 MiB"));
        }
        if key.contains('\0') || value.contains('\0') {
            return Err(invalid("VMF metadata fields cannot contain NUL bytes"));
        }
        source_formats::keyvalues::validate_string(key)
            .map_err(|error| invalid(error.to_string()))?;
        source_formats::keyvalues::validate_string(value)
            .map_err(|error| invalid(error.to_string()))?;
        Ok(())
    }
    pub(crate) fn properties(
        &mut self,
        properties: &Properties,
    ) -> std::result::Result<(), EditError> {
        for (key, value) in &properties.0 {
            self.text(key, value)?;
        }
        Ok(())
    }
    pub(crate) fn editor(
        &mut self,
        editor: Option<&Properties>,
    ) -> std::result::Result<(), EditError> {
        if let Some(editor) = editor {
            self.properties(editor)?;
        }
        Ok(())
    }
    pub(crate) fn nodes(&mut self, roots: &[Node]) -> std::result::Result<(), EditError> {
        if roots.len() > MAX_NODES.saturating_sub(self.nodes) {
            return Err(invalid("Metadata exceeds 131072 nodes"));
        }
        let mut stack = roots.iter().map(|node| (node, 1)).collect::<Vec<_>>();
        while let Some((node, depth)) = stack.pop() {
            if depth > MAX_DEPTH {
                return Err(invalid("Metadata exceeds 64 levels"));
            }
            self.text(&node.key, node.as_str().unwrap_or_default())?;
            if let Some(children) = node.children() {
                if children.len() + stack.len() > MAX_NODES.saturating_sub(self.nodes) {
                    return Err(invalid("Metadata exceeds 131072 nodes"));
                }
                stack.extend(children.iter().map(|child| (child, depth + 1)));
            }
        }
        Ok(())
    }
}
