use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError, Weak};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{
    CompileCompletion, CompileSubmission, DocumentId, DocumentOperation, SharedSession, lock,
};
use crate::game;
use crate::session::Actor;
use crate::session::compilation::{CompileOrigin, CompileResult};

const CAPACITY: usize = 4;
static PROCESS_QUEUE: AtomicBool = AtomicBool::new(false);

type Outcome = Result<(CompileResult, Option<game::Log>), String>;

#[cfg(test)]
type BeforeCompletion = Box<dyn FnOnce(&Result<CompileResult, String>) + Send>;

#[derive(Clone)]
pub(crate) struct QueueStatus(Arc<Observation>);

struct Observation {
    admitted: Instant,
    waited: OnceLock<Duration>,
    queued: AtomicBool,
    position: Mutex<Value>,
}

impl QueueStatus {
    fn new() -> Self {
        Self(Arc::new(Observation {
            admitted: Instant::now(),
            waited: OnceLock::new(),
            queued: AtomicBool::new(true),
            position: Mutex::new(json!({"position":null,"admitted":0,"limit":CAPACITY})),
        }))
    }

    pub(crate) fn queued(&self) -> bool {
        self.0.queued.load(Ordering::Acquire)
    }

    fn start(&self) {
        let _ = self.0.waited.set(self.0.admitted.elapsed());
        self.0.queued.store(false, Ordering::Release);
    }

    pub(crate) fn snapshot(&self) -> Value {
        let mut snapshot = self
            .0
            .position
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        snapshot["waiting_ms"] = json!(
            self.0
                .waited
                .get()
                .copied()
                .unwrap_or_else(|| self.0.admitted.elapsed())
                .as_millis()
        );
        snapshot
    }
}

struct Record {
    key: u64,
    document: DocumentId,
    cancelled: bool,
    observation: Option<QueueStatus>,
}

struct Active {
    key: u64,
    document: DocumentId,
    control: compiler::Control,
}

struct State {
    accepting: bool,
    stopping: bool,
    failure: Option<String>,
    serial: u64,
    records: Vec<Record>,
    denied: Vec<DocumentId>,
    queue: VecDeque<Entry>,
    active: Option<Active>,
}

struct Core {
    state: Mutex<State>,
    changed: Condvar,
    #[cfg(test)]
    before_execute: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    before_play: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    fail_play_spawn: AtomicBool,
    #[cfg(test)]
    before_completion: Mutex<Option<BeforeCompletion>>,
}

impl Core {
    fn refresh(state: &State) {
        for record in &state.records {
            if let Some(observation) = &record.observation {
                let position = state
                    .queue
                    .iter()
                    .position(|entry| entry.reservation().key == record.key);
                *observation
                    .0
                    .position
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = json!({
                    "position": position.map(|position| position + 1),
                    "admitted": state.records.len(),
                    "limit": CAPACITY,
                });
            }
        }
    }

    fn fail(&self, error: String) {
        let (entries, active) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.accepting = false;
            state.stopping = true;
            state.failure = Some(error.clone());
            let active = state.active.as_ref().map(|active| active.control.clone());
            (state.queue.drain(..).collect::<Vec<_>>(), active)
        };
        if let Some(control) = active {
            let _ = control.cancel();
        }
        self.changed.notify_all();
        for entry in entries {
            entry.fail(error.clone());
        }
    }
}

pub(crate) struct CompileQueue {
    native: super::native_activity::Authority,
    core: Arc<Core>,
    thread: Option<std::thread::JoinHandle<()>>,
    terminal: Option<Result<(), String>>,
    process: bool,
}

