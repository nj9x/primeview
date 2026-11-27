//! In-process router tests. No feeds are started and nothing touches the
//! internet (the webhook test uses a local listener on 127.0.0.1).

use crate::api;
use crate::state::{AppState, Settings, State, load_engine};
use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use primeview_alerts::{Alert, Engine, Fired};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::watch;
use tower::ServiceExt;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Ctx {
    state: AppState,
    app: Router,
    dir: PathBuf,
    _shutdown: watch::Sender<bool>,
}

impl Drop for Ctx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn ctx_with(token: Option<&str>, webhooks: Vec<String>) -> Ctx {
    let dir = std::env::temp_dir().join(format!(
        "primeview-server-test-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let settings = Settings {
        data_path: dir.join("alerts.json"),
        webhooks,
        token: token.map(str::to_string),
        twelvedata_key: "demo".into(),
        poll_secs: 60,
    };
    let (tx, rx) = watch::channel(false);
    let state = State::new(settings, Engine::new(), rx);
    Ctx {
        app: api::router(state.clone()),
        state,
        dir,
        _shutdown: tx,
    }
}

fn ctx() -> Ctx {
    ctx_with(None, vec![])
}

async fn call(app: &Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    call_auth(app, method, uri, body, None).await
}

async fn call_auth(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = bearer {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string())),
        None => b.body(Body::empty()),
    }
    .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

fn btc_above_1() -> Value {
    json!({"symbol": "btcusdt", "condition": {"Above": 1.0}, "frequency": "Once", "message": ""})
}

#[tokio::test]
async fn health() {
    let c = ctx();
    let (s, v) = call(&c.app, Method::GET, "/api/health", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["status"], "ok");
    assert_eq!(v["alerts"], 0);
}

