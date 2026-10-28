use std::path::{Path, PathBuf};
use std::sync::Arc;

use compiler::{Control, FileIdentity};
use map_core::{Document, ObjectId, comparison};
use serde_json::{Value, json};

use super::worker::{Slot, Ticket};
use super::{Actor, SelectMode, Session};

pub const MAX_PAGE: usize = 64;
const MAX_PARSE_DEPTH: usize = 64;
const MAX_PARSE_NODES: usize = 65536;
const MAX_SEARCH: usize = 512;
const MAX_REPORT: usize = 1024 * 1024;
const SCOPE: &str = "Committed root-world brushes and whole root entities, including saved hidden content. Keys are object kind and authored ID. Canonical typed VMF node equality counts IDs, editor fields, hidden flags, ordered properties/connections/brushes/sides, opaque blocks and effective entity child layout. Existing typed parsing/writing normalizes numeric spellings and signed zero. Root object order, map settings/world properties/groups/cameras, expanded instances and unapplied drafts are excluded. No geometric equivalence or merge. Actions use all filtered eligible current objects across pages. Captured rows are verified for each explicit request, not continuously watched.";

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    form: Option<Form>,
    pub(super) worker: Slot,
}

struct Form {
    token: u64,
    owner: Actor,
    identity: Arc<()>,
    generation: u64,
    revision: u64,
    control: Control,
    file: String,
    status: &'static str,
    error: Option<String>,
    ready: Option<Arc<Ready>>,
    page: Option<Value>,
}

impl Drop for Form {
    fn drop(&mut self) {
        let _ = self.control.cancel();
    }
}

struct Ready {
    identity: FileIdentity,
    rows: Vec<comparison::Row>,
}
struct Verified {
    ready: Arc<Ready>,
    filtered: Vec<usize>,
}

struct SelectionIntent {
    objects: Vec<ObjectId>,
    face_mode: bool,
    faces: Vec<map_core::surface::FaceId>,
    components_serial: u64,
}

impl SelectionIntent {
    fn capture(session: &Session) -> Self {
        Self {
            objects: session.selected_ids(),
            face_mode: session.face_mode,
            faces: session.selected_faces(),
            components_serial: session.geometry.components.serial,
        }
    }
}

#[derive(Clone, Default)]
pub struct Filter {
    pub status: Option<comparison::Status>,
    pub search: String,
}

#[derive(Clone)]
pub enum Action {
    Page,
    Select(SelectMode),
    Frame(f32),
}

#[derive(Clone)]
pub struct Query {
    pub filter: Filter,
    pub offset: usize,
    pub limit: usize,
    pub action: Action,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            filter: Filter::default(),
            offset: 0,
            limit: MAX_PAGE,
            action: Action::Page,
        }
    }
}

enum Work {
    Prepare {
        document: Box<Document>,
        file: PathBuf,
        budget: comparison::Work,
    },
    Verify(Arc<Ready>),
}

pub struct Request {
    token: u64,
    identity: Arc<()>,
    generation: u64,
    revision: u64,
    control: Control,
    query: Query,
    expected_camera: Option<render::Camera>,
    expected_selection: Option<SelectionIntent>,
    work: Work,
    _worker: Ticket,
}

pub struct Completed {
    token: u64,
    identity: Arc<()>,
    generation: u64,
    revision: u64,
    control: Control,
    query: Query,
    expected_camera: Option<render::Camera>,
    expected_selection: Option<SelectionIntent>,
    result: Result<Verified, String>,
    _worker: Ticket,
}

