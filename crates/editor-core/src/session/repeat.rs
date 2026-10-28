use crate::identity::id_string;
use map_core::{Document, EditError, ObjectId, repeat};
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};

pub struct Preview {
    pub document: Document,
    pub result: repeat::Result,
    token: u64,
    sources: Vec<ObjectId>,
    options: repeat::Options,
    local: bool,
    generation: u64,
    revision: u64,
    definitions: u64,
}

#[derive(Default)]
pub struct State {
    serial: u64,
    pub preview: Option<Preview>,
}

impl Session {
    pub fn repeat_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.definitions_serial,
            self.scene_serial,
            self.geometry.repeat.serial
        )
    }

    pub fn repeat_array(&self) -> Value {
        let preview = self.geometry.repeat.preview.as_ref().map(|preview| {
            let objects = preview.result.objects();
            json!({
                "token": preview.token,
                "sources": preview.sources.iter().copied().map(id_string).collect::<Vec<_>>(),
                "count": preview.options.count,
                "translation": preview.options.translation.to_array(),
                "rotation": preview.options.rotation.to_array(),
                "scale": preview.options.scale.to_array(),
                "pivot": preview.options.pivot.to_array(),
                "space": if preview.local { "local" } else { "world" },
                "basis": preview.options.basis.to_cols_array_2d(),
                "frame_source": preview.local.then(|| preview.sources.iter().copied().min().map(id_string)).flatten(),
                "texture_lock": preview.options.texture_lock,
                "bounds": self.object_bounds_in(&preview.document, &objects).map(|(mins, maxs)| [mins.to_array(), maxs.to_array()]),
                "copies": preview.result.copies.iter().map(|copy| json!({
                    "index": copy.index,
                    "scale": preview.options.copy_scale(copy.index).to_array(),
                    "objects": copy.objects.iter().copied().map(id_string).collect::<Vec<_>>(),
                    "names": copy.names.iter().map(|name| json!({"source":id_string(ObjectId::Entity(name.source)), "copy":id_string(ObjectId::Entity(name.copy)), "before":name.before, "after":name.after})).collect::<Vec<_>>(),
                    "remapped_references": copy.remapped_references,
                })).collect::<Vec<_>>(),
                "entities": objects.iter().filter_map(|id| {
                    let ObjectId::Entity(id) = id else { return None };
                    let entity = preview.document.entity(*id)?;
                    Some(json!({"id":id_string(ObjectId::Entity(*id)),"origin":entity.origin().map(|origin| origin.to_array()),"properties":entity.properties.0,"connections":entity.connections.iter().flatten().map(|connection|json!({"output":connection.output,"value":connection.value})).collect::<Vec<_>>()}))
                }).collect::<Vec<_>>(),
                "object_count": objects.len(),
                "valid": preview.generation == self.document_generation && preview.revision == self.document.revision() && preview.definitions == self.definitions_serial,
            })
        });
        json!({
            "handle": self.repeat_array_handle(),
            "snapshot": self.repeat_stamp(),
            "preview": preview,
            "limits": {"sources":repeat::MAX_SOURCES,"copies":repeat::MAX_COPIES,"objects_including_owned_brushes":repeat::MAX_OBJECTS,"sides":repeat::MAX_SIDES,"name_bytes":repeat::MAX_NAME_BYTES},
            "transform_policy":"Copy n uses the original objects, scales each XYZ axis by its per-copy factor raised to n, then rotates by n × XYZ degrees, then translates by n × the move step. Scale and rotation share one fixed world pivot; all steps use the chosen world or captured source-local frame. Source local uses the first source in stable object-ID order (brushes before entities), captured once for every copy. Scale defaults to 1 and nonidentity factors support only brushes and brush entities. Count excludes originals.",
            "reference_policy":"Unique source targetnames become name_copyN, with a collision suffix when needed. Certain exact internal FGD/I/O/parent references follow that copy; external links and procedural references remain unchanged. Shared names, ambiguous internal matches, wildcard captures of new names, overlay face-ID references and node IDs reject. Named source sets with scripts or dynamic I/O also reject.",
            "metadata_policy":"Object and owned-brush context notes copy with their existing authorship/observations and full map budgets. Saved partitions remain attached to originals. Other opaque custom metadata and embedded text stay verbatim; their semantics are not inferred or remapped.",
        })
    }

    pub(super) fn repeat_array_handle(&self) -> Value {
        Self::edit_handle(
            "repeat_array",
            self.geometry.repeat.preview.as_ref().and_then(|preview| {
                self.checked_repeat(preview.token)
                    .is_ok()
                    .then_some(preview.token)
            }),
        )
    }

    pub fn clear_repeat_preview(&mut self) {
        if self.geometry.repeat.preview.take().is_some() {
            self.geometry.repeat.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    fn checked_repeat(&self, token: u64) -> Result<&Preview, String> {
        let preview = self
            .geometry
            .repeat
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Repeat array was closed or replaced")?;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
            || preview.definitions != self.definitions_serial
        {
            return Err("Map or entity definitions changed; preview the array again".into());
        }
        Ok(preview)
    }

    pub fn preview_repeat_array(
        &mut self,
        sources: &[ObjectId],
        options: repeat::Options,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.preview_repeat_array_in_space(sources, options, false, snapshot)
    }

    pub fn preview_repeat_array_in_space(
        &mut self,
        sources: &[ObjectId],
        mut options: repeat::Options,
        local: bool,
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.repeat_stamp() {
            return Err("Map, selection or preview changed; inspect the repeat array again".into());
        }
        self.ensure_partitions_resolved()?;
        options.basis = self.transform_basis(sources, local)?;
        let mut document = self.document.preview_snapshot();
        let result = document
            .edit("preview repeat array", |tx| {
                repeat::create(tx, sources, options, &self.definitions)
            })
            .map_err(|error| error.to_string())?;
        self.select(sources, SelectMode::Replace)?;
        self.cancel_block_preview();
        self.clear_geometry_previews_except(&[]);

        self.cancel_area_selection();
        self.clear_volume_selection();

        self.clip_tool = false;
        self.geometry.paint.enabled = false;
        self.spatial.draw_block = false;
        self.geometry.repeat.serial += 1;
        self.geometry.repeat.preview = Some(Preview {
            document,
            result,
            token: self.geometry.repeat.serial,
            sources: sources.to_vec(),
            options,
            local,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(self.repeat_array())
    }

    pub fn apply_repeat_array(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let preview = self.checked_repeat(token)?;
        let expected = preview.document.vmf().clone();
        let sources = preview.sources.clone();
        let options = preview.options;
        let expected_result = preview.result.clone();
        let definitions = self.definitions.clone();
        let result = self.edit(actor, "repeat array", |tx| {
            let result = repeat::create(tx, &sources, options, &definitions)?;
            if result != expected_result || tx.document().vmf() != &expected {
                return Err(EditError::Invalid(
                    "Array differs from its preview; preview the array again".into(),
                ));
            }
            Ok(result)
        })?;
        let copies = result.objects();
        self.select(&copies, SelectMode::Replace)?;
        Ok(
            json!({"handle":self.repeat_array_handle(),"objects":copies.iter().copied().map(id_string).collect::<Vec<_>>(), "count":result.copies.len(),"state":self.repeat_array()}),
        )
    }

    pub fn cancel_repeat_array(&mut self, token: u64) -> Result<(), String> {
        self.geometry
            .repeat
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Repeat array was closed or replaced")?;
        self.clear_paste_preview();
        self.clear_repeat_preview();
        Ok(())
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.preview
            .as_ref()
            .map(|preview| serde_json::json!([self.serial, preview.token]))
    }
}
