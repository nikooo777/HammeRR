use std::sync::Arc;
use std::time::Instant;

use compiler::Control;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use source_formats::bsp::native_terrain::basis::{
    MAX_NATIVE_BASIS_SCRATCH_BYTES, MAX_NATIVE_BASIS_WORK, NativeTerrainBasisPrediction,
    NativeTerrainBasisProfile, predict_native_terrain_basis_controlled,
};
use source_formats::bsp::{Bsp, NativeDisplacementRounding};

use super::{
    Actor, MAX_METADATA_BYTES, NativeTerrainSample, Session, baked_build, vector_report, worker,
};
use crate::runtime::{SharedSession, lock};

pub const MAX_COMPARISON_RETAINED_BYTES: usize = 64 * 1024 * 1024;
const FIELD_NAMES: [&str; 4] = ["position", "normal", "tangent_s", "tangent_t"];
const POLICY: &str = "Predicted final position/N/S/T are computed from the exact retained compiled BSP alone, using the complete installed local and ordered neighbor pipeline under the explicit FP profile and bound current CPU RSQRTSS behavior. Observed arrays are independent comparison targets, never predictor inputs. Profile settings do not establish the historical client's MXCSR or CPU arithmetic. Equality means all raw binary32 bits agree, including signed zeros and nonfinite payloads. Numerical errors are unavailable for nonfinite components. Mismatches remain results and never replace observed fields, framing or guides. No shader pixels, VRAD or later runtime LOD-frame parity is claimed.";

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Rounding {
    #[default]
    NearestEven,
    TowardNegative,
    TowardPositive,
    TowardZero,
}

impl Rounding {
    fn native(self) -> NativeDisplacementRounding {
        match self {
            Self::NearestEven => NativeDisplacementRounding::NearestEven,
            Self::TowardNegative => NativeDisplacementRounding::TowardNegative,
            Self::TowardPositive => NativeDisplacementRounding::TowardPositive,
            Self::TowardZero => NativeDisplacementRounding::TowardZero,
        }
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(default, deny_unknown_fields)]
pub struct ComparisonOptions {
    pub position_rounding: Rounding,
    pub rounding: Rounding,
    pub flush_to_zero: bool,
    pub denormals_are_zero: bool,
}

#[derive(Default)]
pub(super) struct State {
    pending: Option<Pending>,
    ready: Option<Ready>,
    status: &'static str,
    error: Option<String>,
}

struct Pending {
    identity: Arc<()>,
    control: Control,
    options: ComparisonOptions,
}

struct Ready {
    prediction: NativeTerrainBasisPrediction,
    diagnostics: Vec<DisplacementDiagnostics>,
    summary: Summary,
    options: ComparisonOptions,
    profile: String,
    receipt: String,
    elapsed_ms: u64,
    retained_bytes: usize,
}

struct DisplacementDiagnostics {
    samples: Vec<SampleDiagnostic>,
    summary: Summary,
}

struct SampleDiagnostic {
    fields: [VectorDiagnostic; 4],
}

#[derive(Clone, Copy, Default)]
struct VectorDiagnostic {
    max_absolute_error: f64,
    max_ulp_distance: u32,
    equal_mask: u8,
    finite_mask: u8,
}

#[derive(Default)]
struct Summary {
    samples: usize,
    equal_samples: usize,
    fields: [FieldSummary; 4],
}

#[derive(Default)]
struct FieldSummary {
    equal_samples: usize,
    equal_components: usize,
    nonfinite_samples: usize,
    finite_samples: usize,
    max_absolute_error: f64,
    max_ulp_distance: u32,
}

pub struct ComparisonRequest {
    token: u64,
    identity: Arc<()>,
    control: Control,
    observed: Arc<super::Ready>,
    options: ComparisonOptions,
    _worker: worker::Ticket,
}

pub struct ComparisonCompleted {
    request: ComparisonRequest,
    result: Result<Ready, String>,
}

fn fields(sample: NativeTerrainSample) -> [[u32; 3]; 4] {
    [
        sample.position_bits(),
        sample.normal().bits(),
        sample.tangent_s().bits(),
        sample.tangent_t().bits(),
    ]
}

fn ordered_bits(bits: u32) -> u32 {
    if bits & (1 << 31) == 0 {
        bits | (1 << 31)
    } else {
        !bits
    }
}

fn component_error(observed: u32, predicted: u32) -> Option<(f64, u32)> {
    let first = f32::from_bits(observed);
    let second = f32::from_bits(predicted);
    if !first.is_finite() || !second.is_finite() {
        return None;
    }
    Some((
        (f64::from(first) - f64::from(second)).abs(),
        ordered_bits(observed).abs_diff(ordered_bits(predicted)),
    ))
}

fn diagnostic(observed: [u32; 3], predicted: [u32; 3]) -> VectorDiagnostic {
    let mut result = VectorDiagnostic::default();
    for axis in 0..3 {
        if observed[axis] == predicted[axis] {
            result.equal_mask |= 1 << axis;
        }
        if let Some((absolute, ulp)) = component_error(observed[axis], predicted[axis]) {
            result.finite_mask |= 1 << axis;
            result.max_absolute_error = result.max_absolute_error.max(absolute);
            result.max_ulp_distance = result.max_ulp_distance.max(ulp);
        }
    }
    result
}

impl Summary {
    fn push(&mut self, sample: &SampleDiagnostic) {
        self.samples += 1;
        self.equal_samples += usize::from(sample.fields.iter().all(|field| field.equal_mask == 7));
        for (total, field) in self.fields.iter_mut().zip(&sample.fields) {
            total.equal_samples += usize::from(field.equal_mask == 7);
            total.equal_components += field.equal_mask.count_ones() as usize;
            total.nonfinite_samples += usize::from(field.finite_mask != 7);
            if field.finite_mask != 0 {
                total.finite_samples += 1;
                total.max_absolute_error = total.max_absolute_error.max(field.max_absolute_error);
                total.max_ulp_distance = total.max_ulp_distance.max(field.max_ulp_distance);
            }
        }
    }

