//! Technical indicators. Every output vector is aligned 1:1 with the input
//! bars; values that are not yet defined (warm-up period) are `f64::NAN`.

use serde::{Deserialize, Serialize};

use crate::bar::Bar;
use crate::time::MS_PER_DAY;

/// Where an indicator is drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Pane {
    /// On top of the price chart, sharing its price scale.
    Overlay,
    /// In its own pane below the price chart.
    Separate {
        /// Fixed value range (e.g. 0..100 for RSI). `None` = autoscale.
        range: Option<(f64, f64)>,
        /// Horizontal reference levels (e.g. 30/70 for RSI).
        levels: Vec<f64>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlotStyle {
    Line,
    Histogram,
    /// Shaded band between this plot and the next plot in the list.
    BandFill,
}

#[derive(Clone, Debug)]
pub struct Plot {
    pub name: String,
    pub values: Vec<f64>,
    pub style: PlotStyle,
    /// sRGBA colour hint. The renderer may override it.
    pub color: [u8; 4],
}

#[derive(Clone, Debug)]
pub struct IndicatorOutput {
    pub label: String,
    pub pane: Pane,
    pub plots: Vec<Plot>,
}

/// An indicator and its parameters. Serializable so layouts can be saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Indicator {
    Sma {
        period: usize,
    },
    Ema {
        period: usize,
    },
    Bollinger {
        period: usize,
        mult: f64,
    },
    Vwap,
    Rsi {
        period: usize,
    },
    Macd {
        fast: usize,
        slow: usize,
        signal: usize,
    },
    Atr {
        period: usize,
    },
    Stochastic {
        k: usize,
        d: usize,
        smooth: usize,
    },
}

const BLUE: [u8; 4] = [41, 98, 255, 255];
const ORANGE: [u8; 4] = [255, 152, 0, 255];
const PURPLE: [u8; 4] = [156, 39, 176, 255];
const TEAL: [u8; 4] = [0, 150, 136, 255];
const GREY: [u8; 4] = [120, 123, 134, 255];
const RED: [u8; 4] = [239, 83, 80, 255];

impl Indicator {
    /// The catalogue shown in the "Indicators" menu, with default params.
    pub fn catalogue() -> Vec<Indicator> {
        vec![
            Indicator::Sma { period: 20 },
            Indicator::Ema { period: 50 },
            Indicator::Bollinger {
                period: 20,
                mult: 2.0,
            },
            Indicator::Vwap,
            Indicator::Rsi { period: 14 },
            Indicator::Macd {
                fast: 12,
                slow: 26,
                signal: 9,
            },
            Indicator::Atr { period: 14 },
            Indicator::Stochastic {
                k: 14,
                d: 3,
                smooth: 3,
            },
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            Indicator::Sma { .. } => "Moving Average",
            Indicator::Ema { .. } => "Exponential Moving Average",
            Indicator::Bollinger { .. } => "Bollinger Bands",
            Indicator::Vwap => "VWAP (daily)",
            Indicator::Rsi { .. } => "Relative Strength Index",
            Indicator::Macd { .. } => "MACD",
            Indicator::Atr { .. } => "Average True Range",
            Indicator::Stochastic { .. } => "Stochastic",
        }
    }

    pub fn label(&self) -> String {
        match self {
            Indicator::Sma { period } => format!("SMA {period}"),
            Indicator::Ema { period } => format!("EMA {period}"),
            Indicator::Bollinger { period, mult } => format!("BB {period} {mult}"),
            Indicator::Vwap => "VWAP".into(),
            Indicator::Rsi { period } => format!("RSI {period}"),
            Indicator::Macd { fast, slow, signal } => format!("MACD {fast} {slow} {signal}"),
            Indicator::Atr { period } => format!("ATR {period}"),
            Indicator::Stochastic { k, d, smooth } => format!("Stoch {k} {d} {smooth}"),
        }
    }

