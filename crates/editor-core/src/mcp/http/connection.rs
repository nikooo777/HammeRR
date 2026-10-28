use std::error::Error;
use std::sync::{Arc, Mutex};

use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{oneshot, watch};

const INTERRUPTED: &str = "MCP HTTP connection was interrupted before completion";

#[derive(Clone, Debug)]
/// Observes one Hyper connection. Lifecycle response ordering also requires a result-body witness.
pub struct ConnectionReceipt {
    result: watch::Receiver<Option<Result<(), String>>>,
    graceful: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl ConnectionReceipt {
    pub fn result(&self) -> Option<Result<(), String>> {
        self.result.borrow().clone()
    }

    pub async fn wait(&self) -> Result<(), String> {
        let mut result = self.result.clone();
        loop {
            let finished = result.borrow().clone();
            if let Some(finished) = finished {
                return finished;
            }
            if result.changed().await.is_err() {
                return Err(INTERRUPTED.into());
            }
        }
    }

    pub fn request_graceful(&self) {
        let request = self
            .graceful
            .lock()
            .expect("HTTP graceful request lock poisoned")
            .take();
        if let Some(request) = request {
            let _ = request.send(());
        }
    }

    fn new() -> (Self, Completion, oneshot::Receiver<()>) {
        let (complete, result) = watch::channel(None);
        let (graceful, requested) = oneshot::channel();
        (
            Self {
                result,
                graceful: Arc::new(Mutex::new(Some(graceful))),
            },
            Completion(Some(complete)),
            requested,
        )
    }
}

struct Completion(Option<watch::Sender<Option<Result<(), String>>>>);

impl Completion {
    fn finish(mut self, result: Result<(), String>) {
        if let Some(complete) = self.0.take() {
            complete.send_replace(Some(result));
        }
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        if let Some(complete) = self.0.take() {
            complete.send_replace(Some(Err(INTERRUPTED.into())));
        }
    }
}

pub(super) fn serve<I>(
    io: I,
    router: axum::Router,
    shutdown: watch::Receiver<()>,
) -> impl Future<Output = ()> + Send + 'static
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (receipt, completion, requested) = ConnectionReceipt::new();
    let router = router.layer(axum::Extension(receipt));
    // Capture completion before the future is polled so cancellation always resolves its receipt.
    drive(io, router, shutdown, requested, completion)
}

async fn drive<I>(
    io: I,
    router: axum::Router,
    mut shutdown: watch::Receiver<()>,
    requested: oneshot::Receiver<()>,
    completion: Completion,
) where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let connection = http1::Builder::new()
        .serve_connection(TokioIo::new(io), TowerToHyperService::new(router))
        .with_upgrades();
    tokio::pin!(connection);
    let result = tokio::select! {
        result = &mut connection => result,
        _ = shutdown.changed() => {
            connection.as_mut().graceful_shutdown();
            connection.await
        }
        _ = requested => {
            connection.as_mut().graceful_shutdown();
            connection.await
        }
    };
    completion.finish(result.map_err(connection_error));
}

fn connection_error(error: hyper::Error) -> String {
    let mut message = format!("MCP HTTP connection failed: {error}");
    let mut source = error.source();
    while let Some(error) = source {
        message.push_str(": ");
        message.push_str(&error.to_string());
        source = error.source();
    }
    message
}
