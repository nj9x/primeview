//! Market data sources for PrimeView.
//!
//! * [`binance`] — public crypto market data (REST history + WebSocket live
//!   updates). No API key required. Works on native and in the browser.
//! * [`twelvedata`] — stocks, ETFs, indices, forex, commodities (API key, polled).
//! * [`demo`] — deterministic synthetic data for offline use and tests.
//! * [`symbol`] — provider-qualified symbols (`BINANCE:BTCUSDT`, `TWELVEDATA:AAPL`).
//!
//! All network APIs are callback/poll based (no async runtime) so the same
//! code runs inside an `eframe` frame loop on every platform.

pub mod binance;
pub mod demo;
pub mod symbol;
pub mod twelvedata;

pub use symbol::{Provider, SymbolId};

/// A 24h ticker snapshot for the watchlist.
#[derive(Clone, Debug, PartialEq)]
pub struct Ticker {
    pub symbol: String,
    pub last: f64,
    pub open_24h: f64,
}

impl Ticker {
    pub fn change_pct(&self) -> f64 {
        if self.open_24h == 0.0 {
            0.0
        } else {
            (self.last / self.open_24h - 1.0) * 100.0
        }
    }
}

/// Connection state of a streaming source, for the status indicator.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamStatus {
    Connecting,
    Live,
    Closed,
    Error(String),
}
