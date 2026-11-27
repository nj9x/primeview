//! PrimeScript: a small, safe, vectorized language for custom indicators.
//!
//! ```text
//! indicator("My RSI", overlay = false, range = 0..100)
//! let len = input("Length", 14, 1, 200)
//! plot(rsi(close, len), "RSI", color = purple)
//! hline(70)
//! hline(30)
//! ```
//!
//! Scripts are compiled once with [`compile`] and run against a bar array with
//! [`Script::run`]. Every expression evaluates to a whole series at once.
//! There are no loops, recursion or I/O, inputs are bounded by the limits in
//! [`limits`], and evaluation has a compute budget, so a script always
//! terminates. See `LANGUAGE.md` for the language reference.

mod ast;
mod builtins;
mod compile;
mod eval;
mod examples;
mod lexer;
mod parser;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fmt;

use primeview_core::{Bar, IndicatorOutput, Pane, Plot};

pub use examples::EXAMPLES;

use crate::compile::{Op, PlotMeta};
use crate::eval::{Machine, Value};
use crate::lexer::Pos;

/// Hard limits that keep any script cheap and bounded.
pub mod limits {
    /// Maximum source size in bytes.
    pub const MAX_SOURCE_LEN: usize = 64 * 1024;
    pub const MAX_TOKENS: usize = 20_000;
    pub const MAX_STATEMENTS: usize = 256;
    pub const MAX_PLOTS: usize = 32;
    pub const MAX_HLINES: usize = 16;
    pub const MAX_INPUTS: usize = 32;
    /// Maximum height of an expression tree.
    pub const MAX_EXPR_DEPTH: usize = 64;
    /// Maximum nesting of parentheses / unary operators / calls.
    pub const MAX_NEST: usize = 48;
    pub const MAX_ARGS: usize = 16;
    /// Maximum arguments to `min` / `max`.
    pub const MAX_VARIADIC: usize = 8;
    pub const MAX_STRING_LEN: usize = 256;
    /// Window lengths are clamped to `1..=MAX_PERIOD`.
    pub const MAX_PERIOD: usize = 5000;
    /// History offsets `x[n]` must be in `0..=MAX_OFFSET`.
    pub const MAX_OFFSET: usize = 5000;
    /// Diagnostics reported per compile.
    pub const MAX_ERRORS: usize = 20;
    /// Abstract work units per run (roughly one per value computed).
    pub const MAX_WORK: u64 = 500_000_000;
}

/// A compile or run-time error with a 1-based source position.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

impl Diagnostic {
    pub(crate) fn at(pos: Pos, message: impl Into<String>) -> Self {
        Diagnostic {
            line: pos.line,
            col: pos.col,
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)
    }
}

impl std::error::Error for Diagnostic {}

/// A user-tunable numeric parameter declared with `input(...)`.
#[derive(Clone, Debug, PartialEq)]
pub struct InputDef {
    pub name: String,
    pub default: f64,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

/// A compiled script.
#[derive(Clone, Debug)]
pub struct Script {
    title: String,
    overlay: bool,
    range: Option<(f64, f64)>,
    inputs: Vec<InputDef>,
    ops: Vec<Op>,
    nslots: usize,
    plots: Vec<PlotMeta>,
    /// Output order of `plots` (fills move their partner next to them).
    order: Vec<usize>,
}

/// Compiles PrimeScript source. On failure returns every diagnostic found
/// (up to [`limits::MAX_ERRORS`]), in source order per phase.
pub fn compile(src: &str) -> Result<Script, Vec<Diagnostic>> {
    if src.len() > limits::MAX_SOURCE_LEN {
        return Err(vec![Diagnostic {
            line: 1,
            col: 1,
            message: format!(
                "script is too long ({} bytes, limit {})",
                src.len(),
                limits::MAX_SOURCE_LEN
            ),
        }]);
    }
    let toks = lexer::lex(src).map_err(|d| vec![d])?;
    let stmts = parser::parse(toks)?;
    compile::compile(&stmts)
}

fn format_value(v: f64) -> String {
    if v == 0.0 {
        "0".to_string()
    } else {
        format!("{v}")
    }
}

impl Script {
    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn overlay(&self) -> bool {
        self.overlay
    }

    pub fn inputs(&self) -> &[InputDef] {
        &self.inputs
    }

    /// Effective input value: the override (clamped to min/max) if present and
    /// finite, otherwise the default.
    fn input_value(def: &InputDef, over: Option<&f64>) -> f64 {
        match over {
            Some(&v) if v.is_finite() => {
                let mut v = v;
                if let Some(lo) = def.min {
                    v = v.max(lo);
                }
                if let Some(hi) = def.max {
                    v = v.min(hi);
                }
                v
            }
            _ => def.default,
        }
    }

    /// Runs the script over `bars`. `inputs` overrides defaults by name;
    /// missing names use defaults. Every plot has `values.len() == bars.len()`.
    pub fn run(
        &self,
        bars: &[Bar],
        inputs: &HashMap<String, f64>,
    ) -> Result<IndicatorOutput, Diagnostic> {
        let vals: Vec<f64> = self
            .inputs
            .iter()
            .map(|d| Self::input_value(d, inputs.get(&d.name)))
            .collect();
        let mut m = Machine::new(bars, &vals, self.nslots);
        let mut plot_vals: Vec<Option<Vec<f64>>> = vec![None; self.plots.len()];
        let mut levels = Vec::new();
        for op in &self.ops {
            match op {
                Op::Let { slots, value } => match m.eval(value)? {
                    Value::Tuple(parts) => {
                        for (slot, part) in slots.iter().zip(parts) {
                            if let Some(s) = slot
                                && let Some(dst) = m.slots.get_mut(*s)
                            {
                                *dst = Value::Series(part);
                            }
                        }
                    }
                    v => {
                        if let Some(Some(s)) = slots.first()
                            && let Some(dst) = m.slots.get_mut(*s)
                        {
                            *dst = v;
                        }
                    }
                },
                Op::Plot { index, value } => {
                    let v = m.eval(value)?;
                    if let Some(dst) = plot_vals.get_mut(*index) {
                        *dst = Some(m.to_vec(v));
                    }
                }
                Op::Hline { value, pos } => {
                    let v = match m.eval(value)? {
                        Value::Scalar(x) => x,
                        _ => f64::NAN,
                    };
                    if !v.is_finite() {
                        return Err(Diagnostic::at(*pos, "hline value is na"));
                    }
                    levels.push(v);
                }
            }
        }
        let plots = self
            .order
            .iter()
            .filter_map(|&k| {
                let meta = self.plots.get(k)?;
                let values = plot_vals
                    .get_mut(k)
                    .and_then(Option::take)
                    .unwrap_or_else(|| vec![f64::NAN; bars.len()]);
                Some(Plot {
                    name: meta.title.clone(),
                    values,
                    style: meta.style,
                    color: meta.color,
                })
            })
            .collect();
        let mut label = self.title.clone();
        for v in &vals {
            label.push(' ');
            label.push_str(&format_value(*v));
        }
        let pane = if self.overlay {
            Pane::Overlay
        } else {
            Pane::Separate {
                range: self.range,
                levels,
            }
        };
        Ok(IndicatorOutput { label, pane, plots })
    }
}
