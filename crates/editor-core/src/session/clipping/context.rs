use std::path::PathBuf;
use std::sync::Arc;

use compiler::instances::{self, DependencyGuard, InstanceStats, OccurrencePath, SourceContext};
use map_core::clip::{self, TerrainMode};
use map_core::{ObjectId, Plane};
use serde_json::{Value, json};
use source_formats::vmf::Vmf;

use super::Session;

const MAX_CONTEXT_BYTES: usize = 64 * 1024 * 1024;
const GUARD_BYTES: usize = 64 * 1024;
const MAX_PROVENANCE_ROWS: usize = 128;

struct Origin {
    side: u32,
    owner: u32,
    file: String,
    solid: u32,
    original_side: u32,
    occurrence: Arc<OccurrencePath>,
}

pub(in crate::session) struct Context {
    vmf: Vmf,
    dependencies: DependencyGuard,
    stats: InstanceStats,
    origins: Vec<Origin>,
    path: PathBuf,
    saved: u64,
    content: u64,
}

impl Context {
    pub(in crate::session) fn prepare(
        session: &Session,
        ids: &[ObjectId],
        plane: Plane,
        terrain_mode: TerrainMode,
        retile_crossing: bool,
    ) -> Result<Option<Arc<Self>>, String> {
        let vmf = session.document.vmf();
        let needed = match terrain_mode {
            TerrainMode::Grid => clip::requires_terrain_context(&session.document, ids, plane)
                .map_err(|error| error.to_string())?,
            TerrainMode::Flat | TerrainMode::Cells | TerrainMode::Fragments => {
                retile_crossing
                    && vmf.entities.iter().any(|entity| {
                        !entity.hidden
                            && entity
                                .classname()
                                .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
                    })
            }
        };
        Self::prepare_required(session, needed)
    }

    pub(in crate::session) fn prepare_required(
        session: &Session,
        needed: bool,
    ) -> Result<Option<Arc<Self>>, String> {
        if !needed {
            return Ok(None);
        }
        let vmf = session.document.vmf();
        let control = compiler::Control::default();
        let original_bytes =
            map_core::references::admit_snapshot_storage(vmf, MAX_CONTEXT_BYTES, &mut || {
                control.checkpoint()
            })?;
        let path = session.path.as_ref().ok_or(
            "Save the parent VMF before clipping terrain with visible instance references",
        )?;
        let source = SourceContext::capture(path, &control).map_err(|error| error.to_string())?;
        let mut report = compiler::Report::default();
        let prepared =
            instances::resolve(vmf, Some(&source), &control, &mut report).map_err(|error| {
                format!("Terrain instance context is unavailable: {}", error.message)
            })?;
        if report.has_errors() {
            return Err("Terrain instance context has unresolved preparation errors".into());
        }
        let context_bytes = map_core::references::admit_snapshot_storage(
            prepared.vmf(),
            MAX_CONTEXT_BYTES,
            &mut || control.checkpoint(),
        )?;
        if map_core::displacement::native::visible_terrain_sources(prepared.vmf()).count()
            > source_formats::bsp::MAX_DISPLACEMENTS
        {
            return Err(
                "Resolved terrain context exceeds the native displacement patch limit".into(),
            );
        }
        let mut retained = original_bytes
            .checked_add(context_bytes)
            .and_then(|bytes| bytes.checked_add(prepared.occurrence_bytes()))
            .and_then(|bytes| bytes.checked_add(GUARD_BYTES))
            .filter(|bytes| *bytes <= MAX_CONTEXT_BYTES)
            .ok_or("Terrain cut and resolved instance context exceed the 64 MiB snapshot budget")?;
        let mut origins = Vec::new();
        for brush in prepared.generated_brushes() {
            for side in brush
                .solid
                .sides
                .iter()
                .filter(|side| side.displacement.is_some())
            {
                let (owner, file, solid, original_side) = prepared
                    .face_origin(side.id)
                    .ok_or("Resolved instance terrain has no source face provenance")?;
                let occurrence = prepared
                    .face_occurrence(side.id)
                    .ok_or("Resolved instance terrain has no occurrence provenance")?;
                retained = retained
                    .checked_add(2 * std::mem::size_of::<Origin>() + file.len())
                    .filter(|bytes| *bytes <= MAX_CONTEXT_BYTES)
                    .ok_or("Resolved instance terrain provenance exceeds the snapshot budget")?;
                origins.push(Origin {
                    side: side.id,
                    owner,
                    file: file.to_owned(),
                    solid,
                    original_side,
                    occurrence: occurrence.clone(),
                });
            }
        }
        let dependencies = prepared
            .dependencies()
            .cloned()
            .ok_or("Resolved terrain context has no complete dependency guard")?;
        let stats = prepared
            .stats()
            .cloned()
            .ok_or("Resolved terrain context has no assembly statistics")?;
        let context = Self {
            vmf: prepared.into_vmf(),
            dependencies,
            stats,
            origins,
            path: path.clone(),
            saved: session.history_save_serial,
            content: session.build_content_epoch(),
        };
        Ok(Some(Arc::new(context)))
    }

    pub(in crate::session) fn current(&self, session: &Session) -> Result<(), String> {
        if session.path.as_ref() != Some(&self.path)
            || self.saved != session.history_save_serial
            || self.content != session.build_content_epoch()
        {
            return Err(
                "Map path, saved source or content changed; preview the terrain cut again".into(),
            );
        }
        self.dependencies
            .verify_metadata()
            .map_err(|error| format!("Terrain instance sources changed: {error}"))
    }

    pub(in crate::session) fn verify(&self) -> Result<(), String> {
        self.dependencies
            .verify(&compiler::Control::default())
            .map_err(|error| format!("Terrain instance sources changed: {error}"))
    }

    pub(in crate::session) fn vmf(&self) -> &Vmf {
        &self.vmf
    }

    pub(in crate::session) fn report(&self) -> Value {
        json!({"references":self.stats.instances,"terrain_faces":self.origins.len(),"dependencies":self.stats.dependencies,
            "sources":self.origins.iter().take(MAX_PROVENANCE_ROWS).map(|origin| json!({
                "assembled_side":origin.side,"owner":format!("entity:{}",origin.owner),"file":origin.file,
                "child_solid":origin.solid,"child_side":origin.original_side,"occurrence":origin.occurrence,
            })).collect::<Vec<_>>(),"sources_truncated":self.origins.len()>MAX_PROVENANCE_ROWS})
    }
}