impl Request {
    pub fn token(&self) -> u64 {
        self.token
    }
    pub fn cancellation(&self) -> Control {
        self.control.clone()
    }
    pub fn run(self) -> Completed {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.control.checkpoint()?;
            let (ready, mut budget) = match self.work {
                Work::Prepare {
                    document,
                    file,
                    mut budget,
                } => {
                    let (identity, bytes) = FileIdentity::read_bounded(
                        &file,
                        comparison::MAX_INPUT_BYTES as u64,
                        &self.control,
                    )
                    .map_err(|error| error.to_string())?;
                    identity
                        .path()
                        .to_str()
                        .ok_or("Comparison source resolves through a non-UTF-8 path")?;
                    let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
                    self.control.checkpoint()?;
                    let parsed = source_formats::keyvalues::parse_bounded(
                        text,
                        MAX_PARSE_DEPTH,
                        MAX_PARSE_NODES,
                    )
                    .map_err(|error| error.to_string())?;
                    self.control.checkpoint()?;
                    comparison::admit_parsed(&parsed, &mut budget, &mut || {
                        self.control.checkpoint()
                    })?;
                    let external = source_formats::vmf::Vmf::from_nodes(&parsed)
                        .map_err(|error| error.to_string())?;
                    drop(parsed);
                    drop(bytes);
                    self.control.checkpoint()?;
                    let rows =
                        comparison::compare(document.vmf(), &external, &mut budget, &mut || {
                            self.control.checkpoint()
                        })?;
                    (Arc::new(Ready { identity, rows }), budget)
                }
                Work::Verify(ready) => (ready, comparison::Work::default()),
            };
            let needle = self.query.filter.search.to_lowercase();
            let mut filtered = Vec::new();
            for (index, row) in ready.rows.iter().enumerate() {
                budget.checkpoint(&mut || self.control.checkpoint())?;
                for _ in 0..(row.label.len() + row.external_label.len()).div_ceil(64) {
                    budget.checkpoint(&mut || self.control.checkpoint())?;
                }
                if matches(row, &self.query.filter, &needle) {
                    filtered.push(index);
                }
            }
            ready
                .identity
                .verify(&self.control)
                .map_err(|error| error.to_string())?;
            self.control.checkpoint()?;
            Ok(Verified { ready, filtered })
        }))
        .unwrap_or_else(|_| Err("External map comparison worker panicked".into()));
        Completed {
            token: self.token,
            identity: self.identity,
            generation: self.generation,
            revision: self.revision,
            control: self.control,
            query: self.query,
            expected_camera: self.expected_camera,
            expected_selection: self.expected_selection,
            result,
            _worker: self._worker,
        }
    }
}

fn validate(query: &Query) -> Result<(), String> {
    if query.filter.search.len() > MAX_SEARCH || !(1..=MAX_PAGE).contains(&query.limit) {
        return Err("Comparison search supports 512 bytes and pages 1–64 rows".into());
    }
    if let Action::Frame(aspect) = query.action
        && (!aspect.is_finite() || !(0.01..=100.0).contains(&aspect))
    {
        return Err("Comparison framing aspect must be finite and between 0.01 and 100".into());
    }
    Ok(())
}

fn matches(row: &comparison::Row, filter: &Filter, needle: &str) -> bool {
    if filter.status.is_some_and(|status| row.status != status) {
        return false;
    }
    row.label.to_lowercase().contains(needle)
        || row.external_label.to_lowercase().contains(needle)
        || crate::identity::id_string(row.object).contains(needle)
}

impl Session {
    pub(super) fn clear_external_comparison(&mut self) {
        if let Some(form) = self.external_comparison.form.take() {
            let _ = form.control.cancel();
        }
        self.external_comparison.serial = self.external_comparison.serial.wrapping_add(1);
    }

    fn invalidate_external_comparison(&mut self) {
        if let Some(form) = &mut self.external_comparison.form
            && (form.generation != self.document_generation
                || form.revision != self.document.revision())
        {
            let _ = form.control.cancel();
            form.ready = None;
            form.page = None;
            form.status = "stale";
            form.error = Some("Current map changed; compare again".into());
        } else if let Some(form) = &mut self.external_comparison.form
            && form.control.cancelled()
        {
            form.ready = None;
            form.page = None;
            form.status = "cancelled";
            form.error = Some("Comparison cancelled; compare again".into());
        }
    }

