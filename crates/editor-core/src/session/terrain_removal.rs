use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use map_core::clip::{ClipOptions, ClipResult, TerrainBlendMode, TerrainMode};
use map_core::displacement::removal;
use map_core::surface::FaceId;
use map_core::{Document, EditError, carve, subobject};
use serde_json::{Value, json};

use super::clipping::{self, context, materials};
use super::components::{self, ridge_bindings};
use super::terrain_vertices::VertexSource;
use super::{Actor, SelectMode, Session};

pub struct Request<'a> {
    pub targets: &'a [(FaceId, Vec<usize>)],
    pub source: VertexSource,
    pub blend_mode: TerrainBlendMode,
    pub partition_neighbors: bool,
}

fn options<'a>(
    blend_mode: TerrainBlendMode,
    partition_neighbors: bool,
    materials: Option<&'a materials::Prepared>,
    context: Option<&'a context::Context>,
) -> ClipOptions<'a> {
    ClipOptions {
        terrain_mode: TerrainMode::Fragments,
        blend_mode,
        partition_neighbors,
        textures: materials.map(|materials| &materials.textures),
        terrain_context: context.map(context::Context::vmf),
    }
}

#[derive(Clone)]
pub(super) struct Preview {
    pub(super) document: Document,
    pub(super) render_error: Option<String>,
    result: ClipResult,
    token: u64,
    targets: Vec<(FaceId, Vec<usize>)>,
    source: VertexSource,
    definitions: u64,
    blend_mode: TerrainBlendMode,
    partition_neighbors: bool,
    face_mapping: BTreeMap<u32, Vec<u32>>,
    cubemap_bindings: ridge_bindings::Plan,
    terrain_context: Option<Arc<context::Context>>,
    terrain_materials: Option<Arc<materials::Prepared>>,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) preview: Option<Preview>,
    serial: u64,
}

pub fn result_json(result: &ClipResult, blend: TerrainBlendMode, partition: bool) -> Value {
    let mut report = clipping::terrain_report(&result.terrain, blend, partition);
    report["operation"] = json!("triangle_removal");
    report["terrain_mode"] = json!("fragments");
    report["cuts"] = json!(
        result
            .cuts
            .iter()
            .map(|cut| json!({"source":cut.source,"solids":cut.solids}))
            .collect::<Vec<_>>()
    );
    report["removed"] = json!(
        result
            .cuts
            .iter()
            .filter(|cut| cut.solids.is_empty())
            .map(|cut| cut.source)
            .collect::<Vec<_>>()
    );
    report["unchanged"] = json!(result.unchanged);
    report["objects"] = json!(
        result
            .objects
            .iter()
            .copied()
            .map(crate::identity::id_string)
            .collect::<Vec<_>>()
    );
    report["side_mapping"] = json!(result.side_mapping);
    report
}

impl Session {
    pub fn terrain_removal_preview_token(&self) -> Option<u64> {
        self.geometry
            .terrain_removal
            .preview
            .as_ref()
            .map(|preview| preview.token)
    }

    pub fn terrain_removal_preview_error(&self) -> Option<String> {
        self.terrain_removal_preview_token()
            .and_then(|token| self.checked_terrain_removal(token).err())
    }

    pub fn terrain_removal_report(&self) -> Value {
        json!({"source":self.terrain_vertex_source().json(),"handle":self.terrain_removal_handle(),
            "limits":clipping::terrain_limits(),"preview":self.geometry.terrain_removal.preview.as_ref().map(|preview| {
                let error = self.checked_terrain_removal(preview.token).err();
                let mut value = json!({"token":preview.token.to_string(),"source":preview.source.json(),"state":if error.is_some(){"stale"}else{"ready"},
                    "guard_error":error,"targets":preview.targets.iter().map(|(face,ids)|json!({"face":{"solid":face.solid,"side":face.side},"triangles":ids})).collect::<Vec<_>>(),
                    "blend_mode":clipping::blend_mode_name(preview.blend_mode),"partition_neighbors":preview.partition_neighbors,
                    "result":result_json(&preview.result,preview.blend_mode,preview.partition_neighbors),"cubemap_bindings":preview.cubemap_bindings.report()});
                if let Some(context) = &preview.terrain_context { value["terrain_context"] = context.report(); }
                if let Some(materials) = &preview.terrain_materials { value["terrain_materials"] = materials.report(); }
                value
            }),"policy":"Remove checked original terrain triangles through real native patches. Empty triangle rows authorize neighbors only with partition_neighbors:true. Removing all triangles deletes the parent brush; empty brush entities reject. Cut borders remain open."})
    }

