use std::sync::{Arc, atomic::Ordering};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::cli::config::{Algorithm, Server};

use super::state::AppState;
use super::writer::ConfigCommand;

#[derive(RustEmbed)]
#[folder = "ui/dist/"]
struct UiAssets;

#[derive(Debug, Serialize)]
pub struct BackendView {
    pub address: String,
    pub weight: u32,
    pub enabled: bool,
    pub draining: bool,
    pub healthy: bool,
    pub active_connections: usize,
    pub total_connections: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub probe_failures: usize,
    pub probe_successes: usize,
    pub connect_failures: u64,
}

#[derive(Debug, Deserialize)]
pub struct AddBackendRequest {
    pub address: std::net::SocketAddr,
    pub weight: u32,
    pub enabled: Option<bool>,
    pub draining: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateBackendRequest {
    pub weight: Option<u32>,
    pub enabled: Option<bool>,
    pub draining: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct SetAlgorithmRequest {
    pub algorithm: Algorithm,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(serve_index))
        .route("/assets/{*path}", get(serve_asset))
        .route("/metrics", get(metrics))
        .route("/healthz", get(healthz))
        .route("/api/v1/backends", get(list_backends).post(add_backend))
        .route(
            "/api/v1/backends/{address}",
            get(get_backend)
                .patch(update_backend)
                .delete(delete_backend),
        )
        .route("/api/v1/algorithm", get(get_algorithm).put(set_algorithm))
        .with_state(state)
}

async fn serve_index() -> impl IntoResponse {
    match UiAssets::get("index.html") {
        Some(f) => asset_response("index.html", f.data.into_owned()),
        None => error(
            StatusCode::NOT_FOUND,
            "UI not built (run npm run build in ui/)",
        ),
    }
}

async fn serve_asset(Path(path): Path<String>) -> impl IntoResponse {
    let key = format!("assets/{path}");
    match UiAssets::get(&key) {
        Some(f) => asset_response(&key, f.data.into_owned()),
        None => error(StatusCode::NOT_FOUND, format!("no asset {key}")),
    }
}

fn asset_response(key: &str, data: Vec<u8>) -> Response {
    let content_type = if key.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if key.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else if key.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if key.ends_with(".svg") {
        "image/svg+xml"
    } else if key.ends_with(".json") {
        "application/json"
    } else if key.ends_with(".ico") {
        "image/x-icon"
    } else {
        "application/octet-stream"
    };
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(data))
        .unwrap()
}

async fn list_backends(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let snapshot = state.pool.borrow().clone();
    let views: Vec<BackendView> = snapshot.backends.iter().map(view_of).collect();
    Json(views)
}

async fn get_backend(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> impl IntoResponse {
    let snapshot = state.pool.borrow().clone();
    match snapshot
        .backends
        .iter()
        .find(|b| b.address.to_string() == address)
    {
        Some(b) => (
            StatusCode::OK,
            Json(serde_json::to_value(view_of(b)).unwrap()),
        )
            .into_response(),
        None => error(StatusCode::NOT_FOUND, format!("no backend {address}")),
    }
}

async fn add_backend(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddBackendRequest>,
) -> impl IntoResponse {
    let server = Server {
        address: req.address,
        weight: req.weight,
        enabled: req.enabled.unwrap_or(true),
        draining: req.draining.unwrap_or(false),
    };
    let (tx, rx) = oneshot::channel();
    let cmd = ConfigCommand::AddBackend { server, reply: tx };
    if state.config_tx.send(cmd).await.is_err() {
        return error(StatusCode::SERVICE_UNAVAILABLE, "config writer gone");
    }
    match rx.await {
        Ok(Ok(())) => (StatusCode::CREATED, Json(serde_json::json!({"ok": true}))).into_response(),
        Ok(Err(e)) => error(StatusCode::BAD_REQUEST, e.to_string()),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "writer dropped reply"),
    }
}

