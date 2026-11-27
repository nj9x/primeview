# PrimeScript language reference

PrimeScript is PrimeView's language for custom indicators. A script is a list
of statements, one per line. Each expression is computed for every bar at
once. The language has no loops, recursion or I/O, so every script finishes.

```
// Comments start with //
indicator("My RSI", overlay = false, range = 0..100)
let len = input("Length", 14, 1, 200)
let r = rsi(close, len)
plot(r, "RSI", color = purple)
hline(70)
hline(30)
```

## Values and types

Every value is one of two kinds:

| Kind   | What it is | Examples |
|--------|------------|----------|
| scalar | one number for the whole run | `14`, `2.5`, `-1e3`, `true` (1), `false` (0), `na`, an `input(...)` |
| series | one number per bar, aligned with the bars | `close`, `sma(close, 20)`, `close - open` |

When a scalar meets a series, the scalar is repeated across every bar.
`na` (or `nan`) is the missing value. Booleans are numbers: comparisons and
logic operators return `1` or `0`. A value is *true* when it is not `na` and
not `0`.

Arithmetic that has no defined result returns `na`. That covers `x / 0`,
`x % 0`, `sqrt(-1)`, `log(0)` and overflow to infinity. Plot outputs never
contain infinities.

## Statements

| Statement | Meaning |
|-----------|---------|
| `indicator("Title", overlay = false, range = lo..hi)` | Optional header. If present it must be the first statement. `overlay = true` draws on the price chart. `range` fixes the scale of a separate pane. The second positional argument is `overlay`. |
| `let name = expr` | Binds a value. Bindings are immutable and can't be redefined. |
| `let (a, b, c) = f(...)` | Destructures a function that returns several values. Use `_` to skip one. |
| `plot(series, "Title", color = ..., style = line\|histogram)` | Outputs a plot. Only the series is required. A scalar draws a flat line. |
| `let p = plot(...)` | Same as `plot`, and also keeps a *plot handle* for `fill`. |
| `fill(p1, p2)` | Shades the area between two plot handles. |
| `hline(value)` | Draws a horizontal level in a separate pane. `value` must be a constant or an input. |

Statements end at a newline or `;`. Newlines inside `( )` and `[ ]` are
ignored, and a line can also continue after a binary operator, `?`, `:` or
`=`:

```
let band = atr(14) *
    input("Mult", 2)
plot(sma(close,
         20), "Basis")
```

`input(...)` can appear anywhere an expression can, e.g.
`plot(sma(close, input("Length", 20)))`.

## Operators (lowest to highest precedence)

| Operators | Notes |
|-----------|-------|
| `c ? a : b` | Picks per bar. Right-associative: `a ? b : c ? d : e`. |
| `or` | |
| `and` | |
| `not` | |
| `< <= > >= == !=` | Return 1/0. A comparison with `na` on either side is `0`. Chains like `a < b < c` are rejected. |
| `+ -` | |
| `* / %` | |
| unary `-` `+` | |
| `x[n]` | History: the value `n` bars ago, `na` before the first bar. |

**History.** `n` must be a scalar, either a literal or an expression built
from inputs (`close[len - 1]`). It is rounded to the nearest integer and must
be between 0 and 5000. Negative offsets would look at future bars and are
rejected. History of a scalar gives a series that is `na` for the first `n`
bars.

## Built-in series

`open high low close volume hl2 hlc3 ohlc4 time bar_index vwap`

`time` is the bar's open time in Unix milliseconds. `bar_index` counts bars
from 0. `vwap` resets at each UTC day.

## Inputs

```
input("Title", default, min, max)        // min and max are optional
input("Mult", 2, min = 0.1, max = 10)
```

Each input is a scalar parameter the user can change in the app. The title
must be a string and unique within the script. `default`, `min` and `max` must
be number literals. Values set in the app are clamped to `[min, max]`. A
non-finite value falls back to the default. The legend label is the title
followed by the input values, e.g. `RSI 14 70 30`.

## Functions

Arguments can be passed by position or by name, e.g.
`sma(src = close, length = 20)`. Named arguments must come after positional
ones. `length` arguments must be scalars. They are rounded and clamped to
`1..=5000`, and an `na` length is a run-time error.

### Moving windows (return series)

