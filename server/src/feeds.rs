//! Price feeds: a supervisor that keeps feed subscriptions in sync with the
//! engine's active symbols, a Binance websocket feed and a Twelve Data poller.

use crate::state::{AppState, now_ms};
use futures_util::{SinkExt, StreamExt};
use primeview_alerts::split_symbol;
use serde_json::Value;
use std::collections::BTreeSet;
use std::time::Duration;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

pub const BINANCE_WS: &str = "wss://data-stream.binance.vision/stream";
pub const TWELVEDATA_PRICE: &str = "https://api.twelvedata.com/price";

/// How often the supervisor re-checks symbols and sweeps expired alerts even
/// without a change notification.
const SUPERVISOR_TICK: Duration = Duration::from_secs(5);
/// Reconnect a websocket that has been silent this long.
const WS_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const MAX_POLL_BACKOFF: Duration = Duration::from_secs(600);

type Symbols = BTreeSet<String>;

/// Spawn the supervisor and both feeds. They stop when shutdown is signalled.
pub fn spawn(state: AppState) {
    let (binance_tx, binance_rx) = watch::channel(Symbols::new());
    let (td_tx, td_rx) = watch::channel(Symbols::new());
    tokio::spawn(binance_feed(state.clone(), binance_rx));
    tokio::spawn(twelvedata_feed(state.clone(), td_rx));
    tokio::spawn(supervisor(state, binance_tx, td_tx));
}

/// Split normalized symbols into (binance tickers, twelvedata tickers).
pub fn partition(symbols: &[String]) -> (Symbols, Symbols) {
    let mut binance = Symbols::new();
    let mut td = Symbols::new();
    for s in symbols {
        let (provider, ticker) = split_symbol(s);
        match provider.as_str() {
            "BINANCE" => {
                binance.insert(ticker);
            }
            "TWELVEDATA" => {
                td.insert(ticker);
            }
            other => tracing::warn!(symbol = %s, "no price feed for provider {other}"),
        }
    }
    (binance, td)
}

async fn supervisor(state: AppState, binance: watch::Sender<Symbols>, td: watch::Sender<Symbols>) {
    let shutdown = state.shutdown.clone();
    loop {
        let swept = state.engine().expire(now_ms());
        if swept > 0 {
            tracing::info!("{swept} alert(s) expired");
            state.persist_logged().await;
        }
        let (b, t) = partition(&state.engine().symbols());
        for (tx, set, name) in [(&binance, b, "binance"), (&td, t, "twelvedata")] {
            tx.send_if_modified(|cur| {
                if *cur == set {
                    return false;
                }
                tracing::info!(feed = name, symbols = ?set, "feed symbols changed");
                *cur = set;
                true
            });
        }
        tokio::select! {
            _ = state.alerts_changed.notified() => {}
            _ = tokio::time::sleep(SUPERVISOR_TICK) => {}
            _ = stopped(shutdown.clone()) => return,
        }
    }
}

pub fn binance_url(tickers: &Symbols) -> String {
    let streams: Vec<String> = tickers
        .iter()
        .map(|t| format!("{}@aggTrade", t.to_lowercase()))
        .collect();
    format!("{BINANCE_WS}?streams={}", streams.join("/"))
}

/// Parse a combined-stream aggTrade message into (symbol, price, trade time ms).
pub fn parse_binance(text: &str) -> Option<(String, f64, i64)> {
    let v: Value = serde_json::from_str(text).ok()?;
    let d = v.get("data")?;
    let ticker = d.get("s")?.as_str()?;
    let price: f64 = d.get("p")?.as_str()?.parse().ok()?;
    let time = d.get("T").and_then(Value::as_i64).unwrap_or_else(now_ms);
    Some((format!("BINANCE:{ticker}"), price, time))
}

