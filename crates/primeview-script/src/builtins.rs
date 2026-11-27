//! Built-in series, function signatures, colors and the numeric kernels.
//!
//! The kernels follow `primeview_core::indicators` exactly (same arithmetic,
//! so results are bit-identical on gap-free input) but additionally tolerate
//! `na` gaps in the middle of a series instead of being poisoned by them.

use std::collections::VecDeque;

use primeview_core::Bar;

const NAN: f64 = f64::NAN;

/// Built-in per-bar series.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BSeries {
    Open,
    High,
    Low,
    Close,
    Volume,
    Hl2,
    Hlc3,
    Ohlc4,
    Time,
    BarIndex,
    Vwap,
}

pub const SERIES_NAMES: &[(&str, BSeries)] = &[
    ("open", BSeries::Open),
    ("high", BSeries::High),
    ("low", BSeries::Low),
    ("close", BSeries::Close),
    ("volume", BSeries::Volume),
    ("hl2", BSeries::Hl2),
    ("hlc3", BSeries::Hlc3),
    ("ohlc4", BSeries::Ohlc4),
    ("time", BSeries::Time),
    ("bar_index", BSeries::BarIndex),
    ("vwap", BSeries::Vwap),
];

impl BSeries {
    pub fn from_name(n: &str) -> Option<BSeries> {
        SERIES_NAMES.iter().find(|(k, _)| *k == n).map(|(_, s)| *s)
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn compute(self, bars: &[Bar]) -> Vec<f64> {
        let raw: Vec<f64> = match self {
            BSeries::Open => bars.iter().map(|b| b.open).collect(),
            BSeries::High => bars.iter().map(|b| b.high).collect(),
            BSeries::Low => bars.iter().map(|b| b.low).collect(),
            BSeries::Close => bars.iter().map(|b| b.close).collect(),
            BSeries::Volume => bars.iter().map(|b| b.volume).collect(),
            BSeries::Hl2 => bars.iter().map(|b| (b.high + b.low) / 2.0).collect(),
            BSeries::Hlc3 => bars
                .iter()
                .map(|b| (b.high + b.low + b.close) / 3.0)
                .collect(),
            BSeries::Ohlc4 => bars
                .iter()
                .map(|b| (b.open + b.high + b.low + b.close) / 4.0)
                .collect(),
            BSeries::Time => bars.iter().map(|b| b.time as f64).collect(),
            BSeries::BarIndex => (0..bars.len()).map(|i| i as f64).collect(),
            BSeries::Vwap => primeview_core::indicators::vwap_daily(bars),
        };
        sanitize(raw)
    }
}

pub const SERIES_COUNT: usize = 11;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Func {
    Sma,
    Ema,
    Rma,
    Wma,
    Stdev,
    Highest,
    Lowest,
    Sum,
    Rsi,
    Change,
    Roc,
    Atr,
    Tr,
    Crossover,
    Crossunder,
    Cum,
    Abs,
    Sqrt,
    Log,
    Log10,
    Exp,
    Floor,
    Ceil,
    Sign,
    Round,
    Pow,
    Min,
    Max,
    Nz,
    IsNa,
    Macd,
    Bb,
    Stoch,
    Supertrend,
}

/// How an argument is typed and converted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Series input; a scalar broadcasts.
    Src,
    /// Elementwise numeric input (result is scalar if all such args are).
    Num,
    /// Window length: must be scalar; rounded and clamped to 1..=MAX_PERIOD.
    Period,
    /// Plain scalar parameter (multiplier, digits, ...).
    Scalar,
}

#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub kind: Kind,
    pub default: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ret {
    Series,
    Elementwise,
    Tuple(usize),
}

#[derive(Debug)]
pub struct Sig {
    pub name: &'static str,
    pub func: Func,
    pub params: &'static [Param],
    pub ret: Ret,
    pub variadic: bool,
}

impl Sig {
    pub fn usage(&self) -> String {
        if self.variadic {
            return format!("{}(a, b, ...)", self.name);
        }
        let ps: Vec<String> = self
            .params
            .iter()
            .map(|p| match p.default {
                Some(d) => format!("{} = {d}", p.name),
                None => p.name.to_string(),
            })
            .collect();
        format!("{}({})", self.name, ps.join(", "))
    }
}

