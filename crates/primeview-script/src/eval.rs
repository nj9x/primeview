//! Vectorized evaluator: each IR node evaluates to a whole series at once.

use std::rc::Rc;

use primeview_core::Bar;

use crate::Diagnostic;
use crate::ast::{BinOp, UnOp};
use crate::builtins::{self as bi, BSeries, Func, SERIES_COUNT, fin};
use crate::compile::{Node, check_offset};
use crate::lexer::Pos;
use crate::limits::{MAX_PERIOD, MAX_WORK};

const NAN: f64 = f64::NAN;

#[derive(Clone, Debug)]
pub(crate) enum Value {
    Scalar(f64),
    Series(Rc<Vec<f64>>),
    Tuple(Vec<Rc<Vec<f64>>>),
}

/// A scalar or a borrowed series, indexable per bar.
#[derive(Clone, Copy)]
enum Lane<'a> {
    S(f64),
    V(&'a [f64]),
}

impl Lane<'_> {
    #[inline]
    fn get(&self, i: usize) -> f64 {
        match self {
            Lane::S(x) => *x,
            Lane::V(v) => v.get(i).copied().unwrap_or(NAN),
        }
    }
}

fn lane(v: &Value) -> Lane<'_> {
    match v {
        Value::Scalar(x) => Lane::S(*x),
        Value::Series(s) => Lane::V(s),
        Value::Tuple(_) => Lane::S(NAN),
    }
}

fn scalar(v: &Value) -> f64 {
    match v {
        Value::Scalar(x) => *x,
        _ => NAN,
    }
}

#[inline]
fn truthy(x: f64) -> bool {
    !x.is_nan() && x != 0.0
}

#[inline]
fn b2f(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

fn period(v: &Value, pos: Pos) -> Result<usize, Diagnostic> {
    let x = scalar(v);
    if !x.is_finite() {
        return Err(Diagnostic::at(pos, "length must be a number, got na"));
    }
    Ok(x.round().clamp(1.0, MAX_PERIOD as f64) as usize)
}

pub(crate) struct Machine<'a> {
    bars: &'a [Bar],
    n: usize,
    inputs: &'a [f64],
    pub slots: Vec<Value>,
    cache: [Option<Rc<Vec<f64>>>; SERIES_COUNT],
    work: u64,
    /// Position used for budget errors from nodes that carry no position.
    last_pos: Pos,
}

impl<'a> Machine<'a> {
    pub fn new(bars: &'a [Bar], inputs: &'a [f64], nslots: usize) -> Self {
        Machine {
            bars,
            n: bars.len(),
            inputs,
            slots: vec![Value::Scalar(NAN); nslots],
            cache: Default::default(),
            work: 0,
            last_pos: Pos { line: 1, col: 1 },
        }
    }

    fn charge(&mut self, units: usize, pos: Pos) -> Result<(), Diagnostic> {
        self.work = self.work.saturating_add(units as u64 + 1);
        if self.work > MAX_WORK {
            return Err(Diagnostic::at(
                pos,
                "script is too expensive to run on this many bars (compute budget exceeded)",
            ));
        }
        Ok(())
    }

    fn builtin(&mut self, s: BSeries) -> Rc<Vec<f64>> {
        let k = s.index();
        if let Some(Some(v)) = self.cache.get(k) {
            return v.clone();
        }
        let v = Rc::new(s.compute(self.bars));
        if let Some(c) = self.cache.get_mut(k) {
            *c = Some(v.clone());
        }
        v
    }

    /// Materializes a value as a full series (scalars broadcast).
    pub fn to_vec(&self, v: Value) -> Vec<f64> {
        match v {
            Value::Scalar(x) => vec![fin(x); self.n],
            Value::Series(s) => {
                let mut out = Rc::try_unwrap(s).unwrap_or_else(|rc| (*rc).clone());
                out.resize(self.n, NAN);
                bi::sanitize(out)
            }
            Value::Tuple(_) => vec![NAN; self.n],
        }
    }

    fn series(&self, v: &Value) -> Rc<Vec<f64>> {
        match v {
            Value::Series(s) => s.clone(),
            other => Rc::new(vec![fin(scalar(other)); self.n]),
        }
    }

    fn map1(&self, v: &Value, f: impl Fn(f64) -> f64) -> Value {
        match v {
            Value::Series(s) => Value::Series(Rc::new(s.iter().map(|&x| fin(f(x))).collect())),
            other => Value::Scalar(fin(f(scalar(other)))),
        }
    }