    fn report(&self) -> Value {
        let fields: serde_json::Map<String, Value> = FIELD_NAMES.iter().zip(&self.fields).map(|(name, field)| {
            ((*name).into(), json!({"equal_samples":field.equal_samples,"mismatched_samples":self.samples-field.equal_samples,
                "equal_components":field.equal_components,"mismatched_components":self.samples*3-field.equal_components,
                "nonfinite_samples":field.nonfinite_samples,
                "max_absolute_error":(field.finite_samples!=0).then_some(field.max_absolute_error),
                "max_ulp_distance":(field.finite_samples!=0).then_some(field.max_ulp_distance)}))
        }).collect();
        json!({"samples":self.samples,"equal_samples":self.equal_samples,"mismatched_samples":self.samples-self.equal_samples,"fields":fields})
    }
}

impl State {
    pub(super) fn running(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn retire(&mut self) {
        if let Some(pending) = self.pending.take() {
            let _ = pending.control.cancel();
        }
        self.ready = None;
        self.status = "not_compared";
        self.error = None;
    }
}

impl ComparisonRequest {
    pub fn token(&self) -> u64 {
        self.token
    }

    pub(crate) fn identity(&self) -> Arc<()> {
        self.identity.clone()
    }

    fn checkpoint(&self, session: &SharedSession) -> Result<(), String> {
        self.control.checkpoint()?;
        let session = lock(session);
        let state = &session.compiled_terrain_snapshot;
        if state.token != Some(self.token)
            || state
                .ready
                .as_ref()
                .is_none_or(|ready| !Arc::ptr_eq(ready, &self.observed))
        {
            return Err("The independent observed terrain snapshot was retired".into());
        }
        self.observed.current(&session)
    }

    pub fn run(self, session: &SharedSession) -> ComparisonCompleted {
        let result = self.execute(session);
        ComparisonCompleted {
            request: self,
            result,
        }
    }