async fn binance_feed(state: AppState, mut symbols: watch::Receiver<Symbols>) {
    let shutdown = state.shutdown.clone();
    let mut backoff = Duration::from_secs(1);
    loop {
        if *shutdown.borrow() {
            return;
        }
        let tickers = symbols.borrow_and_update().clone();
        if tickers.is_empty() {
            tokio::select! {
                r = symbols.changed() => if r.is_err() { return },
                _ = stopped(shutdown.clone()) => return,
            }
            continue;
        }
        let url = binance_url(&tickers);
        tracing::info!(%url, "binance: connecting");
        let connect = tokio::time::timeout(
            Duration::from_secs(15),
            tokio_tungstenite::connect_async(url.as_str()),
        );
        let mut ws = match connect.await {
            Ok(Ok((ws, _))) => ws,
            Ok(Err(e)) => {
                tracing::warn!("binance: connect failed: {e}; retrying in {backoff:?}");
                if sleep_or_shutdown(backoff, &shutdown).await {
                    return;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
            Err(_) => {
                tracing::warn!("binance: connect timed out; retrying in {backoff:?}");
                if sleep_or_shutdown(backoff, &shutdown).await {
                    return;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };
        tracing::info!(count = tickers.len(), "binance: connected");
        let mut got_data = false;
        let reason = loop {
            tokio::select! {
                msg = tokio::time::timeout(WS_IDLE_TIMEOUT, ws.next()) => match msg {
                    Err(_) => break "idle timeout",
                    Ok(None) => break "stream ended",
                    Ok(Some(Err(e))) => {
                        tracing::warn!("binance: websocket error: {e}");
                        break "error";
                    }
                    Ok(Some(Ok(Message::Text(text)))) => {
                        if let Some((sym, price, t)) = parse_binance(&text) {
                            if !got_data {
                                got_data = true;
                                backoff = Duration::from_secs(1);
                                tracing::info!(symbol = %sym, price, "binance: first price");
                            }
                            state.on_price(&sym, price, t);
                        }
                    }
                    Ok(Some(Ok(Message::Ping(p)))) => {
                        let _ = ws.send(Message::Pong(p)).await;
                    }
                    Ok(Some(Ok(Message::Close(_)))) => break "closed by server",
                    Ok(Some(Ok(_))) => {}
                },
                r = symbols.changed() => {
                    if r.is_err() { return; }
                    break "symbols changed";
                }
                _ = stopped(shutdown.clone()) => {
                    let _ = ws.close(None).await;
                    return;
                }
            }
        };
        let _ = ws.close(None).await;
        if reason == "symbols changed" {
            tracing::info!("binance: resubscribing");
            continue;
        }
        tracing::warn!("binance: disconnected ({reason}); reconnecting in {backoff:?}");
        if sleep_or_shutdown(backoff, &shutdown).await {
            return;
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Resolves once shutdown has been signalled (or the sender is gone).
async fn stopped(mut rx: watch::Receiver<bool>) {
    let _ = rx.wait_for(|s| *s).await;
}

/// Returns true if shutdown was signalled while sleeping.
async fn sleep_or_shutdown(d: Duration, shutdown: &watch::Receiver<bool>) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(d) => *shutdown.borrow(),
        _ = stopped(shutdown.clone()) => true,
    }
}

/// Outcome of parsing a Twelve Data `/price` response.
#[derive(Debug, PartialEq)]
pub enum TdResult {
    /// (ticker, price) pairs plus per-symbol errors.
    Prices(Vec<(String, f64)>, Vec<(String, String)>),
    /// Request-level error (`{"code":429,"message":...}`).
    Error(i64, String),
}

fn td_error(v: &Value) -> Option<(i64, String)> {
    let is_err =
        v.get("status").and_then(Value::as_str) == Some("error") || v.get("code").is_some();
    if !is_err || v.get("price").is_some() {
        return None;
    }
    Some((
        v.get("code").and_then(Value::as_i64).unwrap_or(0),
        v.get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string(),
    ))
}

fn td_price(v: &Value) -> Option<f64> {
    match v.get("price")? {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

/// Parse a `/price` body. A single-symbol request returns `{"price":"1.2"}`;
/// a batch returns `{"AAPL":{"price":"1.2"},"EUR/USD":{...}}`.
pub fn parse_twelvedata(body: &str, tickers: &Symbols) -> TdResult {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return TdResult::Error(0, format!("invalid JSON: {e}")),
    };
    if let Some((code, msg)) = td_error(&v) {
        return TdResult::Error(code, msg);
    }
    let mut prices = Vec::new();
    let mut errors = Vec::new();
    if tickers.len() == 1 && v.get("price").is_some() {
        let t = tickers.iter().next().expect("one").clone();
        match td_price(&v) {
            Some(p) => prices.push((t, p)),
            None => errors.push((t, "unparseable price".into())),
        }
        return TdResult::Prices(prices, errors);
    }
    let Some(obj) = v.as_object() else {
        return TdResult::Error(0, "unexpected response shape".into());
    };
    for (k, item) in obj {
        let ticker = k.to_uppercase();
        if let Some((code, msg)) = td_error(item) {
            errors.push((ticker, format!("{code}: {msg}")));
        } else if let Some(p) = td_price(item) {
            prices.push((ticker, p));
        } else {
            errors.push((ticker, "no price".into()));
        }
    }
    TdResult::Prices(prices, errors)
}

async fn twelvedata_feed(state: AppState, mut symbols: watch::Receiver<Symbols>) {
    let shutdown = state.shutdown.clone();
    let interval = Duration::from_secs(state.settings.poll_secs.max(1));
    let mut delay = interval;
    loop {
        let tickers = symbols.borrow_and_update().clone();
        if tickers.is_empty() {
            tokio::select! {
                r = symbols.changed() => if r.is_err() { return },
                _ = stopped(shutdown.clone()) => return,
            }
            continue;
        }
        let joined = tickers.iter().cloned().collect::<Vec<_>>().join(",");
        let req = state.http.get(TWELVEDATA_PRICE).query(&[
            ("symbol", joined.as_str()),
            ("apikey", state.settings.twelvedata_key.as_str()),
        ]);
        let result = match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                match resp.text().await {
                    Ok(body) if status.as_u16() == 429 => TdResult::Error(429, body),
                    Ok(body) => parse_twelvedata(&body, &tickers),
                    Err(e) => TdResult::Error(status.as_u16() as i64, e.to_string()),
                }
            }
            Err(e) => TdResult::Error(0, e.to_string()),
        };
        match result {
            TdResult::Prices(prices, errors) => {
                delay = interval;
                let now = now_ms();
                for (ticker, price) in prices {
                    tracing::debug!(%ticker, price, "twelvedata price");
                    state.on_price(&format!("TWELVEDATA:{ticker}"), price, now);
                }
                for (ticker, err) in errors {
                    tracing::warn!(%ticker, "twelvedata: {err}");
                }
            }
            TdResult::Error(code, msg) => {
                if matches!(code, 401 | 403 | 429) {
                    delay = (delay * 2).min(MAX_POLL_BACKOFF);
                }
                tracing::warn!(
                    code,
                    "twelvedata: request failed: {msg}; next poll in {delay:?}"
                );
            }
        }
        // Wait for the next poll; a symbol change triggers an early poll, but
        // never sooner than 10 s after the previous one (rate limits).
        let next = tokio::time::Instant::now() + delay;
        tokio::select! {
            _ = tokio::time::sleep_until(next) => {}
            r = symbols.changed() => {
                if r.is_err() { return; }
                let early = tokio::time::Instant::now() + Duration::from_secs(10);
                tokio::select! {
                    _ = tokio::time::sleep_until(early.min(next)) => {}
                    _ = stopped(shutdown.clone()) => return,
                }
            }
            _ = stopped(shutdown.clone()) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> Symbols {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn partition_by_provider() {
        let (b, t) = partition(&[
            "BINANCE:BTCUSDT".into(),
            "BINANCE:ETHUSDT".into(),
            "TWELVEDATA:AAPL".into(),
            "TWELVEDATA:EUR/USD".into(),
            "OTHER:X".into(),
        ]);
        assert_eq!(b, set(&["BTCUSDT", "ETHUSDT"]));
        assert_eq!(t, set(&["AAPL", "EUR/USD"]));
    }

    #[test]
    fn binance_combined_url() {
        assert_eq!(
            binance_url(&set(&["BTCUSDT", "ETHUSDT"])),
            "wss://data-stream.binance.vision/stream?streams=btcusdt@aggTrade/ethusdt@aggTrade"
        );
    }

    #[test]
    fn binance_message() {
        let msg = r#"{"stream":"btcusdt@aggTrade","data":{"e":"aggTrade","E":1700000000001,
            "s":"BTCUSDT","a":1,"p":"65000.10","q":"0.01","f":1,"l":1,"T":1700000000000,"m":true}}"#;
        assert_eq!(
            parse_binance(msg),
            Some(("BINANCE:BTCUSDT".into(), 65000.10, 1_700_000_000_000))
        );
        assert_eq!(parse_binance(r#"{"result":null,"id":1}"#), None);
        assert_eq!(parse_binance("not json"), None);
    }

    #[test]
    fn twelvedata_single() {
        assert_eq!(
            parse_twelvedata(r#"{"price":"355.44"}"#, &set(&["AAPL"])),
            TdResult::Prices(vec![("AAPL".into(), 355.44)], vec![])
        );
    }

    #[test]
    fn twelvedata_batch_with_symbol_error() {
        let body = r#"{"AAPL":{"price":"355.44"},"EUR/USD":{"price":"1.0812"},
            "XYZ":{"code":404,"message":"symbol not found","status":"error"}}"#;
        let TdResult::Prices(mut p, e) = parse_twelvedata(body, &set(&["AAPL", "EUR/USD", "XYZ"]))
        else {
            panic!("expected prices");
        };
        p.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(p, vec![("AAPL".into(), 355.44), ("EUR/USD".into(), 1.0812)]);
        assert_eq!(e, vec![("XYZ".into(), "404: symbol not found".into())]);
    }

    #[test]
    fn twelvedata_request_error() {
        let body = r#"{"code":429,"message":"You have run out of API credits","status":"error"}"#;
        assert_eq!(
            parse_twelvedata(body, &set(&["AAPL", "MSFT"])),
            TdResult::Error(429, "You have run out of API credits".into())
        );
        let body = r#"{"code":401,"message":"apikey is invalid"}"#;
        assert!(matches!(
            parse_twelvedata(body, &set(&["AAPL"])),
            TdResult::Error(401, _)
        ));
        assert!(matches!(
            parse_twelvedata("<html>", &set(&["AAPL"])),
            TdResult::Error(0, _)
        ));
    }
}