impl CompileQueue {
    pub(crate) fn process() -> Result<Self, String> {
        if PROCESS_QUEUE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("The process compiler queue already has an application owner".into());
        }
        match Self::start(true) {
            Ok(queue) => Ok(queue),
            Err(error) => {
                PROCESS_QUEUE.store(false, Ordering::Release);
                Err(error)
            }
        }
    }

    fn start(process: bool) -> Result<Self, String> {
        let native = if process {
            super::native_activity::Authority::process()?
        } else {
            super::native_activity::Authority::isolated()
        };
        let core = Arc::new(Core {
            state: Mutex::new(State {
                accepting: true,
                stopping: false,
                failure: None,
                serial: 0,
                records: Vec::new(),
                denied: Vec::new(),
                queue: VecDeque::new(),
                active: None,
            }),
            changed: Condvar::new(),
            #[cfg(test)]
            before_execute: Mutex::new(None),
            #[cfg(test)]
            before_play: Mutex::new(None),
            #[cfg(test)]
            fail_play_spawn: AtomicBool::new(false),
            #[cfg(test)]
            before_completion: Mutex::new(None),
        });
        let worker = core.clone();
        let thread = std::thread::Builder::new()
            .name("map-compile-queue".into())
            .spawn(move || worker_main(worker))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            native,
            core,
            thread: Some(thread),
            terminal: None,
            process,
        })
    }

    #[cfg(test)]
    pub(crate) fn isolated() -> Result<Self, String> {
        Self::start(false)
    }

    #[cfg(test)]
    pub(crate) fn before_next_execute(&self, hook: impl FnOnce() + Send + 'static) {
        *self.core.before_execute.lock().unwrap() = Some(Box::new(hook));
    }

    pub(crate) fn client(&self) -> QueueClient {
        QueueClient(self.core.clone())
    }

    pub(crate) fn native(&self) -> &super::native_activity::Authority {
        &self.native
    }

    pub(crate) fn close_admission(&self) {
        self.core
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .accepting = false;
    }

    pub(crate) fn shutdown(&mut self) -> Result<(), String> {
        if let Some(result) = &self.terminal {
            return result.clone();
        }
        let (entries, active) = {
            let mut state = self
                .core
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.accepting = false;
            state.stopping = true;
            for record in &mut state.records {
                record.cancelled = true;
            }
            let active = state.active.as_ref().map(|active| active.control.clone());
            (state.queue.drain(..).collect::<Vec<_>>(), active)
        };
        if let Some(control) = active {
            let _ = control.cancel();
        }
        self.core.changed.notify_all();
        for entry in entries {
            entry.cancel();
        }
        let joined = self.thread.take().map_or(Ok(()), |thread| {
            thread
                .join()
                .map_err(|_| "Compiler queue worker panicked".to_owned())
        });
        let result = joined.and_then(|()| {
            let state = self
                .core
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.failure.clone().map_or(Ok(()), Err)
        });
        self.terminal = Some(result.clone());
        result
    }
}

impl Drop for CompileQueue {
    fn drop(&mut self) {
        if self.terminal.is_none()
            && let Err(error) = self.shutdown()
        {
            eprintln!("Compiler queue shutdown failed: {error}");
        }
        if self.process {
            PROCESS_QUEUE.store(false, Ordering::Release);
        }
    }
}

#[derive(Clone)]
pub(crate) struct QueueClient(Arc<Core>);

pub(crate) struct CompileInput<'a> {
    pub actor: Actor,
    pub path: Option<&'a std::path::Path>,
    pub play: bool,
    pub generation: Option<u64>,
}

struct Reservation {
    core: Weak<Core>,
    key: u64,
    document: DocumentId,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(core) = self.core.upgrade() {
            let mut state = core.state.lock().unwrap_or_else(PoisonError::into_inner);
            let removed = state
                .records
                .iter()
                .position(|record| record.key == self.key)
                .map(|index| state.records.remove(index));
            if let Some(observation) = removed.and_then(|record| record.observation) {
                observation.start();
                *observation
                    .0
                    .position
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) =
                    json!({"position":null,"admitted":state.records.len(),"limit":CAPACITY});
            }
            if state
                .active
                .as_ref()
                .is_some_and(|active| active.key == self.key)
            {
                state.active = None;
            }
            Core::refresh(&state);
            drop(state);
            core.changed.notify_all();
        }
    }
}