    fn execute(&self, session: &SharedSession) -> Result<Ready, String> {
        let start = Instant::now();
        self.checkpoint(session)?;
        self.observed
            .build
            .verify(&self.control, MAX_METADATA_BYTES)?;
        let bytes =
            baked_build::read_owned_bytes_controlled(&self.observed.build.output, &self.control)?;
        if baked_build::decoded_bsp_bound(&bytes)? > baked_build::MAX_BSP_BYTES as usize {
            return Err("Decoded native-basis BSP exceeds its owned byte budget".into());
        }
        let bsp = Bsp::parse(&bytes).map_err(|error| error.to_string())?;
        let channel = self.observed.snapshot.channel();
        self.observed
            .build
            .origins
            .validate_channel(&bsp, channel)?;
        self.checkpoint(session)?;
        let profile = NativeTerrainBasisProfile::current_cpu(
            self.options.position_rounding.native(),
            self.options.rounding.native(),
            self.options.flush_to_zero,
            self.options.denormals_are_zero,
        )
        .map_err(|error| error.to_string())?;
        let prediction =
            predict_native_terrain_basis_controlled(&bsp, channel, profile, &mut || {
                self.control.checkpoint().map_err(std::io::Error::other)
            })
            .map_err(|error| error.to_string())?;
        drop(bsp);
        drop(bytes);
        self.checkpoint(session)?;
        let predicted = prediction.snapshot();
        let observed = &self.observed.snapshot;
        if predicted.channel() != observed.channel()
            || predicted.bsp_version() != observed.bsp_version()
            || predicted.map_revision() != observed.map_revision()
            || predicted.displacements().len() != observed.displacements().len()
        {
            return Err(
                "Predicted and independently observed terrain catalog identities differ".into(),
            );
        }
        let mut retained_bytes = prediction.retained_bytes();
        let diagnostic_count = observed.displacements().len();
        retained_bytes = retained_bytes
            .checked_add(
                diagnostic_count
                    .checked_mul(std::mem::size_of::<DisplacementDiagnostics>())
                    .ok_or("Comparison catalog size overflow")?,
            )
            .filter(|bytes| *bytes <= MAX_COMPARISON_RETAINED_BYTES)
            .ok_or("Comparison catalog exceeds its retained-result budget")?;
        let mut diagnostics = Vec::new();
        diagnostics
            .try_reserve_exact(diagnostic_count)
            .map_err(|error| error.to_string())?;
        let mut summary = Summary::default();
        for (expected, actual) in observed
            .displacements()
            .iter()
            .zip(predicted.displacements())
        {
            self.control.checkpoint()?;
            if expected.ordinal() != actual.ordinal()
                || expected.face() != actual.face()
                || expected.power() != actual.power()
                || expected.grid_size() != actual.grid_size()
                || expected.samples().len() != actual.samples().len()
            {
                return Err("Predicted and observed displacement/grid identities differ".into());
            }
            let count = expected.samples().len();
            retained_bytes = retained_bytes
                .checked_add(
                    count
                        .checked_mul(std::mem::size_of::<SampleDiagnostic>())
                        .ok_or("Comparison sample size overflow")?,
                )
                .filter(|bytes| *bytes <= MAX_COMPARISON_RETAINED_BYTES)
                .ok_or("Native basis comparison exceeds its retained-result budget")?;
            let mut displacement = DisplacementDiagnostics {
                samples: Vec::new(),
                summary: Summary::default(),
            };
            displacement
                .samples
                .try_reserve_exact(count)
                .map_err(|error| error.to_string())?;
            for (index, (observed, predicted)) in
                expected.samples().iter().zip(actual.samples()).enumerate()
            {
                if index.is_multiple_of(64) {
                    self.control.checkpoint()?;
                }
                let first = fields(*observed);
                let second = fields(*predicted);
                let sample = SampleDiagnostic {
                    fields: std::array::from_fn(|field| diagnostic(first[field], second[field])),
                };
                summary.push(&sample);
                displacement.summary.push(&sample);
                displacement.samples.push(sample);
            }
            diagnostics.push(displacement);
        }
        let profile = prediction.profile();
        let profile=json!({"position_rounding":self.options.position_rounding,"rounding":self.options.rounding,
            "flush_to_zero":self.options.flush_to_zero,"denormals_are_zero":self.options.denormals_are_zero,
            "cpu":{"vendor":profile.cpu.vendor,"brand":profile.cpu.brand,"signature":profile.cpu.signature,
                "rsqrt_probe_bits":profile.cpu.rsqrt_probe_bits.map(|bits|format!("0x{bits:08x}"))},
            "semantics":"Separate binary32 operations; selected MXCSR rounding/FTZ/DAZ; bound host RSQRTSS plus installed Newton step. No inferred historical native MXCSR."}).to_string();
        let receipt = prediction.receipt();
        let receipt = json!({"implementation":receipt.implementation,"engine_sha256":receipt.engine_sha256,
            "stage_crc32":{"local":format!("0x{:08x}",receipt.local_digest),"t_junction":format!("0x{:08x}",receipt.t_junction_digest),
                "corner":format!("0x{:08x}",receipt.corner_digest),"final":format!("0x{:08x}",receipt.final_digest)},
            "displacements":receipt.displacement_count,"samples":receipt.sample_count,"work_units":receipt.work_units,
            "scratch_reserved_bytes":receipt.scratch_reserved_bytes,"retained_bytes":receipt.retained_bytes,
            "t_junction_events":receipt.t_junction_events,"corner_events":receipt.corner_events,
            "edge_pairs":receipt.edge_pairs,"fine_samples":receipt.fine_samples,
            "caller_mxcsr":format!("0x{:08x}",receipt.caller_mxcsr),"selected_mxcsr":format!("0x{:08x}",receipt.selected_mxcsr),
            "digest_semantics":"CRC32 over emitted catalog/index/component position/N/S/T bits; stage checksum, not source identity"}).to_string();
        retained_bytes = prediction
            .retained_bytes()
            .checked_add(
                diagnostics
                    .capacity()
                    .checked_mul(std::mem::size_of::<DisplacementDiagnostics>())
                    .ok_or("Comparison catalog storage overflow")?,
            )
            .and_then(|bytes| {
                diagnostics.iter().try_fold(bytes, |bytes, displacement| {
                    displacement
                        .samples
                        .capacity()
                        .checked_mul(std::mem::size_of::<SampleDiagnostic>())
                        .and_then(|additional| bytes.checked_add(additional))
                })
            })
            .ok_or("Comparison retained storage overflow")?;
        retained_bytes = retained_bytes
            .checked_add(profile.capacity())
            .and_then(|bytes| bytes.checked_add(receipt.capacity()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Ready>()))
            .filter(|bytes| *bytes <= MAX_COMPARISON_RETAINED_BYTES)
            .ok_or("Native basis comparison exceeds its retained-result budget")?;
        self.observed
            .build
            .verify(&self.control, MAX_METADATA_BYTES)?;
        self.checkpoint(session)?;
        Ok(Ready {
            prediction,
            diagnostics,
            summary,
            options: self.options,
            profile,
            receipt,
            elapsed_ms: start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            retained_bytes,
        })
    }
}

impl Session {
    pub fn compiled_terrain_basis_comparison(&self) -> Value {
        let state = &self.compiled_terrain_snapshot;
        let comparison = &state.comparison;
        let current_error = state
            .ready
            .as_ref()
            .and_then(|ready| ready.current(self).err());
        let ready = comparison.ready.as_ref();
        json!({"status":if comparison.status.is_empty(){"not_compared"}else if comparison.status=="ready" && current_error.is_some(){"stale"}else{comparison.status},
            "busy":comparison.pending.is_some(),"cancel_requested":comparison.pending.as_ref().is_some_and(|pending|pending.control.cancelled()),
            "compare_available":state.ready.is_some() && current_error.is_none() && !state.worker.busy() && !self.compiling() && !self.closing && std::ptr::eq(self.render_document(),self.document()),
            "current":ready.is_some() && current_error.is_none(),"current_error":current_error,
            "settings":ready.map(|ready|ready.options).or_else(||comparison.pending.as_ref().map(|pending|pending.options)),
            "defaults":ComparisonOptions::default(),"roundings":[{"id":"nearest_even","label":"Nearest even"},{"id":"toward_negative","label":"Toward negative"},{"id":"toward_positive","label":"Toward positive"},{"id":"toward_zero","label":"Toward zero"}],
            "profile":ready.map(|ready|serde_json::from_str::<Value>(&ready.profile).expect("Retained comparison profile is JSON")),
            "summary":ready.map(|ready|ready.summary.report()),"receipt":ready.map(|ready|serde_json::from_str::<Value>(&ready.receipt).expect("Retained comparison receipt is JSON")),"elapsed_ms":ready.map(|ready|ready.elapsed_ms),
            "retained_bytes":ready.map(|ready|ready.retained_bytes),
            "total_retained_bytes":ready.and_then(|ready|state.ready.as_ref().map(|observed|observed.retained_bytes+ready.retained_bytes)),"error":comparison.error,
            "limits":{"retained_bytes":MAX_COMPARISON_RETAINED_BYTES,"prediction_scratch_bytes":MAX_NATIVE_BASIS_SCRATCH_BYTES,"prediction_work_units":MAX_NATIVE_BASIS_WORK,"results":1},"policy":POLICY})
    }

