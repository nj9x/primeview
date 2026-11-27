//! HTTP API: alert CRUD, health and the SSE event stream.

use crate::state::{AppState, now_ms};
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::{Json, Router};
use futures_util::Stream;
use primeview_alerts::{Alert, Condition, Frequency, normalize_symbol, split_symbol};
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use tokio::sync::broadcast::error::RecvError;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

/// Providers the server has a price feed for.
pub const PROVIDERS: &[&str] = &["BINANCE", "TWELVEDATA"];

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/alerts", get(list_alerts).post(create_alert))
        .route("/api/alerts/{id}", patch(update_alert).delete(delete_alert))
        .route("/api/events", get(events))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token));

    Router::new()
        .route("/api/health", get(health))
        .merge(protected)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub struct ApiError(StatusCode, String);

impl ApiError {
    fn bad_request(msg: impl Into<String>) -> Self {
        ApiError(StatusCode::BAD_REQUEST, msg.into())
    }
    fn not_found(id: u64) -> Self {
        ApiError(StatusCode::NOT_FOUND, format!("alert {id} not found"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

/// Bearer token check. Accepts `Authorization: Bearer <token>` or, for
/// clients that cannot set headers (browser `EventSource`), `?token=<token>`.
async fn require_token(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(expected) = state.settings.token.as_deref() else {
        return next.run(req).await;
    };
    let from_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    let from_query = req.uri().query().and_then(|q| {
        q.split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == "token")
            .map(|(_, v)| v)
    });
    let ok = [from_header, from_query]
        .into_iter()
        .flatten()
        .any(|t| constant_time_eq(t.as_bytes(), expected.as_bytes()));
    if ok {
        return next.run(req).await;
    }
    let mut resp =
        ApiError(StatusCode::UNAUTHORIZED, "missing or invalid token".into()).into_response();
    resp.headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    resp
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let engine = state.engine();
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "alerts": engine.alerts().len(),
        "symbols": engine.symbols(),
    }))
}

async fn list_alerts(State(state): State<AppState>) -> Json<Vec<Alert>> {
    Json(state.engine().alerts().to_vec())
}

fn default_frequency() -> Frequency {
    Frequency::Once
}

#[derive(Debug, Deserialize)]
pub struct CreateAlert {
    pub symbol: String,
    pub condition: Condition,
    #[serde(default = "default_frequency")]
    pub frequency: Frequency,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub expires_ms: Option<i64>,
}

/// Validate a symbol and return its normalized form.
pub fn validate_symbol(raw: &str) -> Result<String, String> {
    if raw.trim().is_empty() {
        return Err("symbol is empty".into());
    }
    let symbol = normalize_symbol(raw);
    let (provider, ticker) = split_symbol(&symbol);
    if !PROVIDERS.contains(&provider.as_str()) {
        return Err(format!(
            "unsupported provider {provider:?} (supported: {})",
            PROVIDERS.join(", ")
        ));
    }
    let allowed: fn(char) -> bool = match provider.as_str() {
        "BINANCE" => |c| c.is_ascii_alphanumeric(),
        _ => |c| c.is_ascii_alphanumeric() || "/.-_".contains(c),
    };
    if ticker.is_empty() || ticker.len() > 32 || !ticker.chars().all(allowed) {
        return Err(format!("invalid ticker {ticker:?} for {provider}"));
    }
    Ok(symbol)
}

fn validate_create(body: &CreateAlert, now: i64) -> Result<String, String> {
    let symbol = validate_symbol(&body.symbol)?;
    if !body.condition.level().is_finite() {
        return Err("condition level must be a finite number".into());
    }
    if let Frequency::EveryTime { cooldown_ms } = body.frequency
        && cooldown_ms < 0
    {
        return Err("cooldown_ms must be >= 0".into());
    }
    if body.message.len() > 2000 {
        return Err("message is longer than 2000 bytes".into());
    }
    if body.expires_ms.is_some_and(|e| e <= now) {
        return Err("expires_ms is in the past".into());
    }
    Ok(symbol)
}

async fn create_alert(
    State(state): State<AppState>,
    Json(body): Json<CreateAlert>,
) -> ApiResult<(StatusCode, Json<Alert>)> {
    let now = now_ms();
    let symbol = validate_create(&body, now).map_err(ApiError::bad_request)?;
    let alert = Alert {
        id: 0,
        symbol,
        condition: body.condition,
        frequency: body.frequency,
        message: body.message,
        active: true,
        created_ms: now,
        last_fired_ms: None,
        expires_ms: body.expires_ms,
    };
    let created = {
        let mut engine = state.engine();
        let id = engine.add(alert);
        engine.get(id).cloned().expect("just added")
    };
    tracing::info!(id = created.id, symbol = %created.symbol, "alert created: {}", created.condition.describe());
    changed(&state).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

#[derive(Debug, Deserialize)]
pub struct UpdateAlert {
    pub active: bool,
}

async fn update_alert(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(body): Json<UpdateAlert>,
) -> ApiResult<Json<Alert>> {
    let updated = {
        let mut engine = state.engine();
        if !engine.set_active(id, body.active) {
            return Err(ApiError::not_found(id));
        }
        engine.get(id).cloned().expect("exists")
    };
    changed(&state).await?;
    Ok(Json(updated))
}

async fn delete_alert(State(state): State<AppState>, Path(id): Path<u64>) -> ApiResult<StatusCode> {
    if !state.engine().remove(id) {
        return Err(ApiError::not_found(id));
    }
    changed(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// After a mutation: persist and wake the feed supervisor.
async fn changed(state: &AppState) -> ApiResult<()> {
    state.alerts_changed.notify_one();
    state.persist().await.map_err(|e| {
        tracing::error!("persisting alerts failed: {e}");
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("change applied but not saved: {e}"),
        )
    })
}

/// Server-Sent Events: one `data:` JSON `Fired` object per event.
/// The stream ends when the server shuts down.
async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.fired_tx.subscribe();
    let shutdown = state.shutdown.clone();
    let stream = futures_util::stream::unfold(
        (rx, shutdown),
        |(mut rx, mut shutdown)| async move {
            loop {
                if *shutdown.borrow() {
                    return None;
                }
                tokio::select! {
                    r = rx.recv() => match r {
                        Ok(f) => {
                            let ev = Event::default().json_data(&f).unwrap_or_else(|_| Event::default());
                            return Some((Ok(ev), (rx, shutdown)));
                        }
                        Err(RecvError::Lagged(n)) => {
                            tracing::warn!("SSE client lagged, skipped {n} events");
                        }
                        Err(RecvError::Closed) => return None,
                    },
                    r = shutdown.changed() => if r.is_err() { return None; },
                }
            }
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}