impl QueueClient {
    fn reserve(&self, document: DocumentId) -> Result<Reservation, String> {
        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.accepting || state.denied.contains(&document) {
            return Err(state
                .failure
                .clone()
                .unwrap_or_else(|| "The compiler queue is closing".into()));
        }
        if state
            .records
            .iter()
            .any(|record| record.document == document)
        {
            return Err("This document already has an admitted compile job".into());
        }
        if state.records.len() == CAPACITY {
            return Err("The compiler queue is full (4 admitted jobs)".into());
        }
        state.serial = state
            .serial
            .checked_add(1)
            .ok_or("Compiler queue identity exhausted")?;
        let key = state.serial;
        state.records.push(Record {
            key,
            document,
            cancelled: false,
            observation: None,
        });
        Core::refresh(&state);
        Ok(Reservation {
            core: Arc::downgrade(&self.0),
            key,
            document,
        })
    }

    pub(crate) fn submit(
        &self,
        document: DocumentId,
        session: &SharedSession,
        operation: DocumentOperation,
        input: CompileInput<'_>,
    ) -> Result<CompileHandle, String> {
        self.submit_with(document, session, operation, input, || {})
    }

    fn submit_with(
        &self,
        document: DocumentId,
        session: &SharedSession,
        operation: DocumentOperation,
        input: CompileInput<'_>,
        before_run: impl FnOnce() + Send + 'static,
    ) -> Result<CompileHandle, String> {
        let reservation = self.reserve(document)?;
        let mut submission = CompileSubmission::begin(
            session,
            input.actor,
            input.path,
            input.play,
            input.generation,
        )?;
        submission._operation = Some(operation);
        self.enqueue(reservation, submission, before_run)
    }

    fn enqueue(
        &self,
        reservation: Reservation,
        mut submission: CompileSubmission,
        before_run: impl FnOnce() + Send + 'static,
    ) -> Result<CompileHandle, String> {
        let session = &submission.completion.session;
        let observation = QueueStatus::new();
        lock(session).queue_compile(&submission.completion.origin, observation.clone())?;
        let mut status = lock(session).compile_status();
        let result = Arc::new(ResultCell::default());
        let entry = Entry {
            control: submission.request.control(),
            guard: CompletionGuard {
                result: result.clone(),
                pending: Some(PendingCompletion {
                    completion: submission.completion(),
                    operation: submission._operation.take(),
                    reservation: Some(reservation),
                    launch: None,
                }),
                #[cfg(test)]
                before_failure: None,
            },
            submission,
            observation: observation.clone(),
            before_run: Box::new(before_run),
        };
        let rejected = {
            let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(record) = state
                .records
                .iter_mut()
                .find(|record| record.key == entry.reservation().key)
            {
                record.observation = Some(observation.clone());
            }
            let rejection = if let Some(error) = &state.failure {
                Some((error.clone(), true))
            } else if !state.accepting
                || state
                    .records
                    .iter()
                    .find(|record| record.key == entry.reservation().key)
                    .is_none_or(|record| record.cancelled)
            {
                Some(("The compiler queue closed during admission".into(), false))
            } else if entry.control.cancelled() {
                Some(("Compile cancelled".into(), false))
            } else {
                None
            };
            if let Some((error, failed)) = rejection {
                Some((entry, error, failed))
            } else {
                state.queue.push_back(entry);
                None
            }
        };
        if let Some((entry, error, failed)) = rejected {
            observation.start();
            if failed {
                entry.fail(error.clone());
            } else {
                entry.cancel();
            }
            return Err(error);
        }
        Core::refresh(&self.0.state.lock().unwrap_or_else(PoisonError::into_inner));
        self.0.changed.notify_one();
        status["job"]["queue"] = observation.snapshot();
        Ok(CompileHandle { status, result })
    }

    pub(crate) fn cancel_queued(&self, document: DocumentId, origin: &CompileOrigin) {
        let entry = {
            let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
            let index = state.queue.iter().position(|entry| {
                entry.reservation().document == document
                    && entry.submission.completion.origin.same(origin)
            });
            index.and_then(|index| state.queue.remove(index))
        };
        Core::refresh(&self.0.state.lock().unwrap_or_else(PoisonError::into_inner));
        if let Some(entry) = entry {
            entry.cancel();
        }
    }