    pub fn begin_compiled_terrain_basis_comparison(
        &mut self,
        actor: Actor,
        token: u64,
        options: ComparisonOptions,
    ) -> Result<ComparisonRequest, String> {
        self.ensure_not_closing()?;
        self.check_terrain_snapshot_token(token)?;
        if self.compiling() || self.compiled_terrain_snapshot.worker.busy() {
            return Err("Wait for the active build or terrain worker to finish".into());
        }
        if !std::ptr::eq(self.render_document(), self.document()) {
            return Err(
                "Finish the displayed geometry draft before comparing native terrain".into(),
            );
        }
        let observed = self
            .compiled_terrain_snapshot
            .ready
            .as_ref()
            .ok_or("Prepare an independent observed native terrain snapshot first")?
            .clone();
        observed.current(self)?;
        let ticket = self
            .compiled_terrain_snapshot
            .worker
            .begin()
            .ok_or("A terrain worker is already running")?;
        let identity = Arc::new(());
        let control = Control::default();
        let state = &mut self.compiled_terrain_snapshot.comparison;
        state.ready = None;
        state.pending = Some(Pending {
            identity: identity.clone(),
            control: control.clone(),
            options,
        });
        state.status = "comparing";
        state.error = None;
        self.log(
            actor,
            "Started complete offline native terrain basis comparison",
        );
        Ok(ComparisonRequest {
            token,
            identity,
            control,
            observed,
            options,
            _worker: ticket,
        })
    }

