//! PrimeView core: market data model, timeframes, series transforms and indicators.
//!
//! This crate is UI-agnostic and has no platform dependencies, so it compiles
//! unchanged for native (Windows/macOS/Linux) and `wasm32-unknown-unknown`.

pub mod bar;
pub mod indicators;
pub mod time;
pub mod timeframe;

pub use bar::{Bar, Series};
pub use indicators::{Indicator, IndicatorOutput, Pane, Plot, PlotStyle};
pub use timeframe::Timeframe;
