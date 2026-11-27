//! Twelve Data (https://twelvedata.com): stocks, ETFs, indices, forex,
//! commodities and crypto. Needs an API key (free tier: 8 requests/minute,
//! 800/day); the `demo` key works for a handful of symbols such as AAPL and
//! EUR/USD. Sends CORS headers, so the web build can call it directly.

use primeview_core::time::{MS_PER_DAY, days_from_civil};
use primeview_core::{Bar, Timeframe};
use serde_json::Value;

pub const BASE: &str = "https://api.twelvedata.com";
pub const DEMO_KEY: &str = "demo";
/// Max bars per request allowed by the API.
pub const MAX_OUTPUT: usize = 5000;

pub fn interval(tf: Timeframe) -> &'static str {
    match tf {
        Timeframe::M1 => "1min",
        Timeframe::M5 => "5min",
        Timeframe::M15 => "15min",
        Timeframe::M30 => "30min",
        Timeframe::H1 => "1h",
        Timeframe::H4 => "4h",
        Timeframe::D1 => "1day",
        Timeframe::W1 => "1week",
    }
}

/// Percent-encode a query value (tickers may contain `/`, `^`, `=`).
pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn format_utc(ms: i64) -> String {
    let dt = primeview_core::time::DateTime::from_millis(ms);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
    )
}

pub fn series_url(
    ticker: &str,
    tf: Timeframe,
    end_time: Option<i64>,
    outputsize: usize,
    key: &str,
) -> String {
    let mut url = format!(
        "{BASE}/time_series?symbol={}&interval={}&outputsize={}&timezone=UTC&order=ASC&apikey={}",
        encode(ticker),
        interval(tf),
        outputsize.clamp(1, MAX_OUTPUT),
        encode(key)
    );
    if let Some(end) = end_time {
        url.push_str(&format!("&end_date={}", encode(&format_utc(end))));
    }
    url
}

/// Fetch bars ending at `end_time` (inclusive) or the latest bars.
pub fn fetch_series(
    ticker: &str,
    tf: Timeframe,
    end_time: Option<i64>,
    outputsize: usize,
    key: &str,
    on_done: impl FnOnce(Result<Vec<Bar>, String>) + Send + 'static,
) {
    let url = series_url(ticker, tf, end_time, outputsize, key);
    ehttp::fetch(ehttp::Request::get(url), move |res| {
        on_done(match res {
            Err(e) => Err(e),
            Ok(r) => parse_series(&r.bytes).map_err(|e| {
                if r.ok {
                    e
                } else {
                    format!("HTTP {}: {e}", r.status)
                }
            }),
        })
    });
}

/// Batch quotes for the watchlist (one request for many tickers). Tickers the
/// key isn't entitled to are silently skipped.
pub fn fetch_quotes(
    tickers: &[String],
    key: &str,
    on_done: impl FnOnce(Result<Vec<crate::Ticker>, String>) + Send + 'static,
) {
    if tickers.is_empty() {
        on_done(Ok(Vec::new()));
        return;
    }
    let list: Vec<String> = tickers.iter().map(|t| encode(t)).collect();
    let url = format!(
        "{BASE}/quote?symbol={}&apikey={}",
        list.join(","),
        encode(key)
    );
    let single = tickers.len() == 1;
    ehttp::fetch(ehttp::Request::get(url), move |res| {
        on_done(match res {
            Err(e) => Err(e),
            Ok(r) => parse_quotes(&r.bytes, single),
        })
    });
}

pub fn parse_quotes(bytes: &[u8], single: bool) -> Result<Vec<crate::Ticker>, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| format!("invalid response: {e}"))?;
    let one = |q: &Value| -> Option<crate::Ticker> {
        if q.get("code").is_some() {
            return None;
        }
        Some(crate::Ticker {
            symbol: format!("TWELVEDATA:{}", q.get("symbol")?.as_str()?),
            last: num(q.get("close"))?,
            open_24h: num(q.get("previous_close"))?,
        })
    };
    if single {
        if v.get("code").is_some() {
            return Err(v
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("error")
                .to_string());
        }
        return Ok(one(&v).into_iter().collect());
    }
    let obj = v.as_object().ok_or("expected object")?;
    Ok(obj.values().filter_map(one).collect())
}