    pub(crate) fn cancel_document(&self, document: DocumentId) {
        let (entries, active) = {
            let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
            if !state.denied.contains(&document) {
                state.denied.push(document);
            }
            for record in &mut state.records {
                if record.document == document {
                    record.cancelled = true;
                }
            }
            let mut entries = Vec::new();
            let mut retained = VecDeque::new();
            while let Some(entry) = state.queue.pop_front() {
                if entry.reservation().document == document {
                    entries.push(entry);
                } else {
                    retained.push_back(entry);
                }
            }
            state.queue = retained;
            let active = state
                .active
                .as_ref()
                .filter(|active| active.document == document)
                .map(|active| active.control.clone());
            (entries, active)
        };
        Core::refresh(&self.0.state.lock().unwrap_or_else(PoisonError::into_inner));
        if let Some(control) = active {
            let _ = control.cancel();
        }
        for entry in entries {
            entry.cancel();
        }
    }
}

struct Entry {
    // Request retirement must precede the fallback completion guard.
    submission: CompileSubmission,
    guard: CompletionGuard,
    control: compiler::Control,
    observation: QueueStatus,
    before_run: Box<dyn FnOnce() + Send>,
}

impl Entry {
    fn reservation(&self) -> &Reservation {
        self.guard
            .pending
            .as_ref()
            .expect("queued completion exists")
            .reservation
            .as_ref()
            .expect("queued compiler reservation exists")
    }

    fn cancel(self) {
        let _ = self.control.cancel();
        self.execute();
    }

    fn fail(self, error: String) {
        let Self {
            submission,
            mut guard,
            ..
        } = self;
        drop(submission);
        guard.fail(error);
    }

    fn execute(self) {
        let Self {
            submission,
            control,
            mut guard,
            before_run,
            ..
        } = self;
        #[cfg(test)]
        let before_completion = {
            let core = guard
                .pending
                .as_ref()
                .unwrap()
                .reservation
                .as_ref()
                .unwrap()
                .core
                .upgrade()
                .unwrap();
            core.before_completion.lock().unwrap().take()
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if !control.cancelled() {
                before_run();
            }
            #[cfg(test)]
            {
                submission.prepare_with(|completion, result, native| {
                    if let Some(before_completion) = before_completion {
                        before_completion(result);
                    }
                    completion.prepare(result, native)
                })
            }
            #[cfg(not(test))]
            submission.prepare()
        }));
        match outcome {
            Ok(Ok(prepared)) if prepared.launch.is_some() => {
                #[cfg(test)]
                let (before_play, fail_play_spawn) = {
                    let core = guard
                        .pending
                        .as_ref()
                        .unwrap()
                        .reservation
                        .as_ref()
                        .unwrap()
                        .core
                        .upgrade()
                        .unwrap();
                    let before_play = core.before_play.lock().unwrap().take();
                    (
                        before_play,
                        core.fail_play_spawn.swap(false, Ordering::AcqRel),
                    )
                };
                guard.prepare_play(&prepared);
                let owner = Arc::new(PlayOwner {
                    guard: Mutex::new(Some(guard)),
                });
                let task = PlayTask {
                    prepared,
                    owner: owner.clone(),
                    #[cfg(test)]
                    before_play,
                };
                if let Err(error) = spawn_play(
                    task,
                    #[cfg(test)]
                    fail_play_spawn,
                ) && let Some(mut guard) = owner
                    .guard
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                {
                    guard.fail(format!("Could not schedule optional Play startup: {error}"));
                }
            }
            Ok(Ok(prepared)) => guard.complete(prepared.run()),
            Ok(Err(error)) => guard.complete(Err(error)),
            Err(_) => guard.fail("Compiler queue submission panicked".into()),
        }
    }
}

struct PlayOwner {
    guard: Mutex<Option<CompletionGuard>>,
}

struct PlayTask {
    prepared: super::PreparedCompile,
    owner: Arc<PlayOwner>,
    #[cfg(test)]
    before_play: Option<Box<dyn FnOnce() + Send>>,
}

fn spawn_play(
    task: PlayTask,
    #[cfg(test)] fail: bool,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    #[cfg(test)]
    if fail {
        drop(task);
        return Err(std::io::Error::other("injected Play scheduling failure"));
    }
    std::thread::Builder::new()
        .name("compile-play-startup".into())
        .spawn(move || run_play(task))
}