async fn update_backend(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
    Json(req): Json<UpdateBackendRequest>,
) -> impl IntoResponse {
    let addr: std::net::SocketAddr = match address.parse() {
        Ok(a) => a,
        Err(_) => return error(StatusCode::BAD_REQUEST, format!("bad address {address}")),
    };
    let (tx, rx) = oneshot::channel();
    let cmd = ConfigCommand::UpdateBackend {
        address: addr,
        weight: req.weight,
        enabled: req.enabled,
        draining: req.draining,
        reply: tx,
    };
    if state.config_tx.send(cmd).await.is_err() {
        return error(StatusCode::SERVICE_UNAVAILABLE, "config writer gone");
    }
    match rx.await {
        Ok(Ok(())) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Ok(Err(e)) => map_command_error(e.to_string()).into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "writer dropped reply"),
    }
}

async fn delete_backend(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> impl IntoResponse {
    let addr: std::net::SocketAddr = match address.parse() {
        Ok(a) => a,
        Err(_) => return error(StatusCode::BAD_REQUEST, format!("bad address {address}")),
    };
    let (tx, rx) = oneshot::channel();
    let cmd = ConfigCommand::RemoveBackend {
        address: addr,
        reply: tx,
    };
    if state.config_tx.send(cmd).await.is_err() {
        return error(StatusCode::SERVICE_UNAVAILABLE, "config writer gone");
    }
    match rx.await {
        Ok(Ok(())) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Ok(Err(e)) => map_command_error(e.to_string()).into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "writer dropped reply"),
    }
}

async fn get_algorithm(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let algo = state.pool.borrow().algorithm;
    Json(serde_json::json!({"algorithm": algo}))
}

async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

async fn metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let snapshot = state.pool.borrow().clone();
    let rejected = state.rejected_connections.load(Ordering::Relaxed);
    let uptime = state.started_at.elapsed().as_secs_f64();
    let healthy_count = snapshot
        .backends
        .iter()
        .filter(|b| b.runtime.healthy.load(Ordering::Acquire))
        .count();

    let mut out = String::with_capacity(1024 + snapshot.backends.len() * 512);
    out.push_str("# HELP sama_uptime_seconds Seconds since process start.\n");
    out.push_str("# TYPE sama_uptime_seconds gauge\n");
    out.push_str(&format!("sama_uptime_seconds {uptime:.3}\n"));
    out.push_str("# HELP sama_backends_total Configured backends.\n");
    out.push_str("# TYPE sama_backends_total gauge\n");
    out.push_str(&format!(
        "sama_backends_total {}\n",
        snapshot.backends.len()
    ));
    out.push_str("# HELP sama_healthy_backends Backends currently marked healthy.\n");
    out.push_str("# TYPE sama_healthy_backends gauge\n");
    out.push_str(&format!("sama_healthy_backends {healthy_count}\n"));
    out.push_str(
        "# HELP sama_rejected_connections_total Connections rejected by max_connections limit.\n",
    );
    out.push_str("# TYPE sama_rejected_connections_total counter\n");
    out.push_str(&format!("sama_rejected_connections_total {rejected}\n"));

    out.push_str("# HELP sama_backend_healthy 1 if backend is healthy else 0.\n");
    out.push_str("# TYPE sama_backend_healthy gauge\n");
    out.push_str("# HELP sama_backend_active_connections Currently proxied connections.\n");
    out.push_str("# TYPE sama_backend_active_connections gauge\n");
    out.push_str("# HELP sama_backend_total_connections_total Lifetime proxied connections.\n");
    out.push_str("# TYPE sama_backend_total_connections_total counter\n");
    out.push_str("# HELP sama_backend_bytes_in_total Bytes client->backend.\n");
    out.push_str("# TYPE sama_backend_bytes_in_total counter\n");
    out.push_str("# HELP sama_backend_bytes_out_total Bytes backend->client.\n");
    out.push_str("# TYPE sama_backend_bytes_out_total counter\n");
    out.push_str("# HELP sama_backend_connect_failures_total Failed upstream dials.\n");
    out.push_str("# TYPE sama_backend_connect_failures_total counter\n");
    out.push_str("# HELP sama_backend_info Static labels (weight/enabled/draining).\n");
    out.push_str("# TYPE sama_backend_info gauge\n");

    for b in &snapshot.backends {
        let addr = b.address.to_string();
        let healthy = i32::from(b.runtime.healthy.load(Ordering::Acquire));
        let active = b.runtime.active_connections.load(Ordering::Relaxed);
        let total = b.runtime.total_connections.load(Ordering::Relaxed);
        let bytes_in = b.runtime.bytes_in.load(Ordering::Relaxed);
        let bytes_out = b.runtime.bytes_out.load(Ordering::Relaxed);
        let connect_failures = b.runtime.connect_failures.load(Ordering::Relaxed);
        let enabled = i32::from(b.enabled);
        let draining = i32::from(b.draining);
        out.push_str(&format!(
            "sama_backend_healthy{{backend=\"{addr}\"}} {healthy}\n"
        ));
        out.push_str(&format!(
            "sama_backend_active_connections{{backend=\"{addr}\"}} {active}\n"
        ));
        out.push_str(&format!(
            "sama_backend_total_connections_total{{backend=\"{addr}\"}} {total}\n"
        ));
        out.push_str(&format!(
            "sama_backend_bytes_in_total{{backend=\"{addr}\"}} {bytes_in}\n"
        ));
        out.push_str(&format!(
            "sama_backend_bytes_out_total{{backend=\"{addr}\"}} {bytes_out}\n"
        ));
        out.push_str(&format!(
            "sama_backend_connect_failures_total{{backend=\"{addr}\"}} {connect_failures}\n"
        ));
        out.push_str(&format!(
            "sama_backend_info{{backend=\"{addr}\",weight=\"{}\",enabled=\"{}\",draining=\"{}\"}} {enabled}\n",
            b.weight, enabled, draining
        ));
    }

    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], out)
}