const fn req(name: &'static str, kind: Kind) -> Param {
    Param {
        name,
        kind,
        default: None,
    }
}

const fn opt(name: &'static str, kind: Kind, d: f64) -> Param {
    Param {
        name,
        kind,
        default: Some(d),
    }
}

const SRC_LEN: &[Param] = &[req("src", Kind::Src), req("length", Kind::Period)];
const X: &[Param] = &[req("x", Kind::Num)];
const AB: &[Param] = &[req("a", Kind::Src), req("b", Kind::Src)];
const NUM_AB: &[Param] = &[req("a", Kind::Num), req("b", Kind::Num)];

const fn sig(name: &'static str, func: Func, params: &'static [Param], ret: Ret) -> Sig {
    Sig {
        name,
        func,
        params,
        ret,
        variadic: false,
    }
}

pub static SIGS: &[Sig] = &[
    sig("sma", Func::Sma, SRC_LEN, Ret::Series),
    sig("ema", Func::Ema, SRC_LEN, Ret::Series),
    sig("rma", Func::Rma, SRC_LEN, Ret::Series),
    sig("wma", Func::Wma, SRC_LEN, Ret::Series),
    sig("stdev", Func::Stdev, SRC_LEN, Ret::Series),
    sig("highest", Func::Highest, SRC_LEN, Ret::Series),
    sig("lowest", Func::Lowest, SRC_LEN, Ret::Series),
    sig("sum", Func::Sum, SRC_LEN, Ret::Series),
    sig("rsi", Func::Rsi, SRC_LEN, Ret::Series),
    sig(
        "change",
        Func::Change,
        &[req("src", Kind::Src), opt("length", Kind::Period, 1.0)],
        Ret::Series,
    ),
    sig("roc", Func::Roc, SRC_LEN, Ret::Series),
    sig(
        "atr",
        Func::Atr,
        &[req("length", Kind::Period)],
        Ret::Series,
    ),
    sig("tr", Func::Tr, &[], Ret::Series),
    sig("crossover", Func::Crossover, AB, Ret::Series),
    sig("crossunder", Func::Crossunder, AB, Ret::Series),
    sig("cum", Func::Cum, &[req("src", Kind::Src)], Ret::Series),
    sig("abs", Func::Abs, X, Ret::Elementwise),
    sig("sqrt", Func::Sqrt, X, Ret::Elementwise),
    sig("log", Func::Log, X, Ret::Elementwise),
    sig("log10", Func::Log10, X, Ret::Elementwise),
    sig("exp", Func::Exp, X, Ret::Elementwise),
    sig("floor", Func::Floor, X, Ret::Elementwise),
    sig("ceil", Func::Ceil, X, Ret::Elementwise),
    sig("sign", Func::Sign, X, Ret::Elementwise),
    sig(
        "round",
        Func::Round,
        &[req("x", Kind::Num), opt("digits", Kind::Scalar, 0.0)],
        Ret::Elementwise,
    ),
    sig(
        "pow",
        Func::Pow,
        &[req("x", Kind::Num), req("y", Kind::Num)],
        Ret::Elementwise,
    ),
    Sig {
        name: "min",
        func: Func::Min,
        params: NUM_AB,
        ret: Ret::Elementwise,
        variadic: true,
    },
    Sig {
        name: "max",
        func: Func::Max,
        params: NUM_AB,
        ret: Ret::Elementwise,
        variadic: true,
    },
    sig(
        "nz",
        Func::Nz,
        &[req("x", Kind::Num), opt("replacement", Kind::Num, 0.0)],
        Ret::Elementwise,
    ),
    sig("na", Func::IsNa, X, Ret::Elementwise),
    sig("is_na", Func::IsNa, X, Ret::Elementwise),
    sig(
        "macd",
        Func::Macd,
        &[
            req("src", Kind::Src),
            opt("fast", Kind::Period, 12.0),
            opt("slow", Kind::Period, 26.0),
            opt("signal", Kind::Period, 9.0),
        ],
        Ret::Tuple(3),
    ),
    sig(
        "bb",
        Func::Bb,
        &[
            req("src", Kind::Src),
            opt("length", Kind::Period, 20.0),
            opt("mult", Kind::Scalar, 2.0),
        ],
        Ret::Tuple(3),
    ),
    sig(
        "stoch",
        Func::Stoch,
        &[
            opt("length", Kind::Period, 14.0),
            opt("smooth", Kind::Period, 3.0),
            opt("signal", Kind::Period, 3.0),
        ],
        Ret::Tuple(2),
    ),
    sig(
        "supertrend",
        Func::Supertrend,
        &[
            opt("factor", Kind::Scalar, 3.0),
            opt("length", Kind::Period, 10.0),
        ],
        Ret::Tuple(2),
    ),
];