fn run_play(task: PlayTask) {
    let PlayTask {
        prepared,
        owner,
        #[cfg(test)]
        before_play,
    } = task;
    let mut guard = owner
        .guard
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .expect("Play continuation owns its completion");
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        if let Some(before_play) = before_play {
            before_play();
        }
        prepared.run()
    }));
    match outcome {
        Ok(outcome) => guard.complete(outcome),
        Err(_) => guard.fail("Optional Play startup worker panicked".into()),
    }
}

struct PendingCompletion {
    completion: CompileCompletion,
    operation: Option<DocumentOperation>,
    reservation: Option<Reservation>,
    launch: Option<super::LaunchCompletion>,
}

struct CompletionGuard {
    result: Arc<ResultCell>,
    pending: Option<PendingCompletion>,
    #[cfg(test)]
    before_failure: Option<Box<dyn FnOnce() + Send>>,
}

impl CompletionGuard {
    fn prepare_play(&mut self, prepared: &super::PreparedCompile) {
        let pending = self
            .pending
            .as_mut()
            .expect("Play continuation owns its completion");
        pending.launch = prepared
            .launch
            .as_ref()
            .map(super::LaunchSubmission::completion);
        drop(pending.reservation.take());
    }

    fn fail(&mut self, error: String) {
        #[cfg(test)]
        if let Some(before_failure) = self.before_failure.take() {
            before_failure();
        }
        if let Some(pending) = &self.pending {
            if let Some(launch) = &pending.launch {
                launch.fail(error.clone());
            } else {
                pending.completion.fail(error.clone());
            }
        }
        self.complete(Err(error));
    }

    fn complete(&mut self, outcome: Outcome) {
        if let Some(PendingCompletion {
            operation,
            reservation,
            ..
        }) = self.pending.take()
        {
            drop(operation);
            drop(reservation);
            self.result.complete(outcome);
        }
    }
}

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        if self.pending.is_some() {
            self.fail("Compiler queue worker panicked".into());
        }
    }
}

#[derive(Default)]
struct ResultState {
    value: Option<Outcome>,
    waker: Option<Waker>,
}

#[derive(Default)]
struct ResultCell {
    state: Mutex<ResultState>,
    ready: Condvar,
}

impl ResultCell {
    fn complete(&self, value: Outcome) {
        let waker = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.value = Some(value);
            state.waker.take()
        };
        self.ready.notify_all();
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

pub struct CompileHandle {
    pub(crate) status: Value,
    result: Arc<ResultCell>,
}

impl CompileHandle {
    pub fn status(&self) -> &Value {
        &self.status
    }
}

impl Future for CompileHandle {
    type Output = Result<(CompileResult, Option<game::Log>), String>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Outcome> {
        let mut state = self
            .result
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(value) = state.value.take() {
            Poll::Ready(value)
        } else {
            let previous = state.waker.replace(context.waker().clone());
            drop(state);
            drop(previous);
            Poll::Pending
        }
    }
}

impl Drop for CompileHandle {
    fn drop(&mut self) {
        let waker = self
            .result
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .waker
            .take();
        drop(waker);
    }
}

fn worker_main(core: Arc<Core>) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker_loop(&core))).is_err() {
        core.fail("Compiler queue worker panicked".into());
    }
}

fn worker_loop(core: &Core) {
    loop {
        let entry = {
            let mut state = core.state.lock().unwrap_or_else(PoisonError::into_inner);
            while state.queue.is_empty() && !state.stopping {
                state = core
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            let Some(entry) = state.queue.pop_front() else {
                return;
            };
            state.active = Some(Active {
                key: entry.reservation().key,
                document: entry.reservation().document,
                control: entry.control.clone(),
            });
            entry
        };
        entry.observation.start();
        Core::refresh(&core.state.lock().unwrap_or_else(PoisonError::into_inner));
        #[cfg(test)]
        let before_execute = core.before_execute.lock().unwrap().take();
        #[cfg(test)]
        if let Some(before_execute) = before_execute {
            before_execute();
        }
        entry.execute();
    }
}

#[cfg(test)]
mod tests;
