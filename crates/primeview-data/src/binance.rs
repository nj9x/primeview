//! Binance public market data (spot). Uses the `binance.vision` market-data
//! hosts, which need no key and send CORS headers, so they work from the web
//! build as well as native.

use primeview_core::{Bar, Timeframe};
use serde_json::Value;

use crate::{StreamStatus, Ticker};

pub const REST_BASE: &str = "https://data-api.binance.vision/api/v3";
pub const WS_BASE: &str = "wss://data-stream.binance.vision";

/// Max bars per history request allowed by the API.
pub const MAX_LIMIT: usize = 1000;

/// Fetch up to `limit` klines. With `end_time` set, fetches bars that opened
/// at or before that time (used for scroll-back history).
pub fn fetch_klines(
    symbol: &str,
    tf: Timeframe,
    end_time: Option<i64>,
    limit: usize,
    on_done: impl FnOnce(Result<Vec<Bar>, String>) + Send + 'static,
) {
    let mut url = format!(
        "{REST_BASE}/klines?symbol={}&interval={}&limit={}",
        symbol.to_uppercase(),
        tf.api_code(),
        limit.clamp(1, MAX_LIMIT)
    );
    if let Some(end) = end_time {
        url.push_str(&format!("&endTime={end}"));
    }
    ehttp::fetch(ehttp::Request::get(url), move |res| {
        let out = match res {
            Err(e) => Err(e),
            Ok(r) if !r.ok => Err(format!(
                "HTTP {} {}: {}",
                r.status,
                r.status_text,
                r.text().unwrap_or("")
            )),
            Ok(r) => parse_klines(&r.bytes),
        };
        on_done(out);
    });
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

/// Parse the REST `klines` response: `[[openTime, "o", "h", "l", "c", "v", ...], ...]`.
pub fn parse_klines(bytes: &[u8]) -> Result<Vec<Bar>, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let rows = v.as_array().ok_or("expected array")?;
    rows.iter()
        .map(|row| {
            let r = row.as_array().ok_or("expected row array")?;
            let f = |i: usize| {
                r.get(i)
                    .and_then(num)
                    .ok_or_else(|| format!("bad field {i}"))
            };
            Ok(Bar {
                time: r.first().and_then(Value::as_i64).ok_or("bad time")?,
                open: f(1)?,
                high: f(2)?,
                low: f(3)?,
                close: f(4)?,
                volume: f(5)?,
            })
        })
        .collect()
}

/// Parse a kline stream event: `{"e":"kline","k":{"t":..,"o":"..",...}}`.
pub fn parse_kline_event(text: &str) -> Option<Bar> {
    let v: Value = serde_json::from_str(text).ok()?;
    let k = v.get("data").unwrap_or(&v).get("k")?;
    Some(Bar {
        time: k.get("t")?.as_i64()?,
        open: num(k.get("o")?)?,
        high: num(k.get("h")?)?,
        low: num(k.get("l")?)?,
        close: num(k.get("c")?)?,
        volume: num(k.get("v")?)?,
    })
}

/// Parse a combined-stream mini ticker event.
pub fn parse_mini_ticker(text: &str) -> Option<Ticker> {
    let v: Value = serde_json::from_str(text).ok()?;
    let d = v.get("data").unwrap_or(&v);
    Some(Ticker {
        symbol: d.get("s")?.as_str()?.to_string(),
        last: num(d.get("c")?)?,
        open_24h: num(d.get("o")?)?,
    })
}

/// A live WebSocket subscription. Poll it once per frame.
pub struct Stream {
    // Dropping the sender closes the socket, so keep it alive.
    _tx: ewebsock::WsSender,
    rx: ewebsock::WsReceiver,
    pub status: StreamStatus,
}

impl Stream {
    fn connect(url: String, wake: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let (tx, rx) = ewebsock::connect_with_wakeup(url, ewebsock::Options::default(), wake)?;
        Ok(Self {
            _tx: tx,
            rx,
            status: StreamStatus::Connecting,
        })
    }

    /// Live candle updates for one symbol/timeframe.
    pub fn klines(
        symbol: &str,
        tf: Timeframe,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let url = format!(
            "{WS_BASE}/ws/{}@kline_{}",
            symbol.to_lowercase(),
            tf.api_code()
        );
        Self::connect(url, wake)
    }

    /// 24h mini tickers for many symbols over one connection.
    pub fn tickers(
        symbols: &[String],
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let streams: Vec<String> = symbols
            .iter()
            .map(|s| format!("{}@miniTicker", s.to_lowercase()))
            .collect();
        let url = format!("{WS_BASE}/stream?streams={}", streams.join("/"));
        Self::connect(url, wake)
    }

    /// Drain pending text messages, updating `status` as a side effect.
    pub fn poll(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(ev) = self.rx.try_recv() {
            match ev {
                ewebsock::WsEvent::Opened => self.status = StreamStatus::Live,
                ewebsock::WsEvent::Message(ewebsock::WsMessage::Text(t)) => {
                    self.status = StreamStatus::Live;
                    out.push(t);
                }
                ewebsock::WsEvent::Message(_) => {}
                ewebsock::WsEvent::Error(e) => self.status = StreamStatus::Error(e),
                ewebsock::WsEvent::Closed => self.status = StreamStatus::Closed,
            }
        }
        out
    }
}

/// Default watchlist.
pub fn default_symbols() -> Vec<String> {
    [
        "BTCUSDT", "ETHUSDT", "SOLUSDT", "BNBUSDT", "XRPUSDT", "DOGEUSDT", "ADAUSDT", "LINKUSDT",
        "AVAXUSDT", "LTCUSDT",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rest_klines() {
        let body = br#"[[1700000000000,"100.5","101.0","99.5","100.9","12.34",1700000059999,"0",1,"0","0","0"]]"#;
        let bars = parse_klines(body).unwrap();
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].time, 1_700_000_000_000);
        assert_eq!(bars[0].close, 100.9);
        assert_eq!(bars[0].volume, 12.34);
    }

    #[test]
    fn parses_stream_events() {
        let k = r#"{"e":"kline","E":1,"s":"BTCUSDT","k":{"t":1700000000000,"T":1,"s":"BTCUSDT","i":"1m","o":"1","h":"3","l":"0.5","c":"2","v":"9","x":false}}"#;
        let b = parse_kline_event(k).unwrap();
        assert_eq!(
            (b.open, b.high, b.low, b.close, b.volume),
            (1.0, 3.0, 0.5, 2.0, 9.0)
        );

        let t = r#"{"stream":"btcusdt@miniTicker","data":{"e":"24hrMiniTicker","s":"BTCUSDT","c":"110","o":"100","h":"1","l":"1","v":"1","q":"1"}}"#;
        let t = parse_mini_ticker(t).unwrap();
        assert_eq!(t.symbol, "BTCUSDT");
        assert!((t.change_pct() - 10.0).abs() < 1e-9);
    }
}