pub fn lookup(name: &str) -> Option<&'static Sig> {
    SIGS.iter().find(|s| s.name == name)
}

/// Names handled by the compiler rather than the signature table.
pub const STATEMENT_FNS: &[&str] = &["plot", "hline", "fill", "indicator"];

pub const COLORS: &[(&str, [u8; 4])] = &[
    ("red", [239, 83, 80, 255]),
    ("green", [38, 166, 154, 255]),
    ("blue", [41, 98, 255, 255]),
    ("orange", [255, 152, 0, 255]),
    ("purple", [156, 39, 176, 255]),
    ("teal", [0, 150, 136, 255]),
    ("gray", [120, 123, 134, 255]),
    ("grey", [120, 123, 134, 255]),
    ("white", [255, 255, 255, 255]),
    ("black", [0, 0, 0, 255]),
    ("yellow", [255, 235, 59, 255]),
    ("aqua", [0, 188, 212, 255]),
    ("lime", [0, 230, 118, 255]),
    ("fuchsia", [224, 64, 251, 255]),
    ("navy", [49, 27, 146, 255]),
    ("maroon", [136, 14, 79, 255]),
    ("silver", [178, 181, 190, 255]),
];

pub fn named_color(n: &str) -> Option<[u8; 4]> {
    COLORS.iter().find(|(k, _)| *k == n).map(|(_, c)| *c)
}

/// Default colors for plots without `color = ...`, cycled by plot index.
pub const PALETTE: &[[u8; 4]] = &[
    [41, 98, 255, 255],
    [255, 152, 0, 255],
    [156, 39, 176, 255],
    [0, 150, 136, 255],
    [239, 83, 80, 255],
    [38, 166, 154, 255],
    [255, 235, 59, 255],
    [120, 123, 134, 255],
];

// ---------------------------------------------------------------------------
// Kernels. Inputs are assumed sanitized: every value is finite or NaN.

#[inline]
pub fn fin(x: f64) -> f64 {
    if x.is_finite() { x } else { NAN }
}

#[inline]
fn ok(x: f64) -> bool {
    x.is_finite()
}

pub fn sanitize(mut v: Vec<f64>) -> Vec<f64> {
    for x in &mut v {
        *x = fin(*x);
    }
    v
}

/// Rolling sum (or mean) over full windows of valid values.
fn rolling(v: &[f64], p: usize, mean: bool) -> Vec<f64> {
    let n = v.len();
    let mut out = vec![NAN; n];
    if p == 0 {
        return out;
    }
    let mut sum = 0.0;
    let mut cnt = 0usize;
    for i in 0..n {
        if ok(v[i]) {
            sum += v[i];
            cnt += 1;
        }
        if i >= p && ok(v[i - p]) {
            sum -= v[i - p];
            cnt -= 1;
        }
        if cnt == 0 {
            sum = 0.0;
        }
        if cnt == p {
            out[i] = if mean { sum / p as f64 } else { sum };
        }
    }
    out
}

pub fn sma(v: &[f64], p: usize) -> Vec<f64> {
    rolling(v, p, true)
}

pub fn sum(v: &[f64], p: usize) -> Vec<f64> {
    rolling(v, p, false)
}