    /// Same indicator type, ignoring parameters.
    pub fn same_kind(&self, other: &Indicator) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    pub fn compute(&self, bars: &[Bar]) -> IndicatorOutput {
        let close: Vec<f64> = bars.iter().map(|b| b.close).collect();
        let label = self.label();
        match *self {
            Indicator::Sma { period } => {
                overlay(label, vec![line("SMA", sma(&close, period), BLUE)])
            }
            Indicator::Ema { period } => {
                overlay(label, vec![line("EMA", ema(&close, period), ORANGE)])
            }
            Indicator::Bollinger { period, mult } => {
                let (mid, upper, lower) = bollinger(&close, period, mult);
                let mut up = line("Upper", upper, TEAL);
                up.style = PlotStyle::BandFill;
                overlay(
                    label,
                    vec![up, line("Lower", lower, TEAL), line("Basis", mid, RED)],
                )
            }
            Indicator::Vwap => overlay(label, vec![line("VWAP", vwap_daily(bars), PURPLE)]),
            Indicator::Rsi { period } => IndicatorOutput {
                label,
                pane: Pane::Separate {
                    range: Some((0.0, 100.0)),
                    levels: vec![30.0, 50.0, 70.0],
                },
                plots: vec![line("RSI", rsi(&close, period), PURPLE)],
            },
            Indicator::Macd { fast, slow, signal } => {
                let (m, s, h) = macd(&close, fast, slow, signal);
                let mut hist = line("Histogram", h, GREY);
                hist.style = PlotStyle::Histogram;
                IndicatorOutput {
                    label,
                    pane: Pane::Separate {
                        range: None,
                        levels: vec![0.0],
                    },
                    plots: vec![hist, line("MACD", m, BLUE), line("Signal", s, ORANGE)],
                }
            }
            Indicator::Atr { period } => IndicatorOutput {
                label,
                pane: Pane::Separate {
                    range: None,
                    levels: vec![],
                },
                plots: vec![line("ATR", atr(bars, period), RED)],
            },
            Indicator::Stochastic { k, d, smooth } => {
                let (kk, dd) = stochastic(bars, k, d, smooth);
                IndicatorOutput {
                    label,
                    pane: Pane::Separate {
                        range: Some((0.0, 100.0)),
                        levels: vec![20.0, 80.0],
                    },
                    plots: vec![line("%K", kk, BLUE), line("%D", dd, ORANGE)],
                }
            }
        }
    }
}

fn line(name: &str, values: Vec<f64>, color: [u8; 4]) -> Plot {
    Plot {
        name: name.to_string(),
        values,
        style: PlotStyle::Line,
        color,
    }
}

fn overlay(label: String, plots: Vec<Plot>) -> IndicatorOutput {
    IndicatorOutput {
        label,
        pane: Pane::Overlay,
        plots,
    }
}

// ---------------------------------------------------------------------------
// Primitive building blocks. All skip a leading run of NaNs so they can be
// chained (e.g. EMA of an EMA).

fn first_valid(v: &[f64]) -> usize {
    v.iter().position(|x| !x.is_nan()).unwrap_or(v.len())
}

/// Simple moving average.
pub fn sma(v: &[f64], period: usize) -> Vec<f64> {
    let mut out = vec![f64::NAN; v.len()];
    if period == 0 {
        return out;
    }
    let start = first_valid(v);
    let mut sum = 0.0;
    for i in start..v.len() {
        sum += v[i];
        if i >= start + period {
            sum -= v[i - period];
        }
        if i + 1 >= start + period {
            out[i] = sum / period as f64;
        }
    }
    out
}

/// Exponential smoothing seeded with the SMA of the first `period` values.
fn smooth(v: &[f64], period: usize, alpha: f64) -> Vec<f64> {
    let mut out = vec![f64::NAN; v.len()];
    if period == 0 {
        return out;
    }
    let start = first_valid(v);
    if start + period > v.len() {
        return out;
    }
    let seed_idx = start + period - 1;
    let mut prev = v[start..=seed_idx].iter().sum::<f64>() / period as f64;
    out[seed_idx] = prev;
    for i in seed_idx + 1..v.len() {
        prev = alpha * v[i] + (1.0 - alpha) * prev;
        out[i] = prev;
    }
    out
}