| Function | Description |
|----------|-------------|
| `sma(src, length)` | Simple moving average |
| `ema(src, length)` | Exponential MA (alpha = 2/(n+1)), seeded with the SMA |
| `rma(src, length)` | Wilder's MA (alpha = 1/n), seeded with the SMA |
| `wma(src, length)` | Linearly weighted MA |
| `stdev(src, length)` | Population standard deviation |
| `highest(src, length)` / `lowest(src, length)` | Window maximum / minimum |
| `sum(src, length)` | Rolling sum |
| `rsi(src, length)` | Wilder RSI, 0..100 |
| `change(src, length = 1)` | `src - src[length]` |
| `roc(src, length)` | Percent change over `length` bars |
| `atr(length)` | Average true range (RMA of `tr()`) |
| `tr()` | True range |
| `cum(src)` | Running total. `na` values are skipped. |
| `crossover(a, b)` / `crossunder(a, b)` | 1 on the bar where `a` crosses above / below `b`, else 0 |

A window needs `length` consecutive valid values. An `na` in the middle of a
series blanks only the windows that contain it, so later values recover.
`ema`/`rma` skip `na` bars and keep their state. On data without gaps, the
results match PrimeView's built-in indicators exactly.

### Multi-value functions (destructure with `let (...) =`)

| Function | Returns |
|----------|---------|
| `macd(src, fast = 12, slow = 26, signal = 9)` | `(macd, signal, histogram)` |
| `bb(src, length = 20, mult = 2)` | `(basis, upper, lower)` |
| `stoch(length = 14, smooth = 3, signal = 3)` | `(k, d)` |
| `supertrend(factor = 3, length = 10)` | `(line, direction)`. Direction is `1` in an uptrend (the line is support under the price) and `-1` in a downtrend. |

```
let (m, s, h) = macd(close, 12, 26, 9)
let (basis, _, _) = bb(close)
```

A tuple can't be used as a value. `plot(macd(close))` is a type error.

### Elementwise math

`abs(x) sqrt(x) log(x) log10(x) exp(x) floor(x) ceil(x) sign(x)`,
`round(x, digits = 0)`, `pow(x, y)`, `min(a, b, ...)` and `max(a, b, ...)`
(2 to 8 arguments; `na` if any argument is `na`), `nz(x, replacement = 0)`,
and `na(x)` / `is_na(x)` (1 when `x` is `na`). These return a scalar when all
inputs are scalars and a series otherwise. `log` is the natural log.

## Plot options

* `color`: `#rrggbb`, `#rrggbbaa`, or one of `red green blue orange purple
  teal gray grey white black yellow aqua lime fuchsia navy maroon silver`.
  Plots without a color take the next color from a default palette.
* `style`: `line` (default) or `histogram`. The app colors histogram bars by
  sign and slope.
* Titles default to `Plot 1`, `Plot 2`, ….
* `fill(a, b)` turns plot `a` into a band fill and moves `b` directly after
  it in the output. A plot can fill to only one other plot, and can be the
  target of only one fill. Histograms can't be filled.

## Limits

| Limit | Value |
|-------|-------|
| Source size | 64 KiB, 20 000 tokens |
| Statements | 256 |
| Plots / hlines / inputs | 32 / 16 / 32 |
| Expression depth / nesting | 64 / 48 |
| Window lengths | clamped to 1..=5000 |
| History offset | 0..=5000 |
| Strings | 256 characters |
| Compute budget | about 5×10⁸ value operations per run; `wma`/`stdev` cost `bars × length` |

A script over the compute budget stops with a diagnostic and doesn't hang.

## Diagnostics

Errors have a 1-based `line:col` and a message. Parse errors are reported for
each statement, up to 20. Type errors are reported once the script parses.
Some examples:

```
2:6: unknown identifier `clsoe`; did you mean `close`?
1:6: `sma` is missing argument `length`; usage: sma(src, length)
1:6: this expression returns 3 values; destructure them with `let (a, b, c) = ...`
1:17: `length` of sma(...) must be a constant or input, not a series
1:12: history offset can't be negative (no peeking at future bars)
```

## More examples

```
// Keltner Channels
indicator("Keltner Channels", overlay = true)
let len = input("Length", 20, 1, 500)
let basis = ema(close, len)
let width = atr(len) * input("ATR Mult", 2, 0.1, 10)
let up = plot(basis + width, "Upper", color = #2962ff)
let dn = plot(basis - width, "Lower", color = #2962ff)
fill(up, dn)
plot(basis, "Basis", color = orange)
```

```
// Supertrend, colored by direction
indicator("Supertrend", overlay = true)
let (st, dir) = supertrend(input("Factor", 3), input("ATR Length", 10))
plot(dir > 0 ? st : na, "Up", color = green)
plot(dir < 0 ? st : na, "Down", color = red)
```

The app's script editor includes these and more (`primeview_script::EXAMPLES`).