/// Exponential smoothing seeded with the SMA of the first `p` consecutive
/// valid values; later `na` values are skipped (output `na`, state kept).
fn smooth(v: &[f64], p: usize, alpha: f64) -> Vec<f64> {
    let n = v.len();
    let mut out = vec![NAN; n];
    if p == 0 {
        return out;
    }
    let mut run = 0;
    let mut seed = None;
    for (i, &x) in v.iter().enumerate() {
        if ok(x) {
            run += 1;
            if run == p {
                seed = Some(i);
                break;
            }
        } else {
            run = 0;
        }
    }
    let Some(s) = seed else {
        return out;
    };
    let mut prev = v[s + 1 - p..=s].iter().sum::<f64>() / p as f64;
    out[s] = prev;
    for i in s + 1..n {
        if ok(v[i]) {
            prev = alpha * v[i] + (1.0 - alpha) * prev;
            out[i] = prev;
        }
    }
    out
}

pub fn ema(v: &[f64], p: usize) -> Vec<f64> {
    smooth(v, p, 2.0 / (p as f64 + 1.0))
}

pub fn rma(v: &[f64], p: usize) -> Vec<f64> {
    smooth(v, p, 1.0 / p.max(1) as f64)
}

pub fn wma(v: &[f64], p: usize) -> Vec<f64> {
    let n = v.len();
    let mut out = vec![NAN; n];
    if p == 0 || p > n {
        return out;
    }
    let denom = (p * (p + 1) / 2) as f64;
    for i in p - 1..n {
        let w = &v[i + 1 - p..=i];
        if w.iter().all(|x| ok(*x)) {
            let num: f64 = w.iter().enumerate().map(|(k, x)| x * (k + 1) as f64).sum();
            out[i] = num / denom;
        }
    }
    out
}

/// Population standard deviation, computed like `indicators::bollinger`.
pub fn stdev(v: &[f64], p: usize) -> Vec<f64> {
    let mid = sma(v, p);
    let mut out = vec![NAN; v.len()];
    for i in 0..v.len() {
        if !ok(mid[i]) {
            continue;
        }
        let window = &v[i + 1 - p..=i];
        let var = window.iter().map(|x| (x - mid[i]).powi(2)).sum::<f64>() / p as f64;
        out[i] = fin(var.sqrt());
    }
    out
}

/// Returns (basis, upper, lower), matching `indicators::bollinger`.
pub fn bb(v: &[f64], p: usize, mult: f64) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let mid = sma(v, p);
    let sd = stdev(v, p);
    let upper = mid
        .iter()
        .zip(&sd)
        .map(|(m, s)| fin(m + mult * s))
        .collect();
    let lower = mid
        .iter()
        .zip(&sd)
        .map(|(m, s)| fin(m - mult * s))
        .collect();
    (mid, upper, lower)
}

fn extreme(v: &[f64], p: usize, want_max: bool) -> Vec<f64> {
    let n = v.len();
    let mut out = vec![NAN; n];
    if p == 0 {
        return out;
    }
    let mut dq: VecDeque<usize> = VecDeque::new();
    let mut last_bad: Option<usize> = None;
    for i in 0..n {
        if ok(v[i]) {
            while let Some(&b) = dq.back() {
                let dominated = if want_max { v[b] <= v[i] } else { v[b] >= v[i] };
                if !dominated {
                    break;
                }
                dq.pop_back();
            }
            dq.push_back(i);
        } else {
            last_bad = Some(i);
            dq.clear();
        }
        while let Some(&f) = dq.front() {
            if f + p > i {
                break;
            }
            dq.pop_front();
        }
        let clean = match last_bad {
            None => i + 1 >= p,
            Some(b) => i >= b + p,
        };
        if clean && let Some(&f) = dq.front() {
            out[i] = v[f];
        }
    }
    out
}

pub fn highest(v: &[f64], p: usize) -> Vec<f64> {
    extreme(v, p, true)
}

pub fn lowest(v: &[f64], p: usize) -> Vec<f64> {
    extreme(v, p, false)
}