async fn set_algorithm(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetAlgorithmRequest>,
) -> impl IntoResponse {
    let (tx, rx) = oneshot::channel();
    let cmd = ConfigCommand::SetAlgorithm {
        algorithm: req.algorithm,
        reply: tx,
    };
    if state.config_tx.send(cmd).await.is_err() {
        return error(StatusCode::SERVICE_UNAVAILABLE, "config writer gone");
    }
    match rx.await {
        Ok(Ok(())) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Ok(Err(e)) => error(StatusCode::BAD_REQUEST, e.to_string()),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "writer dropped reply"),
    }
}

fn view_of(b: &Arc<crate::snapshot::Backend>) -> BackendView {
    BackendView {
        address: b.address.to_string(),
        weight: b.weight,
        enabled: b.enabled,
        draining: b.draining,
        healthy: b.runtime.healthy.load(Ordering::Acquire),
        active_connections: b.runtime.active_connections.load(Ordering::Relaxed),
        total_connections: b.runtime.total_connections.load(Ordering::Relaxed),
        bytes_in: b.runtime.bytes_in.load(Ordering::Relaxed),
        bytes_out: b.runtime.bytes_out.load(Ordering::Relaxed),
        probe_failures: b.runtime.probe_failures.load(Ordering::Relaxed),
        probe_successes: b.runtime.probe_successes.load(Ordering::Relaxed),
        connect_failures: b.runtime.connect_failures.load(Ordering::Relaxed),
    }
}

fn error(status: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (status, Json(serde_json::json!({"error": msg.into()}))).into_response()
}

fn map_command_error(msg: String) -> axum::response::Response {
    if msg.contains("active_connections") {
        error(StatusCode::CONFLICT, msg)
    } else if msg.contains("no backend") {
        error(StatusCode::NOT_FOUND, msg)
    } else {
        error(StatusCode::BAD_REQUEST, msg)
    }
}