    pub fn external_comparison_state(&mut self) -> Value {
        self.invalidate_external_comparison();
        match &self.external_comparison.form {
            Some(form) => {
                json!({"token":form.token,"owner":form.owner.label(),"file":form.file,"status":form.status,
                "busy":self.external_comparison.worker.busy(),"error":form.error,"page":form.page,"scope":SCOPE})
            }
            None => {
                json!({"token":null,"busy":self.external_comparison.worker.busy(),"status":"closed","scope":SCOPE})
            }
        }
    }

    pub fn begin_external_comparison(
        &mut self,
        actor: Actor,
        file: &Path,
    ) -> Result<Request, String> {
        self.ensure_not_closing()?;
        let file_text = file
            .to_str()
            .filter(|text| text.len() <= 4096)
            .ok_or("Choose a comparison VMF through a UTF-8 path within 4096 bytes")?;
        if !file
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("vmf"))
        {
            return Err("Choose a .vmf comparison file".into());
        }
        if self
            .external_comparison
            .form
            .as_ref()
            .is_some_and(|form| form.owner != actor)
        {
            return Err("Review the shared comparison before replacing it".into());
        }
        if self.external_comparison.worker.busy() {
            return Err("Wait for the previous comparison worker to drain".into());
        }
        let mut budget = comparison::Work::default();
        comparison::admit(self.document.vmf(), &mut budget, &mut || Ok(()))?;
        let worker = self
            .external_comparison
            .worker
            .begin()
            .ok_or("Wait for the previous comparison worker to drain")?;
        self.clear_external_comparison();
        let token = self.external_comparison.serial;
        let control = Control::default();
        let identity = Arc::new(());
        self.external_comparison.form = Some(Form {
            token,
            owner: actor,
            identity: identity.clone(),
            generation: self.document_generation,
            revision: self.document.revision(),
            control: control.clone(),
            file: file_text.into(),
            status: "preparing",
            error: None,
            ready: None,
            page: None,
        });
        Ok(Request {
            token,
            identity,
            generation: self.document_generation,
            revision: self.document.revision(),
            control,
            query: Query::default(),
            expected_camera: None,
            expected_selection: None,
            work: Work::Prepare {
                document: Box::new(self.document.preview_snapshot_uncached()),
                file: file.to_owned(),
                budget,
            },
            _worker: worker,
        })
    }

    pub fn query_external_comparison(
        &mut self,
        actor: Actor,
        token: u64,
        query: Query,
    ) -> Result<Request, String> {
        self.ensure_not_closing()?;
        validate(&query)?;
        let expected_camera = matches!(query.action, Action::Frame(_)).then_some(self.camera);
        let expected_selection =
            matches!(query.action, Action::Select(_)).then(|| SelectionIntent::capture(self));
        self.invalidate_external_comparison();
        let form = self
            .external_comparison
            .form
            .as_mut()
            .filter(|form| form.token == token)
            .ok_or("Comparison request expired")?;
        if form.owner != actor {
            return Err("Review the shared comparison before acting".into());
        }
        let ready = form
            .ready
            .clone()
            .ok_or("Compare the current map with the chosen VMF first")?;
        let worker = self
            .external_comparison
            .worker
            .begin()
            .ok_or("Wait for the current comparison request")?;
        form.page = None;
        form.status = "verifying";
        form.error = None;
        Ok(Request {
            token,
            identity: form.identity.clone(),
            generation: form.generation,
            revision: form.revision,
            control: form.control.clone(),
            query,
            expected_camera,
            expected_selection,
            work: Work::Verify(ready),
            _worker: worker,
        })
    }

    pub fn review_external_comparison(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.invalidate_external_comparison();
        if self.external_comparison.worker.busy() {
            return Err("Wait for the comparison worker before reviewing ownership".into());
        }
        let form = self
            .external_comparison
            .form
            .as_mut()
            .filter(|form| form.token == token)
            .ok_or("Comparison request expired")?;
        self.external_comparison.serial = self.external_comparison.serial.wrapping_add(1);
        form.token = self.external_comparison.serial;
        form.owner = actor;
        form.identity = Arc::new(());
        let _ = form.control.cancel();
        form.control = Control::default();
        form.page = None;
        Ok(self.external_comparison_state())
    }

    pub fn cancel_external_comparison(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        let form = self
            .external_comparison
            .form
            .as_ref()
            .filter(|form| form.token == token)
            .ok_or("Comparison request expired")?;
        if form.owner != actor {
            return Err("Only the comparison owner can cancel it".into());
        }
        self.clear_external_comparison();
        Ok(self.external_comparison_state())
    }

    pub fn finish_external_comparison(
        &mut self,
        actor: Actor,
        completed: Completed,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.invalidate_external_comparison();
        let form = self
            .external_comparison
            .form
            .as_mut()
            .filter(|form| {
                form.token == completed.token
                    && Arc::ptr_eq(&form.identity, &completed.identity)
                    && form.owner == actor
            })
            .ok_or("Comparison completion expired or belongs to another owner")?;
        if completed.generation != self.document_generation
            || completed.revision != self.document.revision()
        {
            return Err("Current map changed; compare again".into());
        }
        let result = completed.result.and_then(|verified| {
            verified
                .ready
                .identity
                .verify_metadata()
                .map_err(|error| error.to_string())?;
            completed.control.checkpoint().map(|()| verified)
        });
        let verified = match result {
            Ok(verified) => verified,
            Err(error) => {
                form.ready = None;
                form.page = None;
                form.status = "failed";
                form.error = Some(error.clone());
                return Err(error);
            }
        };
        if completed
            .expected_camera
            .is_some_and(|camera| camera != self.camera)
        {
            let error = "Camera changed while comparison framing was being verified; frame again"
                .to_owned();
            form.page = None;
            form.status = "captured";
            form.ready = Some(verified.ready);
            form.error = Some(error.clone());
            return Err(error);
        }
        if completed
            .expected_selection
            .as_ref()
            .is_some_and(|expected| {
                !expected
                    .objects
                    .iter()
                    .copied()
                    .eq(self.selection.iter().copied())
                    || expected.face_mode != self.face_mode
                    || !expected
                        .faces
                        .iter()
                        .copied()
                        .eq(self.face_selection.iter().copied())
                    || expected.components_serial != self.geometry.components.serial
            })
        {
            let error =
                "Selection changed while comparison selection was being verified; select again"
                    .to_owned();
            form.page = None;
            form.status = "captured";
            form.ready = Some(verified.ready);
            form.error = Some(error.clone());
            return Err(error);
        }
        let ready = verified.ready;
        let filtered = verified
            .filtered
            .iter()
            .map(|&index| &ready.rows[index])
            .collect::<Vec<_>>();
        if completed.query.offset > filtered.len() {
            form.page = None;
            form.status = "captured";
            form.ready = Some(ready);
            form.error = Some("Comparison page offset exceeds the filtered result count".into());
            return Err("Comparison page offset exceeds the filtered result count".into());
        }
        let ids = filtered
            .iter()
            .filter(|row| row.current_count == 1 && row.status != comparison::Status::Ambiguous)
            .map(|row| row.object)
            .collect::<Vec<_>>();
        let end = completed
            .query
            .offset
            .saturating_add(completed.query.limit)
            .min(filtered.len());
        let rows=filtered[completed.query.offset..end].iter().map(|row|json!({"id":crate::identity::id_string(row.object),"status":row.status.name(),
            "label":row.label,"external_label":row.external_label,"current_count":row.current_count,"external_count":row.external_count})).collect::<Vec<_>>();
        let page = json!({"token":form.token,"file":form.file,"source_sha256":ready.identity.sha256(),"generation":form.generation,"revision":form.revision,
            "total":ready.rows.len(),"filtered":filtered.len(),"current_matches":ids.len(),"offset":completed.query.offset,"next_offset":(end<filtered.len()).then_some(end),"rows":rows,
            "limits":{"raw_file_bytes":comparison::MAX_INPUT_BYTES,"objects_per_map":comparison::MAX_OBJECTS,"sides_per_map":comparison::MAX_SIDES,
                "parse_depth":MAX_PARSE_DEPTH,"parse_nodes":MAX_PARSE_NODES,"stored_value_tokens":comparison::MAX_VALUE_TOKENS,"typed_node_bytes_per_map":comparison::MAX_INPUT_BYTES,"comparison_work":comparison::MAX_WORK,
                "page_rows":MAX_PAGE,"search_bytes":MAX_SEARCH,"label_characters":comparison::MAX_LABEL},
            "status_filter":completed.query.filter.status.map(comparison::Status::name),"search":completed.query.filter.search,
            "scope":SCOPE,"verification":"Original external file bytes rechecked for this request; not continuously watched"});
        if serde_json::to_vec(&page)
            .map_err(|error| error.to_string())?
            .len()
            > MAX_REPORT
        {
            form.ready = None;
            form.page = None;
            form.status = "failed";
            form.error = Some("Comparison page exceeds 1 MiB".into());
            return Err("Comparison page exceeds 1 MiB".into());
        }
        if let Err(error) = completed.control.checkpoint() {
            form.ready = None;
            form.page = None;
            form.status = "cancelled";
            form.error = Some(error.clone());
            return Err(error);
        }
        form.ready = Some(ready);
        form.page = Some(page.clone());
        form.status = "captured";
        form.error = None;
        let action: Result<(), String> = (|| {
            completed.control.checkpoint()?;
            match completed.query.action {
                Action::Page => {}
                Action::Select(mode) => {
                    self.comparison_action_context()?;
                    if ids.is_empty() {
                        return Err("No unambiguous current objects match these filters".into());
                    }
                    completed.control.checkpoint()?;
                    self.select(&ids, mode)?;
                    self.log(actor, "selected current comparison matches");
                }
                Action::Frame(aspect) => {
                    self.comparison_action_context()?;
                    self.frame_comparison_matches(actor, &ids, aspect, &completed.control)?;
                }
            }
            Ok(())
        })();
        if let Err(error) = action {
            if let Some(form) = &mut self.external_comparison.form {
                form.error = Some(error.clone());
            }
            return Err(error);
        }
        Ok(page)
    }

    fn comparison_action_context(&self) -> Result<(), String> {
        if self.viewport_pick_conflict()
            || self.measurement_picking()
            || self.linked_path_picking()
            || self.block_preview.is_some()
            || !std::ptr::eq(self.render_document(), &self.document)
        {
            return Err(
                "Finish or cancel geometry drafts and viewport gestures before comparison actions"
                    .into(),
            );
        }
        Ok(())
    }

    fn frame_comparison_matches(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
        aspect: f32,
        control: &Control,
    ) -> Result<(), String> {
        if ids.is_empty() {
            return Err("No unambiguous current objects match these filters".into());
        }
        let presentation = self.presentation_bounds_in(&self.document, None);
        let mut bounds = Vec::with_capacity(ids.len());
        for &id in ids {
            control.checkpoint()?;
            let (lo, hi) = super::models::object_bounds(&self.document, id, &presentation)
                .ok_or_else(|| format!("Comparison object {id} has no framing bounds"))?;
            if !lo.is_finite() || !hi.is_finite() || !lo.cmple(hi).all() {
                return Err(format!("Comparison object {id} has invalid framing bounds"));
            }
            bounds.push((lo, hi));
        }
        let (lo, hi) =
            map_core::query::union(bounds).ok_or("Comparison objects have no framing bounds")?;
        let camera = self.camera.frame_bounds(lo.as_vec3(), hi.as_vec3(), aspect);
        if !camera.position.is_finite() || !camera.view_projection(aspect).is_finite() {
            return Err("Comparison objects have no finite framing camera".into());
        }
        control.checkpoint()?;
        self.set_camera(camera);
        self.log(actor, "framed current comparison matches");
        Ok(())
    }
}
