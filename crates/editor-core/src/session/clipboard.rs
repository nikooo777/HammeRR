use std::sync::Arc;

use map_core::{Document, EditError, ObjectId, edit, transfer};
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};

pub trait Transport: Send {
    fn read(&mut self) -> Result<String, String>;
    fn write(&mut self, text: &str) -> Result<(), String>;
}

#[derive(Default)]
struct Native {
    clipboard: Option<arboard::Clipboard>,
}

impl Native {
    fn open(&mut self) -> Result<&mut arboard::Clipboard, String> {
        if self.clipboard.is_none() {
            self.clipboard = Some(
                arboard::Clipboard::new()
                    .map_err(|error| format!("System clipboard unavailable: {error}"))?,
            );
        }
        Ok(self.clipboard.as_mut().unwrap())
    }
}

impl Transport for Native {
    fn read(&mut self) -> Result<String, String> {
        self.open()?
            .get_text()
            .map_err(|error| format!("Cannot read system clipboard text: {error}"))
    }
    fn write(&mut self, text: &str) -> Result<(), String> {
        self.open()?
            .set_text(text)
            .map_err(|error| format!("Cannot publish system clipboard text: {error}"))
    }
}

pub struct Preview {
    pub document: Document,
    pub result: transfer::Result,
    token: u64,
    generation: u64,
    revision: u64,
    definitions: u64,
    content: u64,
    options: transfer::Options,
}

struct Receipt {
    generation: u64,
    revision: u64,
    definitions: u64,
    content: u64,
}

pub struct State {
    payload: Option<Arc<transfer::Payload>>,
    content: u64,
    serial: u64,
    receipt: Option<Receipt>,
    transport: Box<dyn Transport>,
    pub preview: Option<Preview>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            payload: None,
            content: 0,
            serial: 0,
            receipt: None,
            transport: Box::<Native>::default(),
            preview: None,
        }
    }
}

fn ids(ids: &[ObjectId]) -> Vec<String> {
    ids.iter()
        .copied()
        .map(crate::identity::id_string)
        .collect()
}

fn result_json(result: &transfer::Result) -> Value {
    json!({
        "objects":ids(&result.objects),
        "object_ids":result.mapping.iter().map(|(old,new)|json!({"source":crate::identity::id_string(*old),"copy":crate::identity::id_string(*new)})).collect::<Vec<_>>(),
        "side_ids":result.sides,
        "visgroup_ids":result.visgroups,
        "group_ids":result.groups,
        "names":result.names.iter().map(|name|json!({"source":name.source,"copy":name.copy,"before":name.before,"after":name.after})).collect::<Vec<_>>(),
        "remapped_references":result.remapped_references,
        "external_links":result.external.iter().map(|link|json!({"source":link.source,"target":link.target,"location":link.location,"destination_matches":link.matches})).collect::<Vec<_>>()
    })
}

