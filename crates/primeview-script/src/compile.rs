//! Type checking and lowering from the AST to a small evaluation IR.
//!
//! Types are static: every value expression is either a *scalar* (one number
//! per run, e.g. a literal or an input; broadcasts over bars) or a *series*
//! (one number per bar). Tuples come from multi-output builtins and must be
//! destructured with `let (a, b) = ...`. Plot handles exist only as names
//! bound by `let p = plot(...)` and are consumed by `fill`.

use std::collections::HashMap;

use primeview_core::PlotStyle;

use crate::ast::{Arg, BinOp, Expr, ExprKind, Stmt, UnOp};
use crate::builtins::{self, BSeries, Func, Kind, Ret, SERIES_NAMES, SIGS, STATEMENT_FNS};
use crate::lexer::Pos;
use crate::limits::{MAX_ERRORS, MAX_HLINES, MAX_INPUTS, MAX_OFFSET, MAX_PLOTS, MAX_VARIADIC};
use crate::{Diagnostic, InputDef, Script};

#[derive(Clone, Debug)]
pub(crate) enum Node {
    Const(f64),
    Series(BSeries),
    Slot(usize),
    Input(usize),
    Unary(UnOp, Box<Node>),
    Binary(BinOp, Box<Node>, Box<Node>),
    Ternary(Box<Node>, Box<Node>, Box<Node>),
    History {
        src: Box<Node>,
        offset: Box<Node>,
        pos: Pos,
    },
    Call {
        func: Func,
        args: Vec<Node>,
        arg_pos: Vec<Pos>,
        at: Pos,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum Op {
    Let {
        slots: Vec<Option<usize>>,
        value: Node,
    },
    Plot {
        index: usize,
        value: Node,
    },
    Hline {
        value: Node,
        pos: Pos,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct PlotMeta {
    pub title: String,
    pub color: [u8; 4],
    pub style: PlotStyle,
    pub pos: Pos,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ty {
    Scalar,
    Series,
    Tuple(usize),
}

enum Binding {
    Value {
        slot: usize,
        ty: Ty,
    },
    Plot(usize),
    /// Name from a statement that failed to compile; suppresses follow-up errors.
    Poisoned,
}

/// Error already recorded in `Compiler::errors`.
struct Reported;
type CResult<T> = Result<T, Reported>;

struct Compiler {
    errors: Vec<Diagnostic>,
    env: HashMap<String, Binding>,
    nslots: usize,
    inputs: Vec<InputDef>,
    plots: Vec<PlotMeta>,
    fills: Vec<(usize, usize, Pos)>,
    hlines: usize,
    ops: Vec<Op>,
    title: Option<String>,
    overlay: bool,
    range: Option<(f64, f64)>,
    stmt_index: usize,
}

pub(crate) fn compile(stmts: &[Stmt]) -> Result<Script, Vec<Diagnostic>> {
    let mut c = Compiler {
        errors: Vec::new(),
        env: HashMap::new(),
        nslots: 0,
        inputs: Vec::new(),
        plots: Vec::new(),
        fills: Vec::new(),
        hlines: 0,
        ops: Vec::new(),
        title: None,
        overlay: false,
        range: None,
        stmt_index: 0,
    };
    for (k, s) in stmts.iter().enumerate() {
        c.stmt_index = k;
        let _ = c.stmt(s);
        if c.errors.len() >= MAX_ERRORS {
            break;
        }
    }
    if c.plots.is_empty() && c.errors.is_empty() {
        c.error(
            Pos { line: 1, col: 1 },
            "script has no plot(...) statement; add one, e.g. plot(close)",
        );
    }
    let order = c.resolve_fills();
    if !c.errors.is_empty() {
        c.errors.truncate(MAX_ERRORS);
        return Err(c.errors);
    }
    Ok(Script {
        title: c.title.unwrap_or_else(|| "Script".to_string()),
        overlay: c.overlay,
        range: c.range,
        inputs: c.inputs,
        ops: c.ops,
        nslots: c.nslots,
        plots: c.plots,
        order,
    })
}

/// Evaluates literal arithmetic (`-5`, `100 / 2`) at compile time.
fn const_eval(e: &Expr) -> Option<f64> {
    match &e.kind {
        ExprKind::Num(v) => Some(*v),
        ExprKind::Bool(b) => Some(f64::from(u8::from(*b))),
        ExprKind::Unary(UnOp::Neg, x) => const_eval(x).map(|v| -v),
        ExprKind::Binary(op, a, b) => {
            let (a, b) = (const_eval(a)?, const_eval(b)?);
            match op {
                BinOp::Add => Some(a + b),
                BinOp::Sub => Some(a - b),
                BinOp::Mul => Some(a * b),
                BinOp::Div => Some(a / b),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Validates a history offset; shared by compile-time and run-time checks.
pub(crate) fn check_offset(v: f64) -> Result<usize, String> {
    if !v.is_finite() {
        return Err("history offset is na".into());
    }
    let r = v.round();
    if r < 0.0 {
        return Err("history offset can't be negative (no peeking at future bars)".into());
    }
    if r > MAX_OFFSET as f64 {
        return Err(format!(
            "history offset {r} is too large (limit {MAX_OFFSET})"
        ));
    }
    Ok(r as usize)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len() > 64 || b.len() > 64 {
        return usize::MAX;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

fn suggest<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> String {
    let best = candidates
        .map(|c| (edit_distance(name, c), c))
        .filter(|(d, c)| *d <= 2 && *d < name.len().max(2) && *c != name)
        .min_by_key(|(d, _)| *d);
    match best {
        Some((_, c)) => format!("; did you mean `{c}`?"),
        None => String::new(),
    }
}

fn tuple_hint(k: usize) -> String {
    let names: Vec<&str> = ["a", "b", "c", "d", "e", "f"]
        .iter()
        .take(k)
        .copied()
        .collect();
    format!(
        "this expression returns {k} values; destructure them with `let ({}) = ...`",
        names.join(", ")
    )
}

impl Compiler {
    fn error(&mut self, pos: Pos, msg: impl Into<String>) {
        self.errors.push(Diagnostic::at(pos, msg));
    }

    fn err<T>(&mut self, pos: Pos, msg: impl Into<String>) -> CResult<T> {
        self.error(pos, msg);
        Err(Reported)
    }

    fn alloc(&mut self) -> usize {
        self.nslots += 1;
        self.nslots - 1
    }

    fn stmt(&mut self, s: &Stmt) -> CResult<()> {
        match s {
            Stmt::Let {
                names,
                tuple,
                value,
            } => self.let_stmt(names, *tuple, value),
            Stmt::Expr(e) => self.expr_stmt(e),
        }
    }

    fn check_new_name(&mut self, n: &str, p: Pos) -> CResult<()> {
        if BSeries::from_name(n).is_some() {
            return self.err(
                p,
                format!("`{n}` is a built-in series and can't be redefined"),
            );
        }
        match self.env.get(n) {
            Some(Binding::Value { .. } | Binding::Plot(_)) => self.err(
                p,
                format!("`{n}` is already defined; bindings are immutable, so pick a new name"),
            ),
            _ => Ok(()),
        }
    }

    fn let_stmt(&mut self, names: &[(String, Pos)], tuple: bool, value: &Expr) -> CResult<()> {
        let mut ok = true;
        let mut seen: Vec<&str> = Vec::new();
        for (n, p) in names {
            if n == "_" {
                continue;
            }
            if seen.contains(&n.as_str()) {
                self.error(*p, format!("`{n}` appears twice in this pattern"));
                ok = false;
                continue;
            }
            seen.push(n);
            if self.check_new_name(n, *p).is_err() {
                ok = false;
            }
        }
        let res = if ok {
            self.let_value(names, tuple, value)
        } else {
            Err(Reported)
        };
        if res.is_err() {
            for (n, _) in names {
                if n != "_" && BSeries::from_name(n).is_none() {
                    self.env.entry(n.clone()).or_insert(Binding::Poisoned);
                }
            }
        }
        res
    }

    fn let_value(&mut self, names: &[(String, Pos)], tuple: bool, value: &Expr) -> CResult<()> {
        if !tuple {
            let name = &names[0].0;
            if let ExprKind::Call(f, args) = &value.kind
                && f == "plot"
            {
                let idx = self.plot(args, value.pos)?;
                if name != "_" {
                    self.env.insert(name.clone(), Binding::Plot(idx));
                }
                return Ok(());
            }
            let (node, ty) = self.expr(value)?;
            if let Ty::Tuple(k) = ty {
                return self.err(value.pos, tuple_hint(k));
            }
            let slot = self.alloc();
            self.ops.push(Op::Let {
                slots: vec![Some(slot)],
                value: node,
            });
            if name != "_" {
                self.env.insert(name.clone(), Binding::Value { slot, ty });
            }
            return Ok(());
        }
        let (node, ty) = self.expr(value)?;
        match ty {
            Ty::Tuple(k) if k == names.len() => {
                let mut slots = Vec::with_capacity(k);
                for (n, _) in names {
                    if n == "_" {
                        slots.push(None);
                    } else {
                        let slot = self.alloc();
                        self.env.insert(
                            n.clone(),
                            Binding::Value {
                                slot,
                                ty: Ty::Series,
                            },
                        );
                        slots.push(Some(slot));
                    }
                }
                self.ops.push(Op::Let { slots, value: node });
                Ok(())
            }
            Ty::Tuple(k) => self.err(
                value.pos,
                format!(
                    "this expression returns {k} values but the pattern has {} names",
                    names.len()
                ),
            ),
            _ => self.err(
                value.pos,
                "this expression returns a single value; use `let name = ...` without parentheses",
            ),
        }
    }

    fn expr_stmt(&mut self, e: &Expr) -> CResult<()> {
        if let ExprKind::Call(f, args) = &e.kind {
            match f.as_str() {
                "plot" => return self.plot(args, e.pos).map(|_| ()),
                "hline" => return self.hline(args, e.pos),
                "fill" => return self.fill(args, e.pos),
                "indicator" => return self.indicator(args, e.pos),
                "input" => {
                    return self.err(
                        e.pos,
                        "the value of input(...) is unused; bind it, e.g. `let length = input(\"Length\", 14)`",
                    );
                }
                _ => {}
            }
        }
        self.expr(e)?;
        self.err(
            e.pos,
            "the result of this expression is unused; did you mean plot(...)?",
        )
    }

    /// Matches call arguments to parameter names. Returns one slot per param.
    fn bind<'a>(
        &mut self,
        fname: &str,
        args: &'a [Arg],
        params: &[&str],
        required: usize,
        at: Pos,
        usage: &str,
    ) -> CResult<Vec<Option<&'a Expr>>> {
        let mut out: Vec<Option<&Expr>> = vec![None; params.len()];
        let mut seen_named = false;
        let mut positional = 0;
        for a in args {
            match &a.name {
                None => {
                    if seen_named {
                        return self.err(
                            a.value.pos,
                            "positional arguments must come before named ones",
                        );
                    }
                    if positional >= params.len() {
                        let msg = if params.is_empty() {
                            format!("`{fname}` takes no arguments: {usage}")
                        } else {
                            format!(
                                "too many arguments: `{fname}` takes at most {}: {usage}",
                                params.len()
                            )
                        };
                        return self.err(a.value.pos, msg);
                    }
                    out[positional] = Some(&a.value);
                    positional += 1;
                }
                Some((n, p)) => {
                    seen_named = true;
                    let Some(k) = params.iter().position(|x| x == n) else {
                        let msg = if params.is_empty() {
                            format!("`{fname}` takes no arguments")
                        } else {
                            format!(
                                "`{fname}` has no parameter named `{n}` (parameters: {})",
                                params.join(", ")
                            )
                        };
                        return self.err(*p, msg);
                    };
                    if out[k].is_some() {
                        return self.err(*p, format!("argument `{n}` is given twice"));
                    }
                    out[k] = Some(&a.value);
                }
            }
        }
        for k in 0..required.min(params.len()) {
            if out[k].is_none() {
                return self.err(
                    at,
                    format!(
                        "`{fname}` is missing argument `{}`; usage: {usage}",
                        params[k]
                    ),
                );
            }
        }
        Ok(out)
    }

    fn indicator(&mut self, args: &[Arg], at: Pos) -> CResult<()> {
        if self.title.is_some() {
            return self.err(at, "indicator(...) can only appear once");
        }
        if self.stmt_index != 0 {
            return self.err(
                at,
                "indicator(...) must be the first statement of the script",
            );
        }
        let usage = "indicator(\"Title\", overlay = false, range = 0..100)";
        let a = self.bind(
            "indicator",
            args,
            &["title", "overlay", "range"],
            1,
            at,
            usage,
        )?;
        let title = match a[0].map(|e| &e.kind) {
            Some(ExprKind::Str(s)) => s.clone(),
            _ => {
                let p = a[0].map_or(at, |e| e.pos);
                return self.err(
                    p,
                    "indicator title must be a string, e.g. indicator(\"My Script\")",
                );
            }
        };
        let overlay = match a[1] {
            None => false,
            Some(Expr {
                kind: ExprKind::Bool(b),
                ..
            }) => *b,
            Some(e) => return self.err(e.pos, "overlay must be `true` or `false`"),
        };
        let mut range = None;
        if let Some(e) = a[2] {
            let ExprKind::Range(lo, hi) = &e.kind else {
                return self.err(e.pos, "range must look like `range = 0..100`");
            };
            let (Some(l), Some(h)) = (const_eval(lo), const_eval(hi)) else {
                return self.err(e.pos, "range bounds must be number literals, e.g. 0..100");
            };
            if !(l.is_finite() && h.is_finite() && l < h) {
                return self.err(e.pos, "range must satisfy low < high, e.g. 0..100");
            }
            if overlay {
                return self.err(
                    e.pos,
                    "range only applies to a separate pane; remove it or set overlay = false",
                );
            }
            range = Some((l, h));
        }
        self.title = Some(title);
        self.overlay = overlay;
        self.range = range;
        Ok(())
    }

    fn plot(&mut self, args: &[Arg], at: Pos) -> CResult<usize> {
        let usage = "plot(series, \"Title\", color = #2962ff, style = line)";
        let a = self.bind(
            "plot",
            args,
            &["series", "title", "color", "style"],
            1,
            at,
            usage,
        )?;
        if self.plots.len() >= MAX_PLOTS {
            return self.err(at, format!("too many plots (limit {MAX_PLOTS})"));
        }
        let Some(series) = a[0] else {
            return Err(Reported);
        };
        let (node, _) = self.value(series)?;
        let index = self.plots.len();
        let title = match a[1] {
            None => format!("Plot {}", index + 1),
            Some(Expr {
                kind: ExprKind::Str(s),
                ..
            }) => s.clone(),
            Some(e) => return self.err(e.pos, "plot title must be a string, e.g. \"Signal\""),
        };
        let color = match a[2] {
            None => builtins::PALETTE[index % builtins::PALETTE.len()],
            Some(e) => self.color(e)?,
        };
        let style = match a[3] {
            None => PlotStyle::Line,
            Some(e) => {
                let word = match &e.kind {
                    ExprKind::Ident(s) | ExprKind::Str(s) => s.as_str(),
                    _ => "",
                };
                match word {
                    "line" => PlotStyle::Line,
                    "histogram" => PlotStyle::Histogram,
                    _ => {
                        return self.err(
                            e.pos,
                            "unknown plot style; use `style = line` or `style = histogram`",
                        );
                    }
                }
            }
        };
        self.plots.push(PlotMeta {
            title,
            color,
            style,
            pos: at,
        });
        self.ops.push(Op::Plot { index, value: node });
        Ok(index)
    }

    fn color(&mut self, e: &Expr) -> CResult<[u8; 4]> {
        match &e.kind {
            ExprKind::Color(c) => Ok(*c),
            ExprKind::Ident(n) => match builtins::named_color(n) {
                Some(c) => Ok(c),
                None => {
                    let hint = suggest(n, builtins::COLORS.iter().map(|(k, _)| *k));
                    self.err(
                        e.pos,
                        format!("unknown color `{n}`{hint} (use #rrggbb, #rrggbbaa or a name like red, green, blue)"),
                    )
                }
            },
            _ => self.err(
                e.pos,
                "color must be a literal like #2962ff or a name like teal",
            ),
        }
    }

    fn hline(&mut self, args: &[Arg], at: Pos) -> CResult<()> {
        let a = self.bind("hline", args, &["value"], 1, at, "hline(value)")?;
        if self.overlay {
            return self.err(
                at,
                "hline(...) needs a separate pane; set overlay = false in indicator(...) or plot a constant instead",
            );
        }
        if self.hlines >= MAX_HLINES {
            return self.err(at, format!("too many hlines (limit {MAX_HLINES})"));
        }
        let Some(e) = a[0] else {
            return Err(Reported);
        };
        let (node, ty) = self.value(e)?;
        if ty != Ty::Scalar {
            return self.err(
                e.pos,
                "hline value must be a constant or input, not a series",
            );
        }
        self.hlines += 1;
        self.ops.push(Op::Hline {
            value: node,
            pos: e.pos,
        });
        Ok(())
    }

    fn plot_handle(&mut self, e: Option<&Expr>, at: Pos) -> CResult<usize> {
        let hint = "fill(...) takes plot handles: `let a = plot(...)`, then `fill(a, b)`";
        let Some(e) = e else {
            return self.err(at, hint);
        };
        if let ExprKind::Ident(n) = &e.kind {
            match self.env.get(n) {
                Some(Binding::Plot(i)) => return Ok(*i),
                Some(Binding::Poisoned) => return Err(Reported),
                _ => {}
            }
        }
        self.err(e.pos, hint)
    }

    fn fill(&mut self, args: &[Arg], at: Pos) -> CResult<()> {
        let a = self.bind(
            "fill",
            args,
            &["plot1", "plot2"],
            2,
            at,
            "fill(plot1, plot2)",
        )?;
        let i = self.plot_handle(a[0], at)?;
        let j = self.plot_handle(a[1], at)?;
        if i == j {
            return self.err(at, "can't fill a plot with itself");
        }
        self.fills.push((i, j, at));
        Ok(())
    }

    /// Applies fills: the first plot becomes a BandFill and the second plot is
    /// moved right after it (the renderer fills to the next plot in the list).
    fn resolve_fills(&mut self) -> Vec<usize> {
        let n = self.plots.len();
        let mut next: Vec<Option<usize>> = vec![None; n];
        let mut prev: Vec<Option<usize>> = vec![None; n];
        let fills = std::mem::take(&mut self.fills);
        for &(a, b, pos) in &fills {
            if a >= n || b >= n {
                continue;
            }
            if next[a].is_some() {
                let t = self.plots[a].title.clone();
                self.error(
                    pos,
                    format!("plot \"{t}\" is already filled to another plot"),
                );
                continue;
            }
            if prev[b].is_some() {
                let t = self.plots[b].title.clone();
                self.error(
                    pos,
                    format!("plot \"{t}\" is already the target of another fill"),
                );
                continue;
            }
            if self.plots[a].style == PlotStyle::Histogram
                || self.plots[b].style == PlotStyle::Histogram
            {
                self.error(pos, "histogram plots can't be filled");
                continue;
            }
            next[a] = Some(b);
            prev[b] = Some(a);
            self.plots[a].style = PlotStyle::BandFill;
        }
        let mut order = Vec::with_capacity(n);
        let mut visited = vec![false; n];
        for (i, p) in prev.iter().enumerate() {
            if p.is_some() {
                continue;
            }
            let mut j = Some(i);
            while let Some(k) = j {
                if visited[k] {
                    break;
                }
                visited[k] = true;
                order.push(k);
                j = next[k];
            }
        }
        if let Some(k) = visited.iter().position(|v| !v) {
            let pos = self.plots[k].pos;
            self.error(pos, "fill(...) calls form a cycle");
        }
        order
    }

    fn input(&mut self, args: &[Arg], at: Pos) -> CResult<usize> {
        let usage = "input(\"Title\", default, min, max)";
        let a = self.bind(
            "input",
            args,
            &["title", "default", "min", "max"],
            2,
            at,
            usage,
        )?;
        let name = match a[0].map(|e| &e.kind) {
            Some(ExprKind::Str(s)) if !s.trim().is_empty() => s.clone(),
            _ => {
                let p = a[0].map_or(at, |e| e.pos);
                return self.err(
                    p,
                    "input title must be a non-empty string, e.g. input(\"Length\", 14)",
                );
            }
        };
        if self.inputs.iter().any(|d| d.name == name) {
            return self.err(at, format!("an input named \"{name}\" already exists"));
        }
        if self.inputs.len() >= MAX_INPUTS {
            return self.err(at, format!("too many inputs (limit {MAX_INPUTS})"));
        }
        let default = self.constant(a[1], at, "default")?;
        let min = match a[2] {
            Some(e) => Some(self.constant(Some(e), at, "min")?),
            None => None,
        };
        let max = match a[3] {
            Some(e) => Some(self.constant(Some(e), at, "max")?),
            None => None,
        };
        if let (Some(lo), Some(hi)) = (min, max)
            && lo > hi
        {
            return self.err(at, format!("input min ({lo}) is greater than max ({hi})"));
        }
        if min.is_some_and(|lo| default < lo) || max.is_some_and(|hi| default > hi) {
            return self.err(at, "input default is outside its min/max range");
        }
        self.inputs.push(InputDef {
            name,
            default,
            min,
            max,
        });
        Ok(self.inputs.len() - 1)
    }

    fn constant(&mut self, e: Option<&Expr>, at: Pos, what: &str) -> CResult<f64> {
        let p = e.map_or(at, |e| e.pos);
        match e.and_then(const_eval) {
            Some(v) if v.is_finite() => Ok(v),
            _ => self.err(p, format!("input {what} must be a number literal")),
        }
    }

    /// Compiles an expression that must be a single value (scalar or series).
    fn value(&mut self, e: &Expr) -> CResult<(Node, Ty)> {
        let (n, ty) = self.expr(e)?;
        if let Ty::Tuple(k) = ty {
            return self.err(e.pos, tuple_hint(k));
        }
        Ok((n, ty))
    }

    fn expr(&mut self, e: &Expr) -> CResult<(Node, Ty)> {
        let join = |a: Ty, b: Ty| {
            if a == Ty::Scalar && b == Ty::Scalar {
                Ty::Scalar
            } else {
                Ty::Series
            }
        };
        match &e.kind {
            ExprKind::Num(v) => Ok((Node::Const(*v), Ty::Scalar)),
            ExprKind::Bool(b) => Ok((Node::Const(f64::from(u8::from(*b))), Ty::Scalar)),
            ExprKind::Na => Ok((Node::Const(f64::NAN), Ty::Scalar)),
            ExprKind::Str(_) => self.err(
                e.pos,
                "strings are only allowed as titles (indicator, plot, input)",
            ),
            ExprKind::Color(_) => self.err(
                e.pos,
                "color literals are only allowed as `color = ...` arguments",
            ),
            ExprKind::Range(..) => self.err(
                e.pos,
                "ranges like `0..100` are only allowed in indicator(range = ...)",
            ),
            ExprKind::Ident(n) => self.ident(n, e.pos),
            ExprKind::Unary(op, x) => {
                let (n, t) = self.value(x)?;
                Ok((Node::Unary(*op, Box::new(n)), t))
            }
            ExprKind::Binary(op, a, b) => {
                let (na, ta) = self.value(a)?;
                let (nb, tb) = self.value(b)?;
                Ok((Node::Binary(*op, Box::new(na), Box::new(nb)), join(ta, tb)))
            }
            ExprKind::Ternary(c, a, b) => {
                let (nc, tc) = self.value(c)?;
                let (na, ta) = self.value(a)?;
                let (nb, tb) = self.value(b)?;
                let ty = join(tc, join(ta, tb));
                Ok((Node::Ternary(Box::new(nc), Box::new(na), Box::new(nb)), ty))
            }
            ExprKind::Index(base, off) => {
                let (nb, _) = self.value(base)?;
                let (no, to) = self.value(off)?;
                if to != Ty::Scalar {
                    return self.err(
                        off.pos,
                        "history offset must be a constant or input, not a series",
                    );
                }
                if let Node::Const(v) = no
                    && let Err(m) = check_offset(v)
                {
                    return self.err(off.pos, m);
                }
                Ok((
                    Node::History {
                        src: Box::new(nb),
                        offset: Box::new(no),
                        pos: off.pos,
                    },
                    Ty::Series,
                ))
            }
            ExprKind::Call(f, args) => self.call(f, args, e.pos),
        }
    }

    fn ident(&mut self, n: &str, pos: Pos) -> CResult<(Node, Ty)> {
        match self.env.get(n) {
            Some(Binding::Value { slot, ty }) => return Ok((Node::Slot(*slot), *ty)),
            Some(Binding::Plot(_)) => {
                return self.err(
                    pos,
                    format!("`{n}` is a plot handle; it can only be passed to fill(...)"),
                );
            }
            Some(Binding::Poisoned) => return Err(Reported),
            None => {}
        }
        if let Some(s) = BSeries::from_name(n) {
            return Ok((Node::Series(s), Ty::Series));
        }
        if builtins::lookup(n).is_some() || STATEMENT_FNS.contains(&n) || n == "input" {
            return self.err(
                pos,
                format!("`{n}` is a function; call it with arguments: {n}(...)"),
            );
        }
        if builtins::named_color(n).is_some() {
            return self.err(
                pos,
                format!("color names can only be used as `color = {n}`"),
            );
        }
        if n == "line" || n == "histogram" {
            return self.err(pos, format!("`{n}` can only be used as `style = {n}`"));
        }
        let hint = suggest(
            n,
            self.env
                .keys()
                .map(|k| k.as_str())
                .chain(SERIES_NAMES.iter().map(|(k, _)| *k)),
        );
        self.err(pos, format!("unknown identifier `{n}`{hint}"))
    }

    fn call(&mut self, f: &str, args: &[Arg], at: Pos) -> CResult<(Node, Ty)> {
        if f == "input" {
            return Ok((Node::Input(self.input(args, at)?), Ty::Scalar));
        }
        if STATEMENT_FNS.contains(&f) {
            return self.err(
                at,
                format!("`{f}(...)` is a statement and can't be used inside an expression"),
            );
        }
        let Some(sig) = builtins::lookup(f) else {
            let hint = suggest(
                f,
                SIGS.iter()
                    .map(|s| s.name)
                    .chain(STATEMENT_FNS.iter().copied())
                    .chain(std::iter::once("input")),
            );
            let extra = if self.env.contains_key(f) || BSeries::from_name(f).is_some() {
                format!("; `{f}` is a value, not a function")
            } else {
                hint
            };
            return self.err(at, format!("unknown function `{f}`{extra}"));
        };
        let mut nodes = Vec::new();
        let mut arg_pos = Vec::new();
        let mut any_series = false;
        if sig.variadic {
            if let Some(a) = args.iter().find(|a| a.name.is_some()) {
                let p = a.name.as_ref().map_or(at, |(_, p)| *p);
                return self.err(p, format!("`{f}` takes only positional arguments"));
            }
            if args.len() < 2 || args.len() > MAX_VARIADIC {
                return self.err(
                    at,
                    format!(
                        "`{f}` takes 2 to {MAX_VARIADIC} arguments, got {}",
                        args.len()
                    ),
                );
            }
            for a in args {
                let (n, t) = self.value(&a.value)?;
                any_series |= t == Ty::Series;
                nodes.push(n);
                arg_pos.push(a.value.pos);
            }
        } else {
            let params: Vec<&str> = sig.params.iter().map(|p| p.name).collect();
            let required = sig.params.iter().filter(|p| p.default.is_none()).count();
            let bound = self.bind(f, args, &params, required, at, &sig.usage())?;
            for (p, b) in sig.params.iter().zip(bound) {
                match b {
                    None => {
                        nodes.push(Node::Const(p.default.unwrap_or(f64::NAN)));
                        arg_pos.push(at);
                    }
                    Some(e) => {
                        let (n, t) = self.value(e)?;
                        if matches!(p.kind, Kind::Period | Kind::Scalar) && t == Ty::Series {
                            return self.err(
                                e.pos,
                                format!(
                                    "`{}` of {f}(...) must be a constant or input, not a series",
                                    p.name
                                ),
                            );
                        }
                        any_series |= p.kind == Kind::Num && t == Ty::Series;
                        nodes.push(n);
                        arg_pos.push(e.pos);
                    }
                }
            }
        }
        let ty = match sig.ret {
            Ret::Series => Ty::Series,
            Ret::Elementwise if any_series => Ty::Series,
            Ret::Elementwise => Ty::Scalar,
            Ret::Tuple(k) => Ty::Tuple(k),
        };
        Ok((
            Node::Call {
                func: sig.func,
                args: nodes,
                arg_pos,
                at,
            },
            ty,
        ))
    }
}