#[tokio::test]
async fn alert_crud_roundtrip_and_persistence() {
    let c = ctx();
    let (s, created) = call(&c.app, Method::POST, "/api/alerts", Some(btc_above_1())).await;
    assert_eq!(s, StatusCode::CREATED);
    let alert: Alert = serde_json::from_value(created.clone()).unwrap();
    assert_eq!(alert.id, 1);
    assert_eq!(alert.symbol, "BINANCE:BTCUSDT");
    assert!(alert.active);
    assert!(alert.created_ms > 0);

    // defaults: frequency Once, empty message; TWELVEDATA ticker with '/'
    let (s, v) = call(
        &c.app,
        Method::POST,
        "/api/alerts",
        Some(json!({"symbol": "twelvedata:eur/usd",
                    "condition": {"CrossingUp": 1.1},
                    "frequency": {"EveryTime": {"cooldown_ms": 60000}},
                    "message": "fx", "expires_ms": 32503680000000i64})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["symbol"], "TWELVEDATA:EUR/USD");
    assert_eq!(v["frequency"]["EveryTime"]["cooldown_ms"], 60000);

    let (s, list) = call(&c.app, Method::GET, "/api/alerts", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 2);

    // file on disk reflects the change
    let on_disk = load_engine(&c.state.settings.data_path).unwrap();
    assert_eq!(on_disk.alerts().len(), 2);
    assert!(
        !c.state
            .settings
            .data_path
            .with_extension("json.tmp")
            .exists()
    );

    let (s, v) = call(
        &c.app,
        Method::PATCH,
        "/api/alerts/1",
        Some(json!({"active": false})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["active"], false);
    assert!(!load_engine(&c.state.settings.data_path).unwrap().alerts()[0].active);

    let (s, _) = call(
        &c.app,
        Method::PATCH,
        "/api/alerts/99",
        Some(json!({"active": true})),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let (s, _) = call(&c.app, Method::DELETE, "/api/alerts/1", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, v) = call(&c.app, Method::DELETE, "/api/alerts/1", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["error"], "alert 1 not found");

    let (_, list) = call(&c.app, Method::GET, "/api/alerts", None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(
        load_engine(&c.state.settings.data_path)
            .unwrap()
            .alerts()
            .len(),
        1
    );
}

#[tokio::test]
async fn create_minimal_body_defaults_to_once() {
    let c = ctx();
    let (s, v) = call(
        &c.app,
        Method::POST,
        "/api/alerts",
        Some(json!({"symbol": "ETHUSDT", "condition": {"Below": 100.0}})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(v["frequency"], "Once");
    assert_eq!(v["message"], "");
}

#[tokio::test]
async fn create_validation() {
    let c = ctx();
    let cases = [
        json!({"symbol": "", "condition": {"Above": 1.0}}),
        json!({"symbol": "YAHOO:AAPL", "condition": {"Above": 1.0}}),
        json!({"symbol": "BTC/USDT", "condition": {"Above": 1.0}}),
        json!({"symbol": "BINANCE:BTC USDT", "condition": {"Above": 1.0}}),
        json!({"symbol": "TWELVEDATA:AAPL&apikey=x", "condition": {"Above": 1.0}}),
        json!({"symbol": "TWELVEDATA:", "condition": {"Above": 1.0}}),
        json!({"symbol": format!("TWELVEDATA:{}", "A".repeat(33)), "condition": {"Above": 1.0}}),
        json!({"symbol": "BTCUSDT", "condition": {"Above": 1.0},
               "frequency": {"EveryTime": {"cooldown_ms": -1}}}),
        json!({"symbol": "BTCUSDT", "condition": {"Above": 1.0}, "expires_ms": 1000}),
    ];
    for body in cases {
        let (s, v) = call(&c.app, Method::POST, "/api/alerts", Some(body.clone())).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body} -> {v}");
        assert!(v["error"].is_string());
    }
    // malformed JSON / unknown condition are rejected by the extractor
    let (s, _) = call(
        &c.app,
        Method::POST,
        "/api/alerts",
        Some(json!({"symbol": "BTCUSDT", "condition": {"Sideways": 1.0}})),
    )
    .await;
    assert!(s.is_client_error());
    assert!(c.state.engine().alerts().is_empty());
}

#[tokio::test]
async fn token_required_when_configured() {
    let c = ctx_with(Some("s3cret"), vec![]);
    // health stays open
    let (s, _) = call(&c.app, Method::GET, "/api/health", None).await;
    assert_eq!(s, StatusCode::OK);

    for (method, uri, body) in [
        (Method::GET, "/api/alerts", None),
        (Method::POST, "/api/alerts", Some(btc_above_1())),
        (
            Method::PATCH,
            "/api/alerts/1",
            Some(json!({"active": false})),
        ),
        (Method::DELETE, "/api/alerts/1", None),
        (Method::GET, "/api/events", None),
    ] {
        let (s, _) = call(&c.app, method.clone(), uri, body.clone()).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{method} {uri} without token");
        let (s, _) = call_auth(&c.app, method.clone(), uri, body, Some("wrong")).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{method} {uri} wrong token");
    }

    let (s, _) = call_auth(
        &c.app,
        Method::POST,
        "/api/alerts",
        Some(btc_above_1()),
        Some("s3cret"),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, v) = call_auth(&c.app, Method::GET, "/api/alerts", None, Some("s3cret")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v.as_array().unwrap().len(), 1);
    // query-string token (for EventSource)
    let (s, _) = call(&c.app, Method::GET, "/api/alerts?token=s3cret", None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = call(&c.app, Method::GET, "/api/alerts?token=s3cre", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn no_token_means_open() {
    let c = ctx();
    let (s, _) = call_auth(&c.app, Method::GET, "/api/alerts", None, Some("anything")).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn cors_is_permissive() {
    let c = ctx();
    let req = Request::builder()
        .uri("/api/health")
        .header(header::ORIGIN, "https://example.com")
        .body(Body::empty())
        .unwrap();
    let resp = c.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
}

#[tokio::test]
async fn price_fires_once_alert_and_persists() {
    let c = ctx();
    call(&c.app, Method::POST, "/api/alerts", Some(btc_above_1())).await;
    let mut rx = c.state.fired_tx.subscribe();
    let fired = c.state.on_price("BINANCE:BTCUSDT", 65000.0, 1_000);
    assert_eq!(fired.len(), 1);
    let got = rx.recv().await.unwrap();
    assert_eq!(got.message, "BINANCE:BTCUSDT above 1");
    assert_eq!(got.price, 65000.0);

    let (_, v) = call(&c.app, Method::GET, "/api/alerts", None).await;
    assert_eq!(v[0]["active"], false);
    assert_eq!(v[0]["last_fired_ms"], 1000);
    // persistence after fires happens in a background task
    for _ in 0..50 {
        let e = load_engine(&c.state.settings.data_path).unwrap();
        if e.alerts().first().is_some_and(|a| !a.active) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("fired alert was not persisted");
}

#[tokio::test]
async fn sse_streams_fired_events() {
    let c = ctx();
    call(&c.app, Method::POST, "/api/alerts", Some(btc_above_1())).await;
    let req = Request::builder()
        .uri("/api/events")
        .body(Body::empty())
        .unwrap();
    let resp = c.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()[header::CONTENT_TYPE], "text/event-stream");
    let mut body = resp.into_body();

    c.state.on_price("BTCUSDT", 70000.5, 42);
    let text = tokio::time::timeout(Duration::from_secs(5), async {
        let mut buf = String::new();
        while !buf.contains("\n\n") {
            let frame = body.frame().await.unwrap().unwrap();
            if let Ok(d) = frame.into_data() {
                buf.push_str(std::str::from_utf8(&d).unwrap());
            }
        }
        buf
    })
    .await
    .expect("SSE event within 5s");
    let data = text
        .lines()
        .find_map(|l| l.strip_prefix("data: "))
        .expect("data line");
    let f: Fired = serde_json::from_str(data).unwrap();
    assert_eq!(f.alert_id, 1);
    assert_eq!(f.symbol, "BINANCE:BTCUSDT");
    assert_eq!(f.price, 70000.5);
    assert_eq!(f.time_ms, 42);
}

#[tokio::test]
async fn sse_ends_on_shutdown() {
    let c = ctx();
    let req = Request::builder()
        .uri("/api/events")
        .body(Body::empty())
        .unwrap();
    let resp = c.app.clone().oneshot(req).await.unwrap();
    let mut body = resp.into_body();
    c._shutdown.send(true).unwrap();
    let end = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match body.frame().await {
                None => return,
                Some(Ok(_)) => continue,
                Some(Err(e)) => panic!("{e}"),
            }
        }
    })
    .await;
    assert!(end.is_ok(), "SSE stream did not end on shutdown");
}

#[tokio::test]
async fn webhooks_receive_fired_json() {
    use axum::routing::post;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let hook = Router::new().route(
        "/hook",
        post(move |axum::Json(v): axum::Json<Value>| {
            let tx = tx.clone();
            async move {
                tx.send(v).unwrap();
                StatusCode::NO_CONTENT
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, hook).await.unwrap() });

    let c = ctx_with(None, vec![format!("http://{addr}/hook")]);
    call(&c.app, Method::POST, "/api/alerts", Some(btc_above_1())).await;
    c.state.on_price("BINANCE:BTCUSDT", 2.0, 7);
    let v = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("webhook within 5s")
        .unwrap();
    assert_eq!(
        v,
        json!({"alert_id": 1, "symbol": "BINANCE:BTCUSDT", "price": 2.0, "time_ms": 7,
               "message": "BINANCE:BTCUSDT above 1"})
    );
}

#[test]
fn symbol_validation() {
    assert_eq!(api::validate_symbol("btcusdt").unwrap(), "BINANCE:BTCUSDT");
    assert_eq!(
        api::validate_symbol("TwelveData:brk.b").unwrap(),
        "TWELVEDATA:BRK.B"
    );
    assert_eq!(
        api::validate_symbol("twelvedata:xau/usd").unwrap(),
        "TWELVEDATA:XAU/USD"
    );
    assert!(api::validate_symbol("YAHOO:AAPL").is_err());
    assert!(api::validate_symbol("BINANCE:BTC-USDT").is_err());
    assert!(api::validate_symbol("TWELVEDATA:A,B").is_err());
    assert!(api::validate_symbol("   ").is_err());
}

#[test]
fn corrupt_data_file_is_an_error() {
    let dir = std::env::temp_dir().join(format!("primeview-corrupt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("alerts.json");
    std::fs::write(&p, "{not json").unwrap();
    assert!(load_engine(&p).is_err());
    assert!(
        load_engine(&dir.join("missing.json"))
            .unwrap()
            .alerts()
            .is_empty()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