    fn map2(&self, a: &Value, b: &Value, f: impl Fn(f64, f64) -> f64) -> Value {
        match (a, b) {
            (Value::Scalar(x), Value::Scalar(y)) => Value::Scalar(fin(f(*x, *y))),
            _ => {
                let (la, lb) = (lane(a), lane(b));
                Value::Series(Rc::new(
                    (0..self.n).map(|i| fin(f(la.get(i), lb.get(i)))).collect(),
                ))
            }
        }
    }

    pub fn eval(&mut self, node: &Node) -> Result<Value, Diagnostic> {
        match node {
            Node::History { pos, .. } => self.last_pos = *pos,
            Node::Call { at, .. } => self.last_pos = *at,
            _ => {}
        }
        self.charge(self.n, self.last_pos)?;
        match node {
            Node::Const(v) => Ok(Value::Scalar(fin(*v))),
            Node::Series(s) => Ok(Value::Series(self.builtin(*s))),
            Node::Slot(k) => Ok(self.slots.get(*k).cloned().unwrap_or(Value::Scalar(NAN))),
            Node::Input(k) => Ok(Value::Scalar(self.inputs.get(*k).copied().map_or(NAN, fin))),
            Node::Unary(op, x) => {
                let v = self.eval(x)?;
                Ok(match op {
                    UnOp::Neg => self.map1(&v, |x| -x),
                    UnOp::Not => self.map1(&v, |x| b2f(!truthy(x))),
                })
            }
            Node::Binary(op, a, b) => {
                let va = self.eval(a)?;
                let vb = self.eval(b)?;
                Ok(self.binary(*op, &va, &vb))
            }
            Node::Ternary(c, a, b) => {
                let vc = self.eval(c)?;
                let va = self.eval(a)?;
                let vb = self.eval(b)?;
                if let (Value::Scalar(c), Value::Scalar(x), Value::Scalar(y)) = (&vc, &va, &vb) {
                    return Ok(Value::Scalar(if truthy(*c) { *x } else { *y }));
                }
                let (lc, la, lb) = (lane(&vc), lane(&va), lane(&vb));
                Ok(Value::Series(Rc::new(
                    (0..self.n)
                        .map(|i| {
                            if truthy(lc.get(i)) {
                                la.get(i)
                            } else {
                                lb.get(i)
                            }
                        })
                        .collect(),
                )))
            }
            Node::History { src, offset, pos } => {
                let vs = self.eval(src)?;
                let vo = self.eval(offset)?;
                let off = check_offset(scalar(&vo)).map_err(|m| Diagnostic::at(*pos, m))?;
                let l = lane(&vs);
                Ok(Value::Series(Rc::new(
                    (0..self.n)
                        .map(|i| if i >= off { l.get(i - off) } else { NAN })
                        .collect(),
                )))
            }
            Node::Call {
                func,
                args,
                arg_pos,
                at,
            } => {
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.eval(a)?);
                }
                self.last_pos = *at;
                self.call(*func, &vals, arg_pos, *at)
            }
        }
    }

    fn binary(&self, op: BinOp, a: &Value, b: &Value) -> Value {
        let cmp = |f: fn(f64, f64) -> bool| {
            move |x: f64, y: f64| {
                if x.is_nan() || y.is_nan() {
                    0.0
                } else {
                    b2f(f(x, y))
                }
            }
        };
        match op {
            BinOp::Add => self.map2(a, b, |x, y| x + y),
            BinOp::Sub => self.map2(a, b, |x, y| x - y),
            BinOp::Mul => self.map2(a, b, |x, y| x * y),
            BinOp::Div => self.map2(a, b, |x, y| if y == 0.0 { NAN } else { x / y }),
            BinOp::Mod => self.map2(a, b, |x, y| if y == 0.0 { NAN } else { x % y }),
            BinOp::Lt => self.map2(a, b, cmp(|x, y| x < y)),
            BinOp::Le => self.map2(a, b, cmp(|x, y| x <= y)),
            BinOp::Gt => self.map2(a, b, cmp(|x, y| x > y)),
            BinOp::Ge => self.map2(a, b, cmp(|x, y| x >= y)),
            BinOp::Eq => self.map2(a, b, cmp(|x, y| x == y)),
            BinOp::Ne => self.map2(a, b, cmp(|x, y| x != y)),
            BinOp::And => self.map2(a, b, |x, y| b2f(truthy(x) && truthy(y))),
            BinOp::Or => self.map2(a, b, |x, y| b2f(truthy(x) || truthy(y))),
        }
    }

    fn call(
        &mut self,
        f: Func,
        vals: &[Value],
        arg_pos: &[Pos],
        at: Pos,
    ) -> Result<Value, Diagnostic> {
        let nan = Value::Scalar(NAN);
        let arg = |k: usize| vals.get(k).unwrap_or(&nan);
        let apos = |k: usize| arg_pos.get(k).copied().unwrap_or(at);
        let n = self.n;
        let series = |v: Vec<f64>| Value::Series(Rc::new(v));
        let tuple = |vs: Vec<Vec<f64>>| Value::Tuple(vs.into_iter().map(Rc::new).collect());
        Ok(match f {
            Func::Sma
            | Func::Ema
            | Func::Rma
            | Func::Wma
            | Func::Stdev
            | Func::Highest
            | Func::Lowest
            | Func::Sum
            | Func::Rsi
            | Func::Change
            | Func::Roc => {
                let src = self.series(arg(0));
                let p = period(arg(1), apos(1))?;
                let cost = match f {
                    Func::Wma | Func::Stdev => n.saturating_mul(p),
                    _ => n,
                };
                self.charge(cost, at)?;
                series(match f {
                    Func::Sma => bi::sma(&src, p),
                    Func::Ema => bi::ema(&src, p),
                    Func::Rma => bi::rma(&src, p),
                    Func::Wma => bi::wma(&src, p),
                    Func::Stdev => bi::stdev(&src, p),
                    Func::Highest => bi::highest(&src, p),
                    Func::Lowest => bi::lowest(&src, p),
                    Func::Sum => bi::sum(&src, p),
                    Func::Rsi => bi::rsi(&src, p),
                    Func::Change => bi::change(&src, p),
                    _ => bi::roc(&src, p),
                })
            }
            Func::Atr => {
                let p = period(arg(0), apos(0))?;
                series(bi::atr(self.bars, p))
            }
            Func::Tr => series(bi::true_range(self.bars)),
            Func::Crossover | Func::Crossunder => {
                let a = self.series(arg(0));
                let b = self.series(arg(1));
                series(bi::cross(&a, &b, f == Func::Crossover))
            }
            Func::Cum => series(bi::cum(&self.series(arg(0)))),
            Func::Abs => self.map1(arg(0), f64::abs),
            Func::Sqrt => self.map1(arg(0), f64::sqrt),
            Func::Log => self.map1(arg(0), f64::ln),
            Func::Log10 => self.map1(arg(0), f64::log10),
            Func::Exp => self.map1(arg(0), f64::exp),
            Func::Floor => self.map1(arg(0), f64::floor),
            Func::Ceil => self.map1(arg(0), f64::ceil),
            Func::Sign => self.map1(arg(0), |x| {
                if x > 0.0 {
                    1.0
                } else if x < 0.0 {
                    -1.0
                } else {
                    x
                }
            }),
            Func::Round => {
                let d = scalar(arg(1));
                let d = if d.is_finite() {
                    d.round().clamp(0.0, 12.0)
                } else {
                    0.0
                };
                let k = 10f64.powi(d as i32);
                self.map1(arg(0), move |x| (x * k).round() / k)
            }
            Func::Pow => self.map2(arg(0), arg(1), f64::powf),
            Func::Min | Func::Max => {
                let pick = move |x: f64, y: f64| {
                    if x.is_nan() || y.is_nan() {
                        NAN
                    } else if f == Func::Min {
                        x.min(y)
                    } else {
                        x.max(y)
                    }
                };
                let mut acc = arg(0).clone();
                for v in vals.iter().skip(1) {
                    self.charge(n, at)?;
                    acc = self.map2(&acc, v, pick);
                }
                acc
            }
            Func::Nz => self.map2(arg(0), arg(1), |x, d| if x.is_nan() { d } else { x }),
            Func::IsNa => self.map1(arg(0), |x| b2f(x.is_nan())),
            Func::Macd => {
                let src = self.series(arg(0));
                let fast = period(arg(1), apos(1))?;
                let slow = period(arg(2), apos(2))?;
                let sig = period(arg(3), apos(3))?;
                self.charge(n * 4, at)?;
                tuple(bi::macd(&src, fast, slow, sig).into())
            }
            Func::Bb => {
                let src = self.series(arg(0));
                let p = period(arg(1), apos(1))?;
                let mult = scalar(arg(2));
                self.charge(n.saturating_mul(p), at)?;
                let (m, u, l) = bi::bb(&src, p, mult);
                tuple(vec![m, u, l])
            }
            Func::Stoch => {
                let k = period(arg(0), apos(0))?;
                let smooth = period(arg(1), apos(1))?;
                let d = period(arg(2), apos(2))?;
                self.charge(n.saturating_mul(k), at)?;
                tuple(bi::stoch(self.bars, k, smooth, d).into())
            }
            Func::Supertrend => {
                let factor = scalar(arg(0));
                if !factor.is_finite() {
                    return Err(Diagnostic::at(apos(0), "supertrend factor is na"));
                }
                let p = period(arg(1), apos(1))?;
                tuple(bi::supertrend(self.bars, factor, p).into())
            }
        })
    }
}