impl Session {
    pub fn clipboard_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.definitions_serial,
            self.scene_serial,
            self.geometry.clipboard.content,
            self.geometry.clipboard.serial
        )
    }

    pub fn can_restore_cut_names(&self) -> bool {
        self.geometry
            .clipboard
            .receipt
            .as_ref()
            .is_some_and(|receipt| {
                receipt.generation == self.document_generation
                    && receipt.revision == self.document.revision()
                    && receipt.definitions == self.definitions_serial
                    && receipt.content == self.geometry.clipboard.content
            })
    }

    pub fn clipboard_report(&self) -> Value {
        let payload = self.geometry.clipboard.payload.as_ref().map(|payload| {
            let links=payload.external(&self.definitions,&self.document);
            json!({
            "content_revision":self.geometry.clipboard.content,
            "bytes":payload.text().len(),
            "objects":ids(payload.ids()),
            "groups":payload.document().vmf().world.groups.len(),
            "layers":map_core::layers::Graph::read(payload.document()).map(|graph|graph.records.len()).ok(),
            "sides":payload.document().vmf().side_ids().count(),
            "bounds":map_core::query::bounds_of(payload.document(),payload.ids()).map(|(mins,maxs)|[mins.to_array(),maxs.to_array()]),
            "external_links":links.as_ref().ok().map(|links|links.iter().map(|link|json!({"source":link.source,"target":link.target,"location":link.location,"destination_matches":link.matches})).collect::<Vec<_>>()),
            "reference_error":links.err().map(|error|error.to_string()),
        }) });
        let preview = self.geometry.clipboard.preview.as_ref().map(|preview| json!({
            "token":preview.token,
            "valid":self.checked_paste(preview.token).is_ok(),
            "content_revision":preview.content,
            "offset":preview.options.offset.to_array(),
            "texture_lock":preview.options.texture_lock,
            "keep_external_links":preview.options.keep_external,
            "restore_cut_names":preview.options.restore_names,
            "bounds":map_core::query::bounds_of(&preview.document,&preview.result.objects).map(|(mins,maxs)|[mins.to_array(),maxs.to_array()]),
            "result":result_json(&preview.result),
        }));
        json!({"snapshot":self.clipboard_stamp(),"payload":payload,"preview":preview,"can_restore_cut_names":self.can_restore_cut_names(),
            "native_format":"UTF-8 VMF text; system access is explicit and may be unavailable on headless displays. Private Hammer clipboard formats are unsupported.",
            "limits":{"bytes":transfer::MAX_BYTES,"objects":map_core::repeat::MAX_SOURCES,"objects_including_owned_brushes":map_core::repeat::MAX_OBJECTS,"sides":map_core::repeat::MAX_SIDES,"groups":transfer::MAX_GROUPS,"layers":transfer::MAX_GROUPS,"depth":transfer::MAX_DEPTH,"nodes":transfer::MAX_NODES},
            "reference_policy":"Unique names and exact internal FGD/I/O/parent links remap. External links require explicit keep-by-name. Uncertain/script-driven links, numeric references, saved partitions reject. Opaque metadata remains verbatim.",
        })
    }

    fn check_clipboard_stamp(&self, snapshot: &str) -> Result<(), String> {
        if self.clipboard_stamp() != snapshot {
            return Err("Map, selection or clipboard changed; inspect the clipboard again".into());
        }
        Ok(())
    }

    fn install_clipboard(&mut self, payload: transfer::Payload) {
        self.clear_paste_preview();
        self.geometry.clipboard.payload = Some(Arc::new(payload));
        self.geometry.clipboard.content += 1;
        self.geometry.clipboard.receipt = None;
        self.view_serial += 1;
    }

    pub fn capture_clipboard(
        &mut self,
        actor: Actor,
        sources: &[ObjectId],
        snapshot: &str,
        cut: bool,
        system: bool,
    ) -> Result<Value, String> {
        self.check_clipboard_stamp(snapshot)?;
        self.ensure_partitions_resolved()?;
        let payload = transfer::Payload::capture(&self.document, sources, &self.definitions)
            .map_err(|error| error.to_string())?;
        if cut {
            let mut draft = self.document.preview_snapshot();
            draft
                .edit("prepare cut", |tx| edit::delete(tx, sources))
                .map_err(|error| error.to_string())?;
        }
        if system {
            self.geometry.clipboard.transport.write(payload.text())?;
        }
        self.install_clipboard(payload);
        if cut {
            self.edit(actor, "cut objects", |tx| edit::delete(tx, sources))?;
            self.geometry.clipboard.receipt = Some(Receipt {
                generation: self.document_generation,
                revision: self.document.revision(),
                definitions: self.definitions_serial,
                content: self.geometry.clipboard.content,
            });
        } else {
            self.log(
                actor,
                format!("copied {} objects to clipboard", sources.len()),
            );
        }
        Ok(self.clipboard_report())
    }

    pub fn import_clipboard(
        &mut self,
        actor: Actor,
        text: Option<&str>,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.check_clipboard_stamp(snapshot)?;
        let native;
        let text = if let Some(text) = text {
            text
        } else {
            native = self.geometry.clipboard.transport.read()?;
            &native
        };
        if self
            .geometry
            .clipboard
            .payload
            .as_ref()
            .is_some_and(|payload| payload.text() == text)
        {
            return Ok(self.clipboard_report());
        }
        let payload = transfer::Payload::decode(text, &self.definitions)
            .map_err(|error| error.to_string())?;
        self.install_clipboard(payload);
        self.log(actor, "imported VMF clipboard payload");
        Ok(self.clipboard_report())
    }

    pub fn export_clipboard(&mut self, content: u64, system: bool) -> Result<Value, String> {
        if content != self.geometry.clipboard.content {
            return Err("Clipboard changed; inspect it again".into());
        }
        let payload = self
            .geometry
            .clipboard
            .payload
            .as_ref()
            .ok_or("Clipboard is empty")?;
        if system {
            self.geometry.clipboard.transport.write(payload.text())?;
            Ok(json!({"published":true,"content_revision":content,"bytes":payload.text().len()}))
        } else {
            Ok(json!({"content_revision":content,"format":"vmf","text":payload.text()}))
        }
    }

    pub fn clear_paste_preview(&mut self) {
        if self.geometry.clipboard.preview.take().is_some() {
            self.geometry.clipboard.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    fn checked_paste(&self, token: u64) -> Result<&Preview, String> {
        let preview = self
            .geometry
            .clipboard
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Paste preview was closed or replaced")?;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
            || preview.definitions != self.definitions_serial
            || preview.content != self.geometry.clipboard.content
        {
            return Err(
                "Map, clipboard or entity definitions changed; preview the paste again".into(),
            );
        }
        if preview.options.restore_names && !self.can_restore_cut_names() {
            return Err("Cut-name receipt changed; preview with unique names".into());
        }
        Ok(preview)
    }

    pub fn preview_paste(
        &mut self,
        options: transfer::Options,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.check_clipboard_stamp(snapshot)?;
        self.ensure_partitions_resolved()?;
        if options.restore_names && !self.can_restore_cut_names() {
            return Err(
                "Restore cut names requires an unchanged same-map Cut; use unique names".into(),
            );
        }
        let payload = self
            .geometry
            .clipboard
            .payload
            .clone()
            .ok_or("Copy objects or import VMF text first")?;
        let mut document = self.document.preview_snapshot();
        let result = document
            .edit("preview paste", |tx| {
                transfer::paste(tx, &payload, options, &self.definitions)
            })
            .map_err(|error| error.to_string())?;
        self.changed();
        self.clear_paint_stroke();
        self.clip_tool = false;
        self.geometry.paint.enabled = false;
        self.spatial.draw_block = false;
        self.geometry.clipboard.serial += 1;
        self.geometry.clipboard.preview = Some(Preview {
            document,
            result,
            token: self.geometry.clipboard.serial,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            content: self.geometry.clipboard.content,
            options,
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(self.clipboard_report())
    }

    pub fn apply_paste(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let preview = self.checked_paste(token)?;
        let expected = preview.document.vmf().clone();
        let expected_result = preview.result.clone();
        let options = preview.options;
        let payload = self.geometry.clipboard.payload.clone().unwrap();
        let catalog = self.definitions.clone();
        let result = self.edit(actor, "paste objects", |tx| {
            let result = transfer::paste(tx, &payload, options, &catalog)?;
            if tx.document().vmf() != &expected || result != expected_result {
                return Err(EditError::Invalid(
                    "Paste differs from its preview; preview again".into(),
                ));
            }
            Ok(result)
        })?;
        self.select(&result.objects, SelectMode::Replace)?;
        self.geometry.clipboard.receipt = None;
        Ok(result_json(&result))
    }

    pub fn cancel_paste(&mut self, token: u64) -> Result<Value, String> {
        self.geometry
            .clipboard
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Paste preview was closed or replaced")?;
        self.clear_paste_preview();
        Ok(self.clipboard_report())
    }
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) mod fixtures;

#[cfg(test)]
pub(crate) mod tests;