/// Wilder RSI, matching `indicators::rsi` on gap-free input.
pub fn rsi(v: &[f64], p: usize) -> Vec<f64> {
    let n = v.len();
    let mut out = vec![NAN; n];
    if n < 2 {
        return out;
    }
    let mut gains = vec![NAN; n];
    let mut losses = vec![NAN; n];
    for i in 1..n {
        if ok(v[i]) && ok(v[i - 1]) {
            let ch = v[i] - v[i - 1];
            gains[i] = ch.max(0.0);
            losses[i] = (-ch).max(0.0);
        }
    }
    let ag = rma(&gains, p);
    let al = rma(&losses, p);
    for i in 0..n {
        if !ok(ag[i]) || !ok(al[i]) {
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

pub fn change(v: &[f64], p: usize) -> Vec<f64> {
    (0..v.len())
        .map(|i| if i >= p { fin(v[i] - v[i - p]) } else { NAN })
        .collect()
}

pub fn roc(v: &[f64], p: usize) -> Vec<f64> {
    (0..v.len())
        .map(|i| {
            if i < p || v[i - p] == 0.0 {
                return NAN;
            }
            fin(100.0 * (v[i] - v[i - p]) / v[i - p])
        })
        .collect()
}

pub fn cross(a: &[f64], b: &[f64], over: bool) -> Vec<f64> {
    let n = a.len().min(b.len());
    let mut out = vec![0.0; n];
    for i in 1..n {
        let hit = if over {
            a[i] > b[i] && a[i - 1] <= b[i - 1]
        } else {
            a[i] < b[i] && a[i - 1] >= b[i - 1]
        };
        if hit {
            out[i] = 1.0;
        }
    }
    out
}

pub fn cum(v: &[f64]) -> Vec<f64> {
    let mut s = 0.0;
    let mut started = false;
    v.iter()
        .map(|&x| {
            if ok(x) {
                s += x;
                started = true;
            }
            if started { fin(s) } else { NAN }
        })
        .collect()
}

/// Returns (macd, signal, histogram), matching `indicators::macd`.
pub fn macd(v: &[f64], fast: usize, slow: usize, signal: usize) -> [Vec<f64>; 3] {
    let f = ema(v, fast);
    let s = ema(v, slow);
    let m: Vec<f64> = f.iter().zip(&s).map(|(a, b)| fin(a - b)).collect();
    let sig = ema(&m, signal);
    let hist = m.iter().zip(&sig).map(|(a, b)| fin(a - b)).collect();
    [m, sig, hist]
}

pub fn true_range(bars: &[Bar]) -> Vec<f64> {
    sanitize(primeview_core::indicators::true_range(bars))
}

pub fn atr(bars: &[Bar], p: usize) -> Vec<f64> {
    rma(&true_range(bars), p)
}

/// Returns (%K, %D) via `indicators::stochastic`.
pub fn stoch(bars: &[Bar], length: usize, smooth: usize, signal: usize) -> [Vec<f64>; 2] {
    let (k, d) = primeview_core::indicators::stochastic(bars, length, signal, smooth);
    [sanitize(k), sanitize(d)]
}

/// Supertrend: returns (line, direction) where direction is 1 in an uptrend
/// (line acts as support below price) and -1 in a downtrend.
pub fn supertrend(bars: &[Bar], factor: f64, p: usize) -> [Vec<f64>; 2] {
    let n = bars.len();
    let a = atr(bars, p);
    let mut line = vec![NAN; n];
    let mut dir = vec![NAN; n];
    let (mut fu, mut fd, mut trend) = (NAN, NAN, 1.0);
    let mut prev_close = NAN;
    let mut started = false;
    for i in 0..n {
        let b = &bars[i];
        let c = fin(b.close);
        let hl2 = fin((b.high + b.low) / 2.0);
        if !ok(a[i]) || !ok(c) || !ok(hl2) {
            prev_close = c;
            continue;
        }
        let up = hl2 - factor * a[i];
        let dn = hl2 + factor * a[i];
        if !started {
            fu = up;
            fd = dn;
            started = true;
        } else {
            let (pu, pd) = (fu, fd);
            fu = if prev_close > pu { up.max(pu) } else { up };
            fd = if prev_close < pd { dn.min(pd) } else { dn };
            if trend < 0.0 && c > pd {
                trend = 1.0;
            } else if trend > 0.0 && c < pu {
                trend = -1.0;
            }
        }
        line[i] = fin(if trend > 0.0 { fu } else { fd });
        dir[i] = trend;
        prev_close = c;
    }
    [line, dir]
}