    pub fn finish_compiled_terrain_basis_comparison(
        &mut self,
        actor: Actor,
        completed: ComparisonCompleted,
    ) -> Result<Value, String> {
        let ComparisonCompleted { request, result } = completed;
        let state = &self.compiled_terrain_snapshot;
        let owned = state.token == Some(request.token)
            && state
                .ready
                .as_ref()
                .is_some_and(|ready| Arc::ptr_eq(ready, &request.observed))
            && state
                .comparison
                .pending
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(&pending.identity, &request.identity));
        if !owned {
            drop(request);
            return Ok(self.compiled_terrain_snapshot());
        }
        let context = request
            .control
            .checkpoint()
            .and_then(|()| request.observed.current(self));
        let result = match (result, context) {
            (Ok(ready), Ok(())) => Ok(ready),
            (Err(error), Err(context)) => Err(format!("{error}; {context}")),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        };
        let state = &mut self.compiled_terrain_snapshot.comparison;
        state.pending = None;
        match result {
            Ok(ready) => {
                state.ready = Some(ready);
                state.status = "ready";
                state.error = None;
            }
            Err(error) => {
                state.ready = None;
                state.status = if request.control.cancelled() {
                    "cancelled"
                } else {
                    "failed"
                };
                state.error = Some(baked_build::bounded_message(&error));
            }
        }
        let status = state.status;
        self.log(
            actor,
            format!("Offline native terrain basis comparison {status}"),
        );
        drop(request);
        Ok(self.compiled_terrain_snapshot())
    }