    pub(super) fn terrain_removal_handle(&self) -> Value {
        Self::edit_handle(
            "terrain_triangle_removal",
            self.terrain_removal_preview_token()
                .filter(|token| self.checked_terrain_removal(*token).is_ok()),
        )
    }

    pub fn preview_terrain_triangle_removal(&mut self, request: Request<'_>) -> Result<(), String> {
        if request.source != self.terrain_vertex_source() {
            return Err("Map changed; inspect source terrain triangles again".into());
        }
        let mut targets = request.targets.to_vec();
        targets.sort_by_key(|(face, _)| *face);
        for (_, ids) in &mut targets {
            ids.sort_unstable();
            ids.dedup();
        }
        let required = removal::requires_terrain_context(&self.document, &targets)
            .map_err(|error| error.to_string())?;
        components::admit_fragment_snapshot(self.document.vmf())?;
        let terrain_context = context::Context::prepare_required(self, required)?;
        let mut preparation = carve::Preparation::default();
        let mut faces = removal::texture_faces(
            &self.document,
            &targets,
            &options(
                request.blend_mode,
                request.partition_neighbors,
                None,
                terrain_context.as_deref(),
            ),
            &mut preparation,
        )
        .map_err(|error| error.to_string())?;
        let mut terrain_materials = materials::Prepared::prepare(self, &faces)?;
        if request.partition_neighbors {
            loop {
                let required = removal::partition_faces(
                    &self.document,
                    &targets,
                    options(
                        request.blend_mode,
                        true,
                        terrain_materials.as_deref(),
                        terrain_context.as_deref(),
                    ),
                    &mut preparation,
                )
                .map_err(|error| error.to_string())?;
                if required.is_empty() {
                    break;
                }
                let previous = faces.len();
                faces.extend(required);
                faces.sort_unstable();
                faces.dedup();
                if faces.len() <= previous || faces.len() > map_core::clip::MAX_TERRAIN_SOURCES {
                    return Err("Terrain removal dependency discovery exceeded its source or progress limit".into());
                }
                if let Some(previous) = &terrain_materials {
                    previous.verify(&self.game)?;
                }
                let updated = materials::Prepared::prepare(self, &faces)?;
                if let Some(previous) = &terrain_materials {
                    previous.verify(&self.game)?;
                }
                terrain_materials = updated;
            }
        }
        let mut document = self.document.preview_snapshot();
        let result = document
            .edit("preview terrain triangle removal", |tx| {
                removal::remove(
                    tx,
                    &targets,
                    options(
                        request.blend_mode,
                        request.partition_neighbors,
                        terrain_materials.as_deref(),
                        terrain_context.as_deref(),
                    ),
                    &mut preparation,
                )
            })
            .map_err(|error| error.to_string())?;
        let sources = result
            .cuts
            .iter()
            .map(|cut| cut.source)
            .collect::<BTreeSet<_>>();
        let face_mapping = self.prepare_fragment_mapping(&sources, &result.side_mapping)?;
        let cubemap_bindings =
            self.prepare_fragment_cubemap_bindings(&mut document, &face_mapping)?;
        if let Some(context) = &terrain_context {
            context.verify()?;
        }
        if let Some(materials) = &terrain_materials {
            materials.verify(&self.game)?;
        }
        self.cancel_block_preview();
        self.clear_geometry_previews_except(&[]);
        self.cancel_area_selection();
        self.clear_volume_selection();
        self.retire_terrain_move_handles();
        self.retire_terrain_stroke();
        self.clip_tool = false;
        self.geometry.paint.enabled = false;
        self.spatial.draw_block = false;
        self.geometry.terrain_removal.serial += 1;
        self.geometry.terrain_removal.preview = Some(Preview {
            document,
            result,
            render_error: None,
            token: self.geometry.terrain_removal.serial,
            targets,
            source: request.source,
            definitions: self.definitions_serial,
            blend_mode: request.blend_mode,
            partition_neighbors: request.partition_neighbors,
            face_mapping,
            cubemap_bindings,
            terrain_context,
            terrain_materials,
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    fn checked_terrain_removal(&self, token: u64) -> Result<&Preview, String> {
        let preview = self
            .geometry
            .terrain_removal
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Terrain triangle removal preview was closed or replaced")?;
        if preview.source != self.terrain_vertex_source()
            || preview.definitions != self.definitions_serial
        {
            return Err(
                "Map or definitions changed; preview terrain triangle removal again".into(),
            );
        }
        if let Some(context) = &preview.terrain_context {
            context.current(self)?;
        }
        if let Some(materials) = &preview.terrain_materials {
            materials.current(self)?;
        }
        if let Some(error) = preview.cubemap_bindings.stale_error(self) {
            return Err(error);
        }
        Ok(preview)
    }

    pub fn commit_terrain_triangle_removal(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        let preview = self.checked_terrain_removal(token)?.clone();
        let solids = preview
            .result
            .cuts
            .iter()
            .flat_map(|cut| &cut.solids)
            .map(|id| subobject::solid(&preview.document, *id).map(|(_, solid)| solid.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        self.check_fragment_references(
            &preview.result.cuts.iter().map(|cut| cut.source).collect(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        )?;
        let game = self.game.clone();
        let result = self.edit(actor, "remove terrain triangles", |tx| {
            let writes = ridge_bindings::writes_for_mapping(tx.vmf(), &preview.face_mapping)
                .map_err(EditError::Invalid)?;
            if writes != preview.cubemap_bindings.writes {
                return Err(EditError::Invalid(
                    "Cubemap bindings changed; preview terrain triangle removal again".into(),
                ));
            }
            let result = removal::remove(
                tx,
                &preview.targets,
                options(
                    preview.blend_mode,
                    preview.partition_neighbors,
                    preview.terrain_materials.as_deref(),
                    preview.terrain_context.as_deref(),
                ),
                &mut carve::Preparation::default(),
            )?;
            if result != preview.result
                || solids.iter().any(|solid| {
                    subobject::solid(tx.document(), solid.id)
                        .map_or(true, |(_, actual)| actual != solid)
                })
            {
                return Err(EditError::Invalid(
                    "Terrain geometry or allocation changed; preview triangle removal again".into(),
                ));
            }
            ridge_bindings::apply(tx, &writes)?;
            for source in &preview.cubemap_bindings.sources {
                source
                    .verify(&game, &compiler::Control::default())
                    .map_err(EditError::Invalid)?;
            }
            if let Some(context) = &preview.terrain_context {
                context.verify().map_err(EditError::Invalid)?;
            }
            if let Some(materials) = &preview.terrain_materials {
                materials.verify(&game).map_err(EditError::Invalid)?;
            }
            Ok(result)
        })?;
        self.retire_clipped_terrain_state();
        self.select(&result.objects, SelectMode::Replace)?;
        Ok(result_json(
            &result,
            preview.blend_mode,
            preview.partition_neighbors,
        ))
    }

    pub fn cancel_terrain_triangle_removal(&mut self, token: u64) -> Result<(), String> {
        self.geometry
            .terrain_removal
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Terrain triangle removal preview was closed or replaced")?;
        self.clear_terrain_removal_preview();
        Ok(())
    }

    pub(super) fn clear_terrain_removal_preview(&mut self) {
        if self.geometry.terrain_removal.preview.take().is_some() {
            self.geometry.terrain_removal.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn preflight_terrain_removal_preview(&mut self) {
        let error = self.terrain_removal_preview_error();
        if let Some(preview) = &mut self.geometry.terrain_removal.preview
            && preview.render_error != error
        {
            preview.render_error = error;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn verify_terrain_removal_render(&self) -> Result<(), String> {
        match self.terrain_removal_preview_error() {
            Some(error) => Err(format!(
                "Terrain triangle removal preview unavailable: {error}. Preview again or discard."
            )),
            None => Ok(()),
        }
    }

    pub(super) fn append_terrain_removal_diagnostic(&self, mesh: &mut render::SceneMesh) {
        if let Some(preview) = &self.geometry.terrain_removal.preview
            && let Some(error) = &preview.render_error
            && let Some((face, _)) = preview.targets.first()
        {
            mesh.issues.prepend(render::scene::SceneIssue::Surface {face:*face,message:format!("Terrain triangle removal preview unavailable: {error}. Applied map shown; preview again or discard.")});
        }
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<Value> {
        self.preview
            .as_ref()
            .map(|preview| json!([self.serial, preview.token]))
    }
}
