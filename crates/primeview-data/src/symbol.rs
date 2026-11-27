//! Provider-qualified symbols: `PROVIDER:TICKER`, e.g. `BINANCE:BTCUSDT`,
//! `TWELVEDATA:AAPL`, `TWELVEDATA:EUR/USD`, or the offline `DEMO`.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Provider {
    /// Binance spot crypto. Free, keyless, real-time WebSocket.
    Binance,
    /// Twelve Data: stocks, ETFs, indices, forex, commodities. API key; polled.
    TwelveData,
    /// Deterministic synthetic data, works offline.
    Demo,
}

impl Provider {
    pub fn prefix(self) -> &'static str {
        match self {
            Provider::Binance => "BINANCE",
            Provider::TwelveData => "TWELVEDATA",
            Provider::Demo => "DEMO",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Binance => "Binance",
            Provider::TwelveData => "Twelve Data",
            Provider::Demo => "Demo",
        }
    }

    pub fn from_prefix(p: &str) -> Option<Self> {
        match p.to_ascii_uppercase().as_str() {
            "BINANCE" => Some(Provider::Binance),
            "TWELVEDATA" | "TD" => Some(Provider::TwelveData),
            "DEMO" => Some(Provider::Demo),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolId {
    pub provider: Provider,
    pub ticker: String,
}

/// Quote assets that identify a bare ticker as a Binance pair.
const BINANCE_QUOTES: [&str; 8] = ["USDT", "USDC", "FDUSD", "BUSD", "BTC", "ETH", "BNB", "TRY"];

impl SymbolId {
    pub fn new(provider: Provider, ticker: &str) -> Self {
        Self {
            provider,
            ticker: ticker.trim().to_uppercase(),
        }
    }

    pub fn demo() -> Self {
        Self::new(Provider::Demo, "DEMO")
    }

    /// Parse user input. Explicit prefixes win; bare tickers are resolved by
    /// shape: `DEMO` → demo, `EUR/USD` → Twelve Data, `BTCUSDT` → Binance,
    /// anything else (e.g. `AAPL`) → Twelve Data.
    pub fn parse(input: &str) -> Option<Self> {
        let s = input.trim().to_uppercase();
        if s.is_empty() {
            return None;
        }
        if let Some((prefix, ticker)) = s.split_once(':') {
            let provider = Provider::from_prefix(prefix)?;
            if provider == Provider::Demo {
                return Some(Self::demo());
            }
            let ticker = ticker.trim();
            return (!ticker.is_empty() && valid_ticker(ticker))
                .then(|| Self::new(provider, ticker));
        }
        if !valid_ticker(&s) {
            return None;
        }
        if s == "DEMO" {
            return Some(Self::demo());
        }
        if s.contains('/') {
            return Some(Self::new(Provider::TwelveData, &s));
        }
        let is_pair = BINANCE_QUOTES
            .iter()
            .any(|q| s.len() > q.len() + 1 && s.ends_with(q));
        Some(Self::new(
            if is_pair {
                Provider::Binance
            } else {
                Provider::TwelveData
            },
            &s,
        ))
    }

    /// Canonical `PROVIDER:TICKER` form used for storage and alerts.
    pub fn canonical(&self) -> String {
        match self.provider {
            Provider::Demo => "DEMO".into(),
            p => format!("{}:{}", p.prefix(), self.ticker),
        }
    }
}

impl std::fmt::Display for SymbolId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.ticker)
    }
}

fn valid_ticker(s: &str) -> bool {
    s.len() <= 24
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | '^' | '='))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inputs() {
        let p = |s| SymbolId::parse(s).map(|x| x.canonical());
        assert_eq!(p("btcusdt").as_deref(), Some("BINANCE:BTCUSDT"));
        assert_eq!(p("AAPL").as_deref(), Some("TWELVEDATA:AAPL"));
        assert_eq!(p("eur/usd").as_deref(), Some("TWELVEDATA:EUR/USD"));
        assert_eq!(p("td:msft").as_deref(), Some("TWELVEDATA:MSFT"));
        assert_eq!(p("BINANCE:ETHBTC").as_deref(), Some("BINANCE:ETHBTC"));
        assert_eq!(p("demo").as_deref(), Some("DEMO"));
        assert_eq!(p("DEMO").map(|_| ()), Some(()));
        assert_eq!(p("NOPE:AAPL"), None);
        assert_eq!(p("AA PL"), None);
        assert_eq!(p(""), None);
        // canonical form round-trips
        for s in ["BINANCE:BTCUSDT", "TWELVEDATA:EUR/USD", "DEMO"] {
            assert_eq!(p(s).as_deref(), Some(s));
        }
    }
}
