use std::collections::HashMap;

use primeview_core::indicators as ind;
use primeview_core::{Bar, Pane, PlotStyle};

use crate::lexer::{Tok, lex};
use crate::{Diagnostic, EXAMPLES, InputDef, compile};

// ---------------------------------------------------------------------------
// Helpers

fn bars(n: usize) -> Vec<Bar> {
    let mut p = 100.0;
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    (0..n)
        .map(|i| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let r = (seed % 1000) as f64 / 1000.0 - 0.5;
            let open = p;
            p += r * 2.0 + (i as f64 * 0.05).sin() * 0.3;
            let close = p;
            Bar {
                time: 1_700_000_000_000 + i as i64 * 3_600_000,
                open,
                high: open.max(close) + 0.5 + r.abs(),
                low: open.min(close) - 0.5 - r.abs(),
                close,
                volume: 1000.0 + (seed % 500) as f64,
            }
        })
        .collect()
}

fn no_inputs() -> HashMap<String, f64> {
    HashMap::new()
}

fn compile_ok(src: &str) -> crate::Script {
    match compile(src) {
        Ok(s) => s,
        Err(ds) => panic!(
            "compile failed:\n{}\n---\n{src}",
            ds.iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

fn run(src: &str, b: &[Bar]) -> primeview_core::IndicatorOutput {
    compile_ok(src)
        .run(b, &no_inputs())
        .unwrap_or_else(|d| panic!("run failed: {d}\n{src}"))
}

/// Values of the first plot of `plot(<expr>)`.
fn eval(expr: &str, b: &[Bar]) -> Vec<f64> {
    run(&format!("plot({expr})"), b).plots.remove(0).values
}

fn errors(src: &str) -> Vec<Diagnostic> {
    match compile(src) {
        Ok(_) => panic!("expected compile error for:\n{src}"),
        Err(e) => e,
    }
}

fn first_error(src: &str) -> Diagnostic {
    errors(src).remove(0)
}

#[track_caller]
fn assert_same(a: &[f64], b: &[f64]) {
    assert_eq!(a.len(), b.len(), "length mismatch");
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        let ok = (x.is_nan() && y.is_nan()) || x == y;
        assert!(ok, "mismatch at {i}: {x} vs {y}");
    }
}

#[track_caller]
fn assert_close(a: &[f64], b: &[f64]) {
    assert_eq!(a.len(), b.len(), "length mismatch");
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        let ok = (x.is_nan() && y.is_nan()) || (x - y).abs() <= 1e-9 * (1.0 + y.abs());
        assert!(ok, "mismatch at {i}: {x} vs {y}");
    }
}

fn closes(b: &[Bar]) -> Vec<f64> {
    b.iter().map(|b| b.close).collect()
}

// ---------------------------------------------------------------------------
// Lexer

#[test]
fn lexer_numbers_and_ranges() {
    let toks: Vec<Tok> = lex("1 2.5 .5 1e3 2E-2 0..100")
        .unwrap()
        .into_iter()
        .map(|t| t.tok)
        .collect();
    assert_eq!(
        toks,
        vec![
            Tok::Num(1.0),
            Tok::Num(2.5),
            Tok::Num(0.5),
            Tok::Num(1000.0),
            Tok::Num(0.02),
            Tok::Num(0.0),
            Tok::DotDot,
            Tok::Num(100.0),
            Tok::Eof
        ]
    );
}

#[test]
fn lexer_colors_strings_comments() {
    let toks: Vec<Tok> = lex("#ff0000 #00ff0080 \"a\\\"b\" // comment\nx")
        .unwrap()
        .into_iter()
        .map(|t| t.tok)
        .collect();
    assert_eq!(
        toks,
        vec![
            Tok::Color([255, 0, 0, 255]),
            Tok::Color([0, 255, 0, 128]),
            Tok::Str("a\"b".into()),
            Tok::Newline,
            Tok::Ident("x".into()),
            Tok::Eof
        ]
    );
}

#[test]
fn lexer_drops_newlines_inside_parens() {
    let toks: Vec<Tok> = lex("f(a,\n b\n)\n\n\ng")
        .unwrap()
        .into_iter()
        .map(|t| t.tok)
        .collect();
    let newlines = toks.iter().filter(|t| **t == Tok::Newline).count();
    assert_eq!(newlines, 1);
}

#[test]
fn lexer_errors_have_positions() {
    let d = lex("let x = 1\nlet y = $").unwrap_err();
    assert_eq!((d.line, d.col), (2, 9));
    let d = lex("plot(close, color = #12345)").unwrap_err();
    assert_eq!((d.line, d.col), (1, 21));
    assert!(d.message.contains("invalid color"));
    let d = lex("plot(close, \"oops)").unwrap_err();
    assert!(d.message.contains("unterminated"));
    assert!(lex("a && b").unwrap_err().message.contains("and"));
    assert!(lex("!a").unwrap_err().message.contains("not"));
    assert!(lex("12abc").unwrap_err().message.contains("invalid number"));
}

#[test]
fn lexer_keywords() {
    let toks: Vec<Tok> = lex("let and or not true false na nan")
        .unwrap()
        .into_iter()
        .map(|t| t.tok)
        .collect();
    assert_eq!(
        toks,
        vec![
            Tok::Let,
            Tok::And,
            Tok::Or,
            Tok::Not,
            Tok::True,
            Tok::False,
            Tok::Na,
            Tok::Na,
            Tok::Eof
        ]
    );
}

// ---------------------------------------------------------------------------
// Parser / operators

#[test]
fn precedence_and_associativity() {
    let b = bars(3);
    assert_eq!(eval("1 + 2 * 3", &b), vec![7.0; 3]);
    assert_eq!(eval("(1 + 2) * 3", &b), vec![9.0; 3]);
    assert_eq!(eval("10 - 4 - 3", &b), vec![3.0; 3]);
    assert_eq!(eval("2 * -3", &b), vec![-6.0; 3]);
    assert_eq!(eval("- -4", &b), vec![4.0; 3]);
    assert_eq!(eval("7 % 4", &b), vec![3.0; 3]);
    assert_eq!(eval("1 + 1 == 2", &b), vec![1.0; 3]);
    assert_eq!(eval("not 1 > 2", &b), vec![1.0; 3]);
    assert_eq!(eval("1 < 2 and 3 < 2 or true", &b), vec![1.0; 3]);
    assert_eq!(eval("0 ? 1 : 0 ? 2 : 3", &b), vec![3.0; 3]);
    assert_eq!(eval("1 ? 0 ? 5 : 6 : 7", &b), vec![6.0; 3]);
}

#[test]
fn division_by_zero_and_na_semantics() {
    let b = bars(2);
    assert!(eval("1 / 0", &b).iter().all(|v| v.is_nan()));
    assert!(eval("5 % 0", &b).iter().all(|v| v.is_nan()));
    assert!(eval("sqrt(-1)", &b).iter().all(|v| v.is_nan()));
    assert!(eval("exp(1000)", &b).iter().all(|v| v.is_nan()));
    assert_eq!(eval("na > 1", &b), vec![0.0; 2]);
    assert_eq!(eval("na != 1", &b), vec![0.0; 2]);
    assert_eq!(eval("na(na)", &b), vec![1.0; 2]);
    assert_eq!(eval("is_na(close)", &b), vec![0.0; 2]);
    assert_eq!(eval("nz(na, 7)", &b), vec![7.0; 2]);
    assert_eq!(eval("nz(na)", &b), vec![0.0; 2]);
    assert!(eval("na + 1", &b).iter().all(|v| v.is_nan()));
    assert_eq!(eval("na ? 1 : 2", &b), vec![2.0; 2]);
}

#[test]
fn multiline_and_semicolons() {
    let src = "indicator(\"X\",\n  overlay = true)\nlet a = close +\n  1; let b = a *\n 2\nplot(b,\n \"B\")";
    let b = bars(5);
    let out = run(src, &b);
    let expect: Vec<f64> = closes(&b).iter().map(|c| (c + 1.0) * 2.0).collect();
    assert_same(&out.plots[0].values, &expect);
    assert_eq!(out.plots[0].name, "B");
}

#[test]
fn ternary_is_elementwise() {
    let b = bars(50);
    let v = eval("close > open ? high : low", &b);
    for (i, x) in v.iter().enumerate() {
        let want = if b[i].close > b[i].open {
            b[i].high
        } else {
            b[i].low
        };
        assert_eq!(*x, want);
    }
}

#[test]
fn builtin_series() {
    let b = bars(20);
    assert_same(
        &eval("hl2", &b),
        &b.iter().map(|b| (b.high + b.low) / 2.0).collect::<Vec<_>>(),
    );
    assert_same(
        &eval("hlc3", &b),
        &b.iter()
            .map(|b| (b.high + b.low + b.close) / 3.0)
            .collect::<Vec<_>>(),
    );
    assert_same(
        &eval("ohlc4", &b),
        &b.iter()
            .map(|b| (b.open + b.high + b.low + b.close) / 4.0)
            .collect::<Vec<_>>(),
    );
    assert_same(
        &eval("bar_index", &b),
        &(0..20).map(|i| i as f64).collect::<Vec<_>>(),
    );
    assert_same(
        &eval("time", &b),
        &b.iter().map(|b| b.time as f64).collect::<Vec<_>>(),
    );
    assert_same(
        &eval("volume", &b),
        &b.iter().map(|b| b.volume).collect::<Vec<_>>(),
    );
    assert_same(&eval("vwap", &b), &ind::vwap_daily(&b));
}

// ---------------------------------------------------------------------------
// History operator

#[test]
fn history_operator() {
    let b = bars(10);
    let c = closes(&b);
    let v = eval("close[1]", &b);
    assert!(v[0].is_nan());
    assert_same(&v[1..], &c[..9]);
    assert_same(&eval("close[0]", &b), &c);
    assert!(eval("close[20]", &b).iter().all(|x| x.is_nan()));
    let v = eval("close - close[3]", &b);
    assert!(v[2].is_nan());
    assert_eq!(v[5], c[5] - c[2]);
    // Scalar expression offsets, including inputs.
    let s = compile_ok("let k = input(\"K\", 2)\nplot(close[k + 1])");
    let out = s.run(&b, &no_inputs()).unwrap();
    assert_same(&out.plots[0].values[3..], &c[..7]);
    // History of a history.
    let v = eval("close[1][2]", &b);
    assert_same(&v[3..], &c[..7]);
    // Rounded offsets.
    assert_same(&eval("close[1.4]", &b)[1..], &c[..9]);
}

#[test]
fn history_offset_errors() {
    let d = first_error("plot(close[-1])");
    assert!(d.message.contains("negative"), "{d}");
    assert_eq!((d.line, d.col), (1, 12));
    let d = first_error("plot(close[100000])");
    assert!(d.message.contains("too large"));
    let d = first_error("plot(close[volume])");
    assert!(d.message.contains("not a series"));
    // Runtime check for input-driven offsets.
    let s = compile_ok("let k = input(\"K\", 1)\nplot(close[k - 5])");
    let d = s.run(&bars(5), &no_inputs()).unwrap_err();
    assert!(d.message.contains("negative"));
    assert_eq!(d.line, 2);
}

// ---------------------------------------------------------------------------
// Builtins vs primeview_core reference implementations

#[test]
fn moving_averages_match_core() {
    let b = bars(300);
    let c = closes(&b);
    for len in [1, 2, 5, 14, 20, 50, 299, 300, 400] {
        assert_same(&eval(&format!("sma(close, {len})"), &b), &ind::sma(&c, len));
        assert_same(&eval(&format!("ema(close, {len})"), &b), &ind::ema(&c, len));
        assert_same(&eval(&format!("rma(close, {len})"), &b), &ind::rma(&c, len));
        assert_same(&eval(&format!("rsi(close, {len})"), &b), &ind::rsi(&c, len));
        assert_same(&eval(&format!("atr({len})"), &b), &ind::atr(&b, len));
    }
    // Chained smoothing over a leading-NaN series matches core too.
    assert_same(
        &eval("ema(ema(close, 5), 7)", &b),
        &ind::ema(&ind::ema(&c, 5), 7),
    );
    assert_same(
        &eval("sma(rsi(close, 14), 9)", &b),
        &ind::sma(&ind::rsi(&c, 14), 9),
    );
    assert_same(&eval("tr()", &b), &ind::true_range(&b));
}

#[test]
fn tuple_builtins_match_core() {
    let b = bars(200);
    let c = closes(&b);
    let out = run(
        "let (m, s, h) = macd(close, 12, 26, 9)\nplot(m)\nplot(s)\nplot(h)",
        &b,
    );
    let (m, s, h) = ind::macd(&c, 12, 26, 9);
    assert_same(&out.plots[0].values, &m);
    assert_same(&out.plots[1].values, &s);
    assert_same(&out.plots[2].values, &h);

    let out = run(
        "let (mid, up, lo) = bb(close, 20, 2.5)\nplot(mid)\nplot(up)\nplot(lo)",
        &b,
    );
    let (mid, up, lo) = ind::bollinger(&c, 20, 2.5);
    assert_same(&out.plots[0].values, &mid);
    assert_same(&out.plots[1].values, &up);
    assert_same(&out.plots[2].values, &lo);

    // Defaults: macd(close) == macd(close, 12, 26, 9).
    let out = run("let (m, _, _) = macd(close)\nplot(m)", &b);
    assert_same(&out.plots[0].values, &m);

    let out = run("let (k, d) = stoch(14, 3, 3)\nplot(k)\nplot(d)", &b);
    let (k, d) = ind::stochastic(&b, 14, 3, 3);
    assert_same(&out.plots[0].values, &k);
    assert_same(&out.plots[1].values, &d);
}

#[test]
fn stdev_matches_bollinger_width() {
    let b = bars(120);
    let c = closes(&b);
    let (mid, up, _) = ind::bollinger(&c, 20, 1.0);
    let sd: Vec<f64> = up.iter().zip(&mid).map(|(u, m)| u - m).collect();
    assert_close(&eval("stdev(close, 20)", &b), &sd);
}

#[test]
fn window_functions() {
    let b = bars(60);
    let c = closes(&b);
    let n = c.len();
    let naive = |f: &dyn Fn(&[f64]) -> f64, p: usize| -> Vec<f64> {
        (0..n)
            .map(|i| {
                if i + 1 >= p {
                    f(&c[i + 1 - p..=i])
                } else {
                    f64::NAN
                }
            })
            .collect()
    };
    for p in [1, 3, 10] {
        let hi = naive(&|w| w.iter().copied().fold(f64::MIN, f64::max), p);
        let lo = naive(&|w| w.iter().copied().fold(f64::MAX, f64::min), p);
        let sm = naive(&|w| w.iter().sum(), p);
        let wm = naive(
            &|w| {
                let num: f64 = w.iter().enumerate().map(|(k, x)| x * (k + 1) as f64).sum();
                num / (w.len() * (w.len() + 1) / 2) as f64
            },
            p,
        );
        assert_same(&eval(&format!("highest(close, {p})"), &b), &hi);
        assert_same(&eval(&format!("lowest(close, {p})"), &b), &lo);
        assert_close(&eval(&format!("sum(close, {p})"), &b), &sm);
        assert_close(&eval(&format!("wma(close, {p})"), &b), &wm);
    }
    let ch = eval("change(close)", &b);
    assert!(ch[0].is_nan());
    assert_eq!(ch[5], c[5] - c[4]);
    let ch = eval("change(close, 3)", &b);
    assert_eq!(ch[5], c[5] - c[2]);
    let r = eval("roc(close, 2)", &b);
    assert!((r[9] - 100.0 * (c[9] - c[7]) / c[7]).abs() < 1e-12);
    let cu = eval("cum(1)", &b);
    assert_eq!(cu[59], 60.0);
    let cu = eval("cum(close[1])", &b);
    assert!(cu[0].is_nan());
    assert_eq!(cu[1], c[0]);
}

#[test]
fn gaps_do_not_poison_moving_averages() {
    let b = bars(40);
    // na every 10th bar: sma recovers after each gap instead of staying na.
    let v = eval("sma(bar_index % 10 == 5 ? na : close, 3)", &b);
    assert!(v[5].is_nan() && v[6].is_nan() && v[7].is_nan());
    assert!(!v[8].is_nan() && !v[39].is_nan());
    let v = eval("ema(bar_index == 20 ? na : close, 3)", &b);
    assert!(v[20].is_nan() && !v[21].is_nan());
}

#[test]
fn math_functions() {
    let b = bars(4);
    assert_eq!(eval("abs(-2)", &b), vec![2.0; 4]);
    assert_eq!(eval("sqrt(16)", &b), vec![4.0; 4]);
    assert_eq!(eval("pow(2, 10)", &b), vec![1024.0; 4]);
    assert_eq!(eval("round(2.567, 2)", &b), vec![2.57; 4]);
    assert_eq!(eval("round(2.5)", &b), vec![3.0; 4]);
    assert_eq!(eval("floor(-1.5) + ceil(1.2)", &b), vec![0.0; 4]);
    assert_eq!(eval("sign(-3) + sign(0) + sign(5)", &b), vec![0.0; 4]);
    assert_eq!(eval("min(3, 1, 2)", &b), vec![1.0; 4]);
    assert_eq!(eval("max(3, 1, 2, 9)", &b), vec![9.0; 4]);
    assert!(eval("max(1, na)", &b).iter().all(|v| v.is_nan()));
    assert!((eval("log(exp(2))", &b)[0] - 2.0).abs() < 1e-12);
    assert_eq!(eval("log10(1000)", &b), vec![3.0; 4]);
    let c = closes(&b);
    assert_same(
        &eval("max(open, close)", &b),
        &b.iter().map(|x| x.open.max(x.close)).collect::<Vec<_>>(),
    );
    assert_same(&eval("abs(close - close)", &b), &vec![0.0; c.len()]);
}

#[test]
fn crossover_and_crossunder() {
    let b = bars(8);
    // a: 0 1 2 3 4 5 6 7 ; b = 3.5 constant -> crossover at index 4.
    let v = eval("crossover(bar_index, 3.5)", &b);
    assert_eq!(v, vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
    let v = eval("crossunder(7 - bar_index, 3.5)", &b);
    assert_eq!(v, vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
    // Touching then crossing counts once.
    let v = eval("crossover(bar_index, 3)", &b);
    assert_eq!(v, vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
    // Real data: crossover implies a > b now and a <= b one bar ago.
    let b = bars(400);
    let src =
        "let f = sma(close, 5)\nlet s = sma(close, 20)\nplot(crossover(f, s))\nplot(f)\nplot(s)";
    let out = run(src, &b);
    let (x, f, s) = (
        &out.plots[0].values,
        &out.plots[1].values,
        &out.plots[2].values,
    );
    let mut hits = 0;
    for i in 1..b.len() {
        let want = f[i] > s[i] && f[i - 1] <= s[i - 1];
        assert_eq!(x[i] == 1.0, want, "bar {i}");
        hits += usize::from(want);
    }
    assert!(hits > 0);
}

#[test]
fn supertrend_is_sane() {
    let b = bars(300);
    let out = run("let (st, dir) = supertrend(3, 10)\nplot(st)\nplot(dir)", &b);
    let (st, dir) = (&out.plots[0].values, &out.plots[1].values);
    assert!(st[8].is_nan());
    for i in 10..b.len() {
        assert!(dir[i] == 1.0 || dir[i] == -1.0);
        if dir[i] > 0.0 {
            assert!(st[i] <= b[i].close + 1e-9, "support above close at {i}");
        } else {
            assert!(st[i] >= b[i].close - 1e-9, "resistance below close at {i}");
        }
    }
}

#[test]
fn periods_are_clamped() {
    let b = bars(30);
    let c = closes(&b);
    assert_same(&eval("sma(close, 0)", &b), &ind::sma(&c, 1));
    assert_same(&eval("sma(close, -5)", &b), &ind::sma(&c, 1));
    assert_same(&eval("sma(close, 2.6)", &b), &ind::sma(&c, 3));
    assert!(eval("sma(close, 1e9)", &b).iter().all(|v| v.is_nan()));
    let d = compile_ok("plot(sma(close, na))")
        .run(&b, &no_inputs())
        .unwrap_err();
    assert!(d.message.contains("length"));
    assert_eq!((d.line, d.col), (1, 17));
}

// ---------------------------------------------------------------------------
// Inputs, header, outputs

#[test]
fn inputs_defaults_overrides_and_label() {
    let src = "indicator(\"My RSI\")\nlet len = input(\"Length\", 14, 2, 100)\nlet k = input(\"Mult\", 1.5)\nplot(rsi(close, len) * k)";
    let s = compile_ok(src);
    assert_eq!(s.title(), "My RSI");
    assert!(!s.overlay());
    assert_eq!(
        s.inputs(),
        &[
            InputDef {
                name: "Length".into(),
                default: 14.0,
                min: Some(2.0),
                max: Some(100.0)
            },
            InputDef {
                name: "Mult".into(),
                default: 1.5,
                min: None,
                max: None
            }
        ]
    );
    let b = bars(100);
    let c = closes(&b);
    let out = s.run(&b, &no_inputs()).unwrap();
    assert_eq!(out.label, "My RSI 14 1.5");
    let want: Vec<f64> = ind::rsi(&c, 14).iter().map(|v| v * 1.5).collect();
    assert_same(&out.plots[0].values, &want);

    let mut over = HashMap::new();
    over.insert("Length".to_string(), 7.0);
    over.insert("Unknown".to_string(), 1.0);
    let out = s.run(&b, &over).unwrap();
    assert_eq!(out.label, "My RSI 7 1.5");
    let want: Vec<f64> = ind::rsi(&c, 7).iter().map(|v| v * 1.5).collect();
    assert_same(&out.plots[0].values, &want);

    // Clamped to max; NaN override falls back to the default.
    over.insert("Length".to_string(), 1000.0);
    over.insert("Mult".to_string(), f64::NAN);
    let out = s.run(&b, &over).unwrap();
    assert_eq!(out.label, "My RSI 100 1.5");
}

#[test]
fn input_inline_and_negative_defaults() {
    let s = compile_ok("plot(close + input(\"Shift\", -2.5, -10, 10))");
    assert_eq!(s.inputs()[0].default, -2.5);
    assert_eq!(s.title(), "Script");
    let b = bars(3);
    let out = s.run(&b, &no_inputs()).unwrap();
    assert_eq!(out.label, "Script -2.5");
    assert_eq!(out.plots[0].values[0], b[0].close - 2.5);
}

#[test]
fn input_errors() {
    assert!(
        first_error("plot(close * input(\"A\", close))")
            .message
            .contains("number literal")
    );
    assert!(
        first_error("plot(input(\"A\", 1) + input(\"A\", 2))")
            .message
            .contains("already exists")
    );
    assert!(
        first_error("plot(input(\"A\", 50, 0, 10))")
            .message
            .contains("outside")
    );
    assert!(
        first_error("plot(input(\"A\", 5, 10, 0))")
            .message
            .contains("greater")
    );
    assert!(first_error("plot(input(14, 5))").message.contains("title"));
    assert!(
        first_error("plot(input(\"A\"))")
            .message
            .contains("missing argument `default`")
    );
    assert!(
        first_error("input(\"A\", 1)\nplot(close)")
            .message
            .contains("unused")
    );
}

#[test]
fn header_and_pane() {
    let out = run(
        "indicator(\"Osc\", overlay = false, range = -100..100)\nplot(close)\nhline(-50)\nhline(input(\"Top\", 50))",
        &bars(5),
    );
    assert_eq!(
        out.pane,
        Pane::Separate {
            range: Some((-100.0, 100.0)),
            levels: vec![-50.0, 50.0]
        }
    );
    let out = run("indicator(\"Ov\", true)\nplot(close)", &bars(5));
    assert_eq!(out.pane, Pane::Overlay);
    assert_eq!(out.label, "Ov");
    let out = run("plot(close)", &bars(5));
    assert_eq!(
        out.pane,
        Pane::Separate {
            range: None,
            levels: vec![]
        }
    );
}

#[test]
fn header_errors() {
    assert!(
        first_error("plot(close)\nindicator(\"X\")")
            .message
            .contains("first statement")
    );
    assert!(
        first_error("indicator(\"X\", overlay = true)\nhline(1)\nplot(close)")
            .message
            .contains("separate pane")
    );
    assert!(
        first_error("indicator(\"X\", overlay = true, range = 0..1)\nplot(close)")
            .message
            .contains("range")
    );
    assert!(
        first_error("indicator(\"X\", range = 5..1)\nplot(close)")
            .message
            .contains("low < high")
    );
    assert!(
        first_error("indicator(\"X\", overlay = 3)\nplot(close)")
            .message
            .contains("true")
    );
    assert!(
        first_error("indicator(42)\nplot(close)")
            .message
            .contains("string")
    );
    assert!(first_error("indicator(\"X\")").message.contains("no plot"));
}

#[test]
fn plot_options() {
    let out = run(
        "plot(close, \"A\", color = #11223344)\nplot(volume, title = \"V\", style = histogram, color = red)\nplot(1)",
        &bars(4),
    );
    assert_eq!(out.plots[0].name, "A");
    assert_eq!(out.plots[0].color, [0x11, 0x22, 0x33, 0x44]);
    assert_eq!(out.plots[0].style, PlotStyle::Line);
    assert_eq!(out.plots[1].name, "V");
    assert_eq!(out.plots[1].style, PlotStyle::Histogram);
    assert_eq!(out.plots[1].color, [239, 83, 80, 255]);
    assert_eq!(out.plots[2].name, "Plot 3");
    assert_eq!(out.plots[2].values, vec![1.0; 4]);
    assert!(
        first_error("plot(close, color = pink)")
            .message
            .contains("unknown color")
    );
    assert!(
        first_error("plot(close, style = dots)")
            .message
            .contains("style")
    );
    assert!(first_error("plot(close, 5)").message.contains("title"));
}

#[test]
fn fill_reorders_and_sets_band_fill() {
    let out = run(
        "let a = plot(high, \"H\")\nplot(close, \"C\")\nlet b = plot(low, \"L\")\nfill(a, b)",
        &bars(5),
    );
    let names: Vec<&str> = out.plots.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["H", "L", "C"]);
    assert_eq!(out.plots[0].style, PlotStyle::BandFill);
    assert_eq!(out.plots[1].style, PlotStyle::Line);

    assert!(
        first_error("let a = plot(high)\nfill(a, a)")
            .message
            .contains("itself")
    );
    assert!(
        first_error("let a = plot(high)\nfill(a, close)")
            .message
            .contains("plot handles")
    );
    assert!(
        first_error(
            "let a = plot(high)\nlet b = plot(low)\nlet c = plot(close)\nfill(a, b)\nfill(a, c)"
        )
        .message
        .contains("already filled")
    );
    assert!(
        first_error("let a = plot(high)\nlet b = plot(low)\nfill(a, b)\nfill(b, a)")
            .message
            .contains("cycle")
    );
    assert!(
        first_error("let a = plot(high)\nplot(a + 1)")
            .message
            .contains("plot handle")
    );
}

#[test]
fn lengths_always_match_bars() {
    for n in [0, 1, 2, 3, 50] {
        let b = bars(n);
        let out = run(
            "let (m, s, h) = macd(close)\nplot(m)\nplot(1)\nplot(close[3])\nplot(highest(high, 10))\nlet (st, d) = supertrend()\nplot(st)",
            &b,
        );
        for p in &out.plots {
            assert_eq!(p.values.len(), n);
        }
    }
}

#[test]
fn all_examples_compile_and_run() {
    assert!(EXAMPLES.len() >= 5);
    for n in [0, 1, 5, 30, 500] {
        let b = bars(n);
        for (name, src) in EXAMPLES {
            let s = compile(src).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            let out = s
                .run(&b, &no_inputs())
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!out.plots.is_empty(), "{name}");
            for p in &out.plots {
                assert_eq!(p.values.len(), n, "{name}/{}", p.name);
                assert!(p.values.iter().all(|v| v.is_finite() || v.is_nan()));
            }
            if n == 500 {
                for p in &out.plots {
                    assert!(
                        p.values.iter().any(|v| v.is_finite()),
                        "{name}/{} all na",
                        p.name
                    );
                }
                assert!(out.label.starts_with(s.title()));
            }
        }
    }
}

#[test]
fn example_labels() {
    let find = |n: &str| EXAMPLES.iter().find(|(k, _)| *k == n).unwrap().1;
    let out = run(find("RSI"), &bars(50));
    assert_eq!(out.label, "RSI 14 70 30");
    assert_eq!(
        out.pane,
        Pane::Separate {
            range: Some((0.0, 100.0)),
            levels: vec![70.0, 50.0, 30.0]
        }
    );
    let out = run(find("SMA Cross"), &bars(50));
    assert_eq!(out.label, "SMA Cross 9 21");
    assert_eq!(out.pane, Pane::Overlay);
    let out = run(find("Keltner Channels"), &bars(50));
    assert_eq!(out.plots[0].style, PlotStyle::BandFill);
    assert_eq!(out.plots[1].name, "Lower");
}

// ---------------------------------------------------------------------------
// Diagnostics

#[test]
fn diagnostic_positions_and_messages() {
    let d = first_error("let x = 1\nplot(clsoe)");
    assert_eq!((d.line, d.col), (2, 6));
    assert!(d.message.contains("unknown identifier `clsoe`"));
    assert!(d.message.contains("did you mean `close`"));
    assert_eq!(d.to_string(), format!("2:6: {}", d.message));

    let d = first_error("plot(smaa(close, 5))");
    assert_eq!((d.line, d.col), (1, 6));
    assert!(d.message.contains("did you mean `sma`"), "{d}");

    let d = first_error("plot(sma(close))");
    assert!(d.message.contains("missing argument `length`"), "{d}");
    let d = first_error("plot(sma(close, 1, 2))");
    assert!(d.message.contains("too many arguments"), "{d}");
    assert_eq!((d.line, d.col), (1, 20));
    let d = first_error("plot(sma(close, len = 3))");
    assert!(d.message.contains("no parameter named `len`"), "{d}");
    assert!(compile("plot(sma(length = 3, src = close))").is_ok());

    let d = first_error("plot(macd(close))");
    assert!(d.message.contains("returns 3 values"), "{d}");
    let d = first_error("let m = macd(close)\nplot(m)");
    assert!(d.message.contains("let (a, b, c)"), "{d}");
    let d = first_error("let (a, b) = macd(close)\nplot(a)");
    assert!(d.message.contains("3 values but the pattern has 2"), "{d}");
    let d = first_error("let (a, b) = close\nplot(a)");
    assert!(d.message.contains("single value"), "{d}");

    let d = first_error("plot(sma(close, volume))");
    assert!(d.message.contains("not a series"), "{d}");
    assert_eq!((d.line, d.col), (1, 17));
    let d = first_error("x = 5");
    assert!(d.message.contains("let x"), "{d}");
    let d = first_error("plot(1 < 2 < 3)");
    assert!(d.message.contains("chained"), "{d}");
    let d = first_error("let a = 1\nlet a = 2\nplot(a)");
    assert!(d.message.contains("already defined"));
    assert_eq!((d.line, d.col), (2, 5));
    let d = first_error("let close = 1\nplot(close)");
    assert!(d.message.contains("built-in"));
    let d = first_error("plot(close)\nclose + 1");
    assert!(d.message.contains("unused"));
    let d = first_error("plot(\"hi\")");
    assert!(d.message.contains("strings"));
    let d = first_error("plot(sma)");
    assert!(d.message.contains("is a function"));
    let d = first_error("plot(1 + plot(close))");
    assert!(d.message.contains("statement"));
    let d = first_error("plot(close");
    assert!(d.message.contains("expected `,` or `)`"), "{d}");
    let d = first_error("plot(min(1))");
    assert!(d.message.contains("2 to"), "{d}");
    let d = first_error("plot(tr(1))");
    assert!(d.message.contains("no arguments"), "{d}");
}

#[test]
fn multiple_errors_are_reported_without_cascades() {
    let errs = errors("let a = foo(close)\nlet b = a + 1\nplot(b)\nplot(bar)\n");
    // `b` depends on the failed `a`: no extra error for it.
    let lines: Vec<usize> = errs.iter().map(|d| d.line).collect();
    assert_eq!(lines, vec![1, 4], "{errs:?}");
    // Parse errors are collected per statement (compile checks run only
    // once the whole script parses).
    let errs = errors("plot(close +)\nplot(open *)\nlet = 3\nplot(foo)\n");
    let lines: Vec<usize> = errs.iter().map(|d| d.line).collect();
    assert_eq!(lines, vec![1, 2, 3], "{errs:?}");
}

// ---------------------------------------------------------------------------
// Safety / robustness

#[test]
fn limits_are_enforced() {
    let deep = format!("plot({}close{})", "(".repeat(5000), ")".repeat(5000));
    assert!(first_error(&deep).message.contains("nested"));
    let negs = format!("plot({}1)", "-".repeat(5000));
    assert!(compile(&negs).is_err());
    let nots = format!("plot({}1)", "not ".repeat(5000));
    assert!(compile(&nots).is_err());
    let chain = format!("plot(1{})", " + 1".repeat(3000));
    assert!(first_error(&chain).message.contains("too complex"));
    let idx = format!("plot(close{})", "[1]".repeat(3000));
    assert!(compile(&idx).is_err());
    let tern = format!("plot({}1)", "1 ? 1 : ".repeat(3000));
    assert!(compile(&tern).is_err());
    let calls = format!("plot({}close{})", "abs(".repeat(3000), ")".repeat(3000));
    assert!(compile(&calls).is_err());
    let many = "plot(close)\n".repeat(40);
    assert!(first_error(&many).message.contains("too many plots"));
    let stmts = "let _ = 1\n".repeat(300) + "plot(close)";
    assert!(first_error(&stmts).message.contains("too many statements"));
    let huge = "a".repeat(crate::limits::MAX_SOURCE_LEN + 1);
    assert!(first_error(&huge).message.contains("too long"));
    // Moderately long but legal expression still works.
    let ok = format!("plot(close{})", " + 1".repeat(50));
    let b = bars(3);
    assert_eq!(run(&ok, &b).plots[0].values[0], b[0].close + 50.0);
}

#[test]
fn compute_budget_stops_expensive_scripts() {
    let b = bars(120_000);
    let s = compile_ok("plot(wma(close, 5000))");
    let d = s.run(&b, &no_inputs()).unwrap_err();
    assert!(d.message.contains("budget"), "{d}");
    assert_eq!((d.line, d.col), (1, 6));
}

#[test]
fn hostile_bar_data() {
    let mut b = bars(60);
    b[10].close = f64::NAN;
    b[11].high = f64::INFINITY;
    b[12].low = f64::NEG_INFINITY;
    b[13].volume = f64::NAN;
    for (name, src) in EXAMPLES {
        let out = compile(src).unwrap().run(&b, &no_inputs()).unwrap();
        for p in &out.plots {
            assert_eq!(p.values.len(), 60, "{name}");
            assert!(
                p.values.iter().all(|v| v.is_finite() || v.is_nan()),
                "{name}"
            );
        }
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn try_all(src: &str, b: &[Bar]) {
    if let Ok(s) = compile(src) {
        let _ = s.title();
        let mut over = HashMap::new();
        for d in s.inputs() {
            over.insert(d.name.clone(), -1e300);
        }
        for inputs in [HashMap::new(), over] {
            if let Ok(out) = s.run(b, &inputs) {
                for p in &out.plots {
                    assert_eq!(p.values.len(), b.len(), "{src}");
                }
            }
        }
    }
}

#[test]
fn fuzz_random_bytes_never_panic() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let b = bars(40);
    for _ in 0..3000 {
        let len = rng.below(80);
        let s: String = (0..len)
            .map(|_| match rng.below(10) {
                0 => char::from_u32(rng.below(0x10000) as u32).unwrap_or('?'),
                _ => (rng.below(95) as u8 + 32) as char,
            })
            .collect();
        try_all(&s, &b);
    }
}

#[test]
fn fuzz_token_soup_never_panics() {
    const PIECES: &[&str] = &[
        "plot(",
        ")",
        "(",
        "[",
        "]",
        ",",
        "let ",
        "x",
        "y",
        " = ",
        "close",
        "open",
        "high",
        "sma(",
        "ema(",
        "macd(",
        "bb(",
        "rsi(",
        "atr(",
        "highest(",
        "input(\"A\", ",
        "\"t\"",
        "1",
        "0",
        "-5",
        "14",
        "1e9",
        "na",
        "true",
        " + ",
        " - ",
        " * ",
        " / ",
        " % ",
        " ? ",
        " : ",
        " and ",
        " or ",
        "not ",
        " < ",
        " == ",
        "\n",
        "hline(",
        "fill(",
        "indicator(",
        "color = ",
        "#ff0000",
        "red",
        "style = ",
        "histogram",
        "range = ",
        "0..100",
        "(a, b)",
        "supertrend(",
        "stoch(",
        "nz(",
        "min(",
        "cum(",
        "//c\n",
        "_",
        "..",
        "bar_index",
        "crossover(",
        "round(",
        "let (a, b, c) = ",
        "overlay = ",
        "plot(close)\n",
    ];
    let mut rng = Rng(0xdead_beef_cafe_f00d);
    let b = bars(30);
    for _ in 0..4000 {
        let k = 1 + rng.below(25);
        let s: String = (0..k).map(|_| PIECES[rng.below(PIECES.len())]).collect();
        try_all(&s, &b);
    }
}

#[test]
fn fuzz_mutated_examples_never_panic() {
    let mut rng = Rng(42);
    let b = bars(25);
    for (_, src) in EXAMPLES {
        let chars: Vec<char> = src.chars().collect();
        for _ in 0..300 {
            let mut c = chars.clone();
            for _ in 0..1 + rng.below(4) {
                if c.is_empty() {
                    break;
                }
                let i = rng.below(c.len());
                match rng.below(4) {
                    0 => {
                        c.remove(i);
                    }
                    1 => c.insert(
                        i,
                        "()[],=+-*/<>?:#\"\n0a."[rng.below(20)..]
                            .chars()
                            .next()
                            .unwrap(),
                    ),
                    2 => {
                        let j = rng.below(c.len());
                        c.swap(i, j);
                    }
                    _ => c.truncate(i),
                }
            }
            let s: String = c.into_iter().collect();
            try_all(&s, &b);
        }
    }
}

#[test]
fn language_reference_samples_compile() {
    let doc = include_str!("../LANGUAGE.md");
    let mut count = 0;
    for (k, block) in doc.split("```").enumerate() {
        if k % 2 == 1 && block.contains("plot(") {
            let s = compile_ok(block);
            let out = s.run(&bars(100), &no_inputs()).unwrap();
            assert!(out.plots.iter().all(|p| p.values.len() == 100));
            count += 1;
        }
    }
    assert!(count >= 4, "only {count} samples found");
}