/// Exponential moving average (alpha = 2 / (n + 1)).
pub fn ema(v: &[f64], period: usize) -> Vec<f64> {
    smooth(v, period, 2.0 / (period as f64 + 1.0))
}

/// Wilder's moving average / RMA (alpha = 1 / n), used by RSI and ATR.
pub fn rma(v: &[f64], period: usize) -> Vec<f64> {
    smooth(v, period, 1.0 / period.max(1) as f64)
}

/// Returns (basis, upper, lower) using population standard deviation.
pub fn bollinger(v: &[f64], period: usize, mult: f64) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let mid = sma(v, period);
    let mut upper = vec![f64::NAN; v.len()];
    let mut lower = vec![f64::NAN; v.len()];
    for i in 0..v.len() {
        if mid[i].is_nan() {
            continue;
        }
        let window = &v[i + 1 - period..=i];
        let var = window.iter().map(|x| (x - mid[i]).powi(2)).sum::<f64>() / period as f64;
        let sd = var.sqrt();
        upper[i] = mid[i] + mult * sd;
        lower[i] = mid[i] - mult * sd;
    }
    (mid, upper, lower)
}

pub fn rsi(close: &[f64], period: usize) -> Vec<f64> {
    let n = close.len();
    let mut out = vec![f64::NAN; n];
    if n < 2 {
        return out;
    }
    let mut gains = vec![f64::NAN; n];
    let mut losses = vec![f64::NAN; n];
    for i in 1..n {
        let ch = close[i] - close[i - 1];
        gains[i] = ch.max(0.0);
        losses[i] = (-ch).max(0.0);
    }
    let ag = rma(&gains, period);
    let al = rma(&losses, period);
    for i in 0..n {
        if ag[i].is_nan() || al[i].is_nan() {
            continue;
        }
        out[i] = if al[i] == 0.0 {
            if ag[i] == 0.0 { 50.0 } else { 100.0 }
        } else {
            100.0 - 100.0 / (1.0 + ag[i] / al[i])
        };
    }
    out
}

/// Returns (macd, signal, histogram).
pub fn macd(
    close: &[f64],
    fast: usize,
    slow: usize,
    signal: usize,
) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let f = ema(close, fast);
    let s = ema(close, slow);
    let m: Vec<f64> = f.iter().zip(&s).map(|(a, b)| a - b).collect();
    let sig = ema(&m, signal);
    let hist = m.iter().zip(&sig).map(|(a, b)| a - b).collect();
    (m, sig, hist)
}

pub fn true_range(bars: &[Bar]) -> Vec<f64> {
    bars.iter()
        .enumerate()
        .map(|(i, b)| match i.checked_sub(1).map(|p| bars[p].close) {
            None => b.high - b.low,
            Some(pc) => (b.high - b.low)
                .max((b.high - pc).abs())
                .max((b.low - pc).abs()),
        })
        .collect()
}

pub fn atr(bars: &[Bar], period: usize) -> Vec<f64> {
    rma(&true_range(bars), period)
}

/// Returns (%K, %D).
pub fn stochastic(bars: &[Bar], k: usize, d: usize, smooth_k: usize) -> (Vec<f64>, Vec<f64>) {
    let n = bars.len();
    let mut raw = vec![f64::NAN; n];
    if k > 0 {
        for i in k.saturating_sub(1)..n {
            let w = &bars[i + 1 - k..=i];
            let hh = w.iter().map(|b| b.high).fold(f64::MIN, f64::max);
            let ll = w.iter().map(|b| b.low).fold(f64::MAX, f64::min);
            raw[i] = if hh > ll {
                (bars[i].close - ll) / (hh - ll) * 100.0
            } else {
                50.0
            };
        }
    }
    let kk = sma(&raw, smooth_k);
    let dd = sma(&kk, d);
    (kk, dd)
}