    pub fn fail_compiled_terrain_basis_comparison_start(
        &mut self,
        actor: Actor,
        token: u64,
        identity: &Arc<()>,
        error: String,
    ) -> Result<Value, String> {
        if self.compiled_terrain_snapshot.token == Some(token)
            && self
                .compiled_terrain_snapshot
                .comparison
                .pending
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(identity, &pending.identity))
        {
            let state = &mut self.compiled_terrain_snapshot.comparison;
            state.pending = None;
            state.ready = None;
            state.status = "failed";
            state.error = Some(baked_build::bounded_message(&error));
            self.log(
                actor,
                "Offline native terrain basis comparison failed to start",
            );
        }
        Err(error)
    }

    pub fn cancel_compiled_terrain_basis_comparison(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        self.check_terrain_snapshot_token(token)?;
        self.compiled_terrain_snapshot
            .comparison
            .pending
            .as_ref()
            .ok_or("No native terrain basis comparison is running")?
            .control
            .cancel()?;
        self.log(
            actor,
            "Requested offline native terrain basis comparison cancellation",
        );
        Ok(self.compiled_terrain_snapshot())
    }

    pub fn clear_compiled_terrain_basis_comparison(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        self.check_terrain_snapshot_token(token)?;
        self.compiled_terrain_snapshot.comparison.retire();
        self.compiled_terrain_snapshot.comparison.status = "cleared";
        self.log(actor, "Cleared offline native terrain basis comparison");
        Ok(self.compiled_terrain_snapshot())
    }

    pub(super) fn terrain_basis_displacement_comparison(&self, ordinal: u16) -> Option<Value> {
        self.compiled_terrain_snapshot
            .comparison
            .ready
            .as_ref()?
            .diagnostics
            .get(usize::from(ordinal))
            .map(|diagnostics| json!({"summary":diagnostics.summary.report()}))
    }

    pub(super) fn terrain_basis_sample_comparison(
        &self,
        ordinal: u16,
        vertex: usize,
    ) -> Option<Value> {
        let ready = self.compiled_terrain_snapshot.comparison.ready.as_ref()?;
        let predicted = ready
            .prediction
            .snapshot()
            .displacements()
            .get(usize::from(ordinal))?
            .samples()
            .get(vertex)?;
        let observed = self
            .compiled_terrain_snapshot
            .ready
            .as_ref()?
            .sample(ordinal, vertex)
            .ok()?;
        let diagnostic = ready
            .diagnostics
            .get(usize::from(ordinal))?
            .samples
            .get(vertex)?;
        let first = fields(observed);
        let second = fields(*predicted);
        let mut field_reports = serde_json::Map::new();
        let mut predicted_reports = serde_json::Map::new();
        for (field, name) in FIELD_NAMES.iter().enumerate() {
            let diagnostic = diagnostic.fields[field];
            let errors: [Option<(f64, u32)>; 3] = std::array::from_fn(|axis| {
                component_error(first[field][axis], second[field][axis])
            });
            field_reports.insert((*name).into(),json!({"equal":diagnostic.equal_mask==7,
                "component_equal":std::array::from_fn::<_,3,_>(|axis|diagnostic.equal_mask & (1<<axis)!=0),
                "finite_components":std::array::from_fn::<_,3,_>(|axis|diagnostic.finite_mask & (1<<axis)!=0),
                "numerical_error_state":if diagnostic.finite_mask==7{"finite"}else if diagnostic.finite_mask==0{"unavailable"}else{"partial"},
                "absolute_error":errors.map(|error|error.map(|error|error.0)),"ulp_distance":errors.map(|error|error.map(|error|error.1)),
                "max_absolute_error":(diagnostic.finite_mask!=0).then_some(diagnostic.max_absolute_error),"max_ulp_distance":(diagnostic.finite_mask!=0).then_some(diagnostic.max_ulp_distance)}));
            predicted_reports.insert((*name).into(), vector_report(second[field]));
        }
        Some(
            json!({"equal":diagnostic.fields.iter().all(|field|field.equal_mask==7),"predicted":predicted_reports,"fields":field_reports}),
        )
    }
}
