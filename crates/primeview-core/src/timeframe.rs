use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Timeframe {
    M1,
    M5,
    M15,
    M30,
    H1,
    H4,
    D1,
    W1,
}

impl Timeframe {
    pub const ALL: [Timeframe; 8] = [
        Timeframe::M1,
        Timeframe::M5,
        Timeframe::M15,
        Timeframe::M30,
        Timeframe::H1,
        Timeframe::H4,
        Timeframe::D1,
        Timeframe::W1,
    ];

    pub fn millis(self) -> i64 {
        const MIN: i64 = 60_000;
        match self {
            Timeframe::M1 => MIN,
            Timeframe::M5 => 5 * MIN,
            Timeframe::M15 => 15 * MIN,
            Timeframe::M30 => 30 * MIN,
            Timeframe::H1 => 60 * MIN,
            Timeframe::H4 => 240 * MIN,
            Timeframe::D1 => 1_440 * MIN,
            Timeframe::W1 => 10_080 * MIN,
        }
    }

    /// Short label for toolbars, e.g. `15m`, `4h`, `1D`.
    pub fn label(self) -> &'static str {
        match self {
            Timeframe::M1 => "1m",
            Timeframe::M5 => "5m",
            Timeframe::M15 => "15m",
            Timeframe::M30 => "30m",
            Timeframe::H1 => "1h",
            Timeframe::H4 => "4h",
            Timeframe::D1 => "1D",
            Timeframe::W1 => "1W",
        }
    }

    /// Interval code used by most exchange APIs (Binance-style).
    pub fn api_code(self) -> &'static str {
        match self {
            Timeframe::D1 => "1d",
            Timeframe::W1 => "1w",
            other => other.label(),
        }
    }

    pub fn is_intraday(self) -> bool {
        self.millis() < Timeframe::D1.millis()
    }
}