/// VWAP that resets at each UTC day boundary.
pub fn vwap_daily(bars: &[Bar]) -> Vec<f64> {
    let mut out = Vec::with_capacity(bars.len());
    let (mut pv, mut vol, mut day) = (0.0, 0.0, i64::MIN);
    for b in bars {
        let d = b.time.div_euclid(MS_PER_DAY);
        if d != day {
            day = d;
            pv = 0.0;
            vol = 0.0;
        }
        pv += b.typical_price() * b.volume;
        vol += b.volume;
        out.push(if vol > 0.0 {
            pv / vol
        } else {
            b.typical_price()
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn sma_basic() {
        let v = sma(&[1.0, 2.0, 3.0, 4.0, 5.0], 3);
        assert!(v[0].is_nan() && v[1].is_nan());
        assert_eq!(&v[2..], &[2.0, 3.0, 4.0]);
    }

    #[test]
    fn ema_seeded_by_sma() {
        let v = ema(&[1.0, 2.0, 3.0, 4.0], 3);
        assert_eq!(v[2], 2.0);
        assert!(approx(v[3], 0.5 * 4.0 + 0.5 * 2.0));
    }

    #[test]
    fn chained_ema_skips_nan_prefix() {
        let e = ema(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 2);
        let ee = ema(&e, 2);
        assert!(ee[1].is_nan());
        assert!(!ee[2].is_nan());
    }

    #[test]
    fn rsi_extremes() {
        let up: Vec<f64> = (0..30).map(|i| i as f64).collect();
        assert_eq!(*rsi(&up, 14).last().unwrap(), 100.0);
        let down: Vec<f64> = (0..30).rev().map(|i| i as f64).collect();
        assert!(approx(*rsi(&down, 14).last().unwrap(), 0.0));
        let r = rsi(&up, 14);
        assert!(r[13].is_nan() && !r[14].is_nan());
    }

    #[test]
    fn rsi_reference_value() {
        // Classic Wilder example series.
        let c = [
            44.34, 44.09, 44.15, 43.61, 44.33, 44.83, 45.10, 45.42, 45.84, 46.08, 45.89, 46.03,
            45.61, 46.28, 46.28, 46.00, 46.03, 46.41, 46.22, 45.64,
        ];
        let r = rsi(&c, 14);
        assert!((r[14] - 70.46).abs() < 0.1, "got {}", r[14]);
    }

    #[test]
    fn bollinger_flat_series_has_zero_width() {
        let (m, u, l) = bollinger(&[5.0; 10], 5, 2.0);
        assert_eq!((m[9], u[9], l[9]), (5.0, 5.0, 5.0));
    }

    #[test]
    fn macd_histogram_is_difference() {
        let c: Vec<f64> = (0..60)
            .map(|i| (i as f64 * 0.3).sin() * 10.0 + 100.0)
            .collect();
        let (m, s, h) = macd(&c, 12, 26, 9);
        let i = 59;
        assert!(approx(h[i], m[i] - s[i]));
        assert!(s[25 + 7].is_nan() && !s[25 + 8].is_nan());
    }

    #[test]
    fn vwap_resets_daily() {
        let b = |t, p, v| Bar {
            time: t,
            open: p,
            high: p,
            low: p,
            close: p,
            volume: v,
        };
        let bars = [
            b(0, 10.0, 1.0),
            b(1000, 20.0, 1.0),
            b(MS_PER_DAY, 50.0, 2.0),
        ];
        let v = vwap_daily(&bars);
        assert_eq!(v, vec![10.0, 15.0, 50.0]);
    }

    #[test]
    fn outputs_align_with_input() {
        let bars: Vec<Bar> = (0..100)
            .map(|i| {
                let p = 100.0 + (i as f64).sin();
                Bar {
                    time: i * 60_000,
                    open: p,
                    high: p + 1.0,
                    low: p - 1.0,
                    close: p,
                    volume: 1.0,
                }
            })
            .collect();
        for ind in Indicator::catalogue() {
            let out = ind.compute(&bars);
            for p in &out.plots {
                assert_eq!(p.values.len(), bars.len(), "{}", out.label);
            }
        }
    }
}
