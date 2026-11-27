//! Example scripts shown in the app's script editor.

/// `(name, source)` pairs of example scripts.
pub const EXAMPLES: &[(&str, &str)] = &[
    (
        "SMA Cross",
        r#"// Fast/slow simple moving averages on the price chart.
indicator("SMA Cross", overlay = true)
let fast_len = input("Fast", 9, 1, 500)
let slow_len = input("Slow", 21, 1, 500)
let fast = sma(close, fast_len)
let slow = sma(close, slow_len)
plot(fast, "Fast", color = teal)
plot(slow, "Slow", color = orange)
"#,
    ),
    (
        "RSI",
        r#"// Relative Strength Index with overbought/oversold levels.
indicator("RSI", overlay = false, range = 0..100)
let len = input("Length", 14, 1, 500)
let upper = input("Overbought", 70, 50, 100)
let lower = input("Oversold", 30, 0, 50)
plot(rsi(close, len), "RSI", color = purple)
hline(upper)
hline(50)
hline(lower)
"#,
    ),
    (
        "MACD",
        r#"// MACD line, signal line and histogram.
indicator("MACD", overlay = false)
let fast = input("Fast", 12, 1, 500)
let slow = input("Slow", 26, 1, 500)
let signal = input("Signal", 9, 1, 500)
let (line, sig, hist) = macd(close, fast, slow, signal)
plot(hist, "Histogram", color = gray, style = histogram)
plot(line, "MACD", color = blue)
plot(sig, "Signal", color = orange)
hline(0)
"#,
    ),
    (
        "Bollinger %B",
        r#"// Where the close sits inside the Bollinger Bands (0 = lower, 1 = upper).
indicator("Bollinger %B", overlay = false)
let len = input("Length", 20, 1, 500)
let mult = input("StdDev", 2, 0.1, 10)
let (basis, upper, lower) = bb(close, len, mult)
let pct_b = (close - lower) / (upper - lower)
plot(pct_b, "%B", color = teal)
hline(0)
hline(0.5)
hline(1)
"#,
    ),
    (
        "Keltner Channels",
        r#"// EMA basis with ATR-based bands, shaded between upper and lower.
indicator("Keltner Channels", overlay = true)
let len = input("Length", 20, 1, 500)
let mult = input("ATR Mult", 2, 0.1, 10)
let basis = ema(close, len)
let width = atr(len) * mult
let up = plot(basis + width, "Upper", color = #2962ff)
let dn = plot(basis - width, "Lower", color = #2962ff)
fill(up, dn)
plot(basis, "Basis", color = orange)
"#,
    ),
    (
        "Supertrend",
        r#"// ATR trailing stop that flips with the trend.
indicator("Supertrend", overlay = true)
let factor = input("Factor", 3, 0.5, 10)
let len = input("ATR Length", 10, 1, 500)
let (st, dir) = supertrend(factor, len)
plot(dir > 0 ? st : na, "Up Trend", color = green)
plot(dir < 0 ? st : na, "Down Trend", color = red)
"#,
    ),
    (
        "Donchian Channel",
        r#"// Highest high / lowest low over a lookback window.
indicator("Donchian Channel", overlay = true)
let len = input("Length", 20, 1, 1000)
let hi = highest(high, len)
let lo = lowest(low, len)
let top = plot(hi, "Upper", color = teal)
let bottom = plot(lo, "Lower", color = teal)
fill(top, bottom)
plot((hi + lo) / 2, "Middle", color = #ff980080)
"#,
    ),
    (
        "Momentum Oscillator",
        r#"// Smoothed rate of change; the histogram shows its bar-to-bar change.
indicator("Momentum Oscillator", overlay = false)
let len = input("Length", 10, 1, 500)
let smooth = input("Smoothing", 5, 1, 100)
let mom = ema(roc(close, len), smooth)
plot(mom - mom[1], "Change", style = histogram)
plot(mom, "ROC", color = blue)
hline(0)
"#,
    ),
];