/// Parse `YYYY-MM-DD` or `YYYY-MM-DD HH:MM:SS` (UTC) into Unix ms.
pub fn parse_datetime(s: &str) -> Option<i64> {
    let (date, time) = s.split_once(' ').unwrap_or((s, "00:00:00"));
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    let mut t = time.split(':').map(|p| p.parse::<i64>().ok());
    let (h, mi) = (t.next()??, t.next()??);
    let sec = t.next().flatten().unwrap_or(0);
    Some(
        days_from_civil(y as i32, m as u32, day as u32) * MS_PER_DAY
            + ((h * 60 + mi) * 60 + sec) * 1000,
    )
}

fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

pub fn parse_series(bytes: &[u8]) -> Result<Vec<Bar>, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| format!("invalid response: {e}"))?;
    if v.get("status").and_then(Value::as_str) == Some("error") || v.get("code").is_some() {
        let msg = v
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(msg.to_string());
    }
    let values = v
        .get("values")
        .and_then(Value::as_array)
        .ok_or("response has no values")?;
    let mut bars: Vec<Bar> = values
        .iter()
        .filter_map(|row| {
            Some(Bar {
                time: parse_datetime(row.get("datetime")?.as_str()?)?,
                open: num(row.get("open"))?,
                high: num(row.get("high"))?,
                low: num(row.get("low"))?,
                close: num(row.get("close"))?,
                volume: num(row.get("volume")).unwrap_or(0.0),
            })
        })
        .collect();
    bars.sort_by_key(|b| b.time);
    Ok(bars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_series_and_errors() {
        let ok = br#"{"meta":{"symbol":"AAPL"},"values":[
            {"datetime":"2026-11-25 15:30:00","open":"354.35","high":"356.75","low":"353.64","close":"355.44","volume":"5470011"},
            {"datetime":"2026-11-25 14:30:00","open":"355.41","high":"355.67","low":"354.32","close":"354.35","volume":"3112604"}],"status":"ok"}"#;
        let bars = parse_series(ok).unwrap();
        assert_eq!(bars.len(), 2);
        assert!(bars[0].time < bars[1].time, "sorted ascending");
        assert_eq!(bars[1].close, 355.44);

        let fx = br#"{"values":[{"datetime":"2026-11-27","open":"1.05","high":"1.06","low":"1.04","close":"1.055"}],"status":"ok"}"#;
        assert_eq!(parse_series(fx).unwrap()[0].volume, 0.0);

        let err = br#"{"code":401,"message":"bad key","status":"error"}"#;
        assert_eq!(parse_series(err).unwrap_err(), "bad key");
    }

    #[test]
    fn parses_quotes() {
        let many = br#"{"AAPL":{"symbol":"AAPL","close":"355.4","previous_close":"350.0"},
                        "MSFT":{"code":401,"message":"not allowed"}}"#;
        let q = parse_quotes(many, false).unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].symbol, "TWELVEDATA:AAPL");
        let one = br#"{"symbol":"EUR/USD","close":"1.05","previous_close":"1.0"}"#;
        assert!((parse_quotes(one, true).unwrap()[0].change_pct() - 5.0).abs() < 1e-9);
        assert!(parse_quotes(br#"{"code":401,"message":"nope"}"#, true).is_err());
    }

    #[test]
    fn datetimes() {
        let d = days_from_civil(2026, 11, 25) * MS_PER_DAY;
        assert_eq!(parse_datetime("2026-11-25"), Some(d));
        assert_eq!(
            parse_datetime("2026-11-25 15:30:00"),
            Some(d + (15 * 60 + 30) * 60_000)
        );
        assert_eq!(parse_datetime("2026-13-01"), None);
        assert_eq!(parse_datetime("garbage"), None);
        assert_eq!(format_utc(d + 3_600_000), "2026-11-25 01:00:00");
    }

    #[test]
    fn urls() {
        assert_eq!(encode("EUR/USD"), "EUR%2FUSD");
        let u = series_url("EUR/USD", Timeframe::H4, Some(0), 99_999, "k");
        assert!(
            u.contains("symbol=EUR%2FUSD")
                && u.contains("interval=4h")
                && u.contains("outputsize=5000")
        );
        assert!(u.contains("end_date=1970-01-01%2000%3A00%3A00"));
    }
}
