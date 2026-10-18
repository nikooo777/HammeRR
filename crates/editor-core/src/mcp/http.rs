use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use rmcp::transport::streamable_http_server::{
    SessionManager, StreamableHttpServerConfig, StreamableHttpService,
    session::local::LocalSessionManager,
};
use tokio::runtime::Runtime;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinSet;

use super::EditorServer;

mod connection;
pub use connection::ConnectionReceipt;

const HTTP_WORKERS: usize = 2;
const NETWORK_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

#[must_use]
pub struct HttpServer {
    local_addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<Result<(), String>>>,
    finished: Option<Result<(), String>>,
}

impl HttpServer {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }

    pub fn shutdown(&mut self) -> Result<(), String> {
        if let Some(result) = &self.finished {
            return result.clone();
        }
        self.stop();
        let result = match self.thread.take().expect("HTTP thread missing").join() {
            Ok(result) => result,
            Err(payload) => {
                let message = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("unknown panic");
                Err(format!("MCP HTTP thread panicked: {message}"))
            }
        };
        self.finished = Some(result.clone());
        result
    }

    pub fn finish(
        mut self,
        primary: Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match (primary, self.shutdown()) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error.into()),
            (Err(primary), Err(shutdown)) => {
                Err(format!("{primary}; MCP HTTP shutdown failed: {shutdown}").into())
            }
        }
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        if self.finished.is_none()
            && let Err(error) = self.shutdown()
        {
            eprintln!("MCP HTTP shutdown failed: {error}");
        }
    }
}

pub fn spawn_http(server: EditorServer, addr: SocketAddr) -> Result<HttpServer, String> {
    spawn(server, addr, NETWORK_SHUTDOWN_GRACE)
}

fn spawn(
    server: EditorServer,
    addr: SocketAddr,
    network_grace: Duration,
) -> Result<HttpServer, String> {
    if server.operation.is_some() {
        return Err("MCP HTTP requires an idle server without document admission".into());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(HTTP_WORKERS)
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind(addr))
        .map_err(|error| format!("MCP server could not listen on {addr}: {error}"))?;
    let local_addr = listener.local_addr().map_err(|error| error.to_string())?;
    let (stop, stopped) = oneshot::channel();
    let worker = HttpThread {
        runtime: Some(runtime),
        anchor: server,
        listener: Some(listener),
        stopped: Some(stopped),
        network_grace,
    };
    let thread = std::thread::Builder::new()
        .name("mcp-http".into())
        .spawn(move || worker.run())
        .map_err(|error| error.to_string())?;
    Ok(HttpServer {
        local_addr,
        stop: Some(stop),
        thread: Some(thread),
        finished: None,
    })
}

struct HttpThread {
    // Runtime teardown must release every worker before the last idle document owner.
    runtime: Option<Runtime>,
    anchor: EditorServer,
    listener: Option<tokio::net::TcpListener>,
    stopped: Option<oneshot::Receiver<()>>,
    network_grace: Duration,
}

impl HttpThread {
    fn run(mut self) -> Result<(), String> {
        let result = self
            .runtime
            .as_ref()
            .expect("HTTP runtime missing")
            .block_on(serve(
                self.anchor.clone(),
                self.listener.take().expect("HTTP listener missing"),
                self.stopped.take().expect("HTTP stop receiver missing"),
                self.network_grace,
            ));
        drop(self.runtime.take());
        result
    }
}

async fn serve(
    server: EditorServer,
    listener: tokio::net::TcpListener,
    mut stopped: oneshot::Receiver<()>,
    network_grace: Duration,
) -> Result<(), String> {
    let config = StreamableHttpServerConfig::default();
    let cancellation = config.cancellation_token.clone();
    let sessions = Arc::new(LocalSessionManager::default());
    let service: StreamableHttpService<EditorServer, LocalSessionManager> =
        StreamableHttpService::new(move || Ok(server.clone()), sessions.clone(), config);
    let router = axum::Router::new().nest_service("/mcp", service);
    let (shutdown, shutdown_requested) = watch::channel(());
    let mut connections = JoinSet::new();
    let mut errors = Vec::new();
    loop {
        tokio::select! {
            _ = &mut stopped => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        connections.spawn(connection::serve(
                            stream,
                            router.clone(),
                            shutdown_requested.clone(),
                        ));
                    }
                    Err(error) if matches!(error.kind(), ErrorKind::Interrupted | ErrorKind::ConnectionAborted | ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset) => {}
                    Err(error) => {
                        errors.push(format!("MCP HTTP listener failed: {error}"));
                        break;
                    }
                }
            }
            joined = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = joined {
                    errors.push(format!("MCP HTTP connection task failed: {error}"));
                    break;
                }
            }
        }
    }
    drop(listener);
    cancellation.cancel();
    let _ = shutdown.send(());
    let drain = async {
        let ids: Vec<_> = sessions.sessions.read().await.keys().cloned().collect();
        for id in ids {
            if let Err(error) = sessions.close_session(&id).await {
                errors.push(format!("MCP session {id} could not close: {error}"));
            }
        }
        while let Some(joined) = connections.join_next().await {
            if let Err(error) = joined {
                errors.push(format!("MCP HTTP connection task failed: {error}"));
            }
        }
    };
    if tokio::time::timeout(network_grace, drain).await.is_err() {
        errors.push(format!(
            "MCP HTTP network shutdown exceeded {} ms",
            network_grace.as_millis()
        ));
        connections.abort_all();
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(test)]
mod tests;
