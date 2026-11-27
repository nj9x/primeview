//! Recursive-descent parser with bounded recursion.
//!
//! Precedence, lowest to highest:
//! `?:` (right-assoc) < `or` < `and` < `not` < comparisons (non-chaining)
//! < `+ -` < `* / %` < unary `- +` < postfix `[n]` < primary.

use crate::Diagnostic;
use crate::ast::{Arg, BinOp, Expr, ExprKind, Stmt, UnOp};
use crate::lexer::{Pos, Tok, Token};
use crate::limits::{MAX_ARGS, MAX_ERRORS, MAX_EXPR_DEPTH, MAX_NEST, MAX_STATEMENTS};

type PResult<T> = Result<T, Diagnostic>;

pub fn parse(toks: Vec<Token>) -> Result<Vec<Stmt>, Vec<Diagnostic>> {
    let mut p = Parser {
        toks,
        i: 0,
        nest: 0,
    };
    if p.toks.is_empty() {
        p.toks.push(Token {
            tok: Tok::Eof,
            pos: Pos { line: 1, col: 1 },
        });
    }
    let mut stmts = Vec::new();
    let mut errors = Vec::new();
    loop {
        p.skip_newlines();
        if *p.peek() == Tok::Eof {
            break;
        }
        if stmts.len() >= MAX_STATEMENTS {
            errors.push(Diagnostic::at(
                p.pos(),
                format!("too many statements (limit {MAX_STATEMENTS})"),
            ));
            break;
        }
        p.nest = 0;
        match p.statement() {
            Ok(s) => match p.peek() {
                Tok::Newline | Tok::Eof => stmts.push(s),
                t => {
                    let msg = format!(
                        "expected end of line after statement, found {}",
                        t.describe()
                    );
                    errors.push(Diagnostic::at(p.pos(), msg));
                    p.recover();
                }
            },
            Err(d) => {
                errors.push(d);
                p.recover();
            }
        }
        if errors.len() >= MAX_ERRORS {
            break;
        }
    }
    if errors.is_empty() {
        Ok(stmts)
    } else {
        Err(errors)
    }
}

fn cmp_op(t: &Tok) -> Option<BinOp> {
    Some(match t {
        Tok::Lt => BinOp::Lt,
        Tok::Le => BinOp::Le,
        Tok::Gt => BinOp::Gt,
        Tok::Ge => BinOp::Ge,
        Tok::EqEq => BinOp::Eq,
        Tok::Ne => BinOp::Ne,
        _ => return None,
    })
}

struct Parser {
    toks: Vec<Token>,
    i: usize,
    nest: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        self.peek_at(0)
    }

    fn peek_at(&self, k: usize) -> &Tok {
        let last = self.toks.len() - 1;
        &self.toks[(self.i + k).min(last)].tok
    }

    fn pos(&self) -> Pos {
        let last = self.toks.len() - 1;
        self.toks[self.i.min(last)].pos
    }

    fn advance(&mut self) {
        if self.i < self.toks.len() - 1 {
            self.i += 1;
        }
    }

    fn skip_newlines(&mut self) {
        while *self.peek() == Tok::Newline {
            self.advance();
        }
    }

    /// Skip to the start of the next statement.
    fn recover(&mut self) {
        while !matches!(self.peek(), Tok::Newline | Tok::Eof) {
            self.advance();
        }
    }

    fn expect(&mut self, t: Tok, msg: impl FnOnce(&Tok) -> String) -> PResult<Pos> {
        if *self.peek() == t {
            let p = self.pos();
            self.advance();
            Ok(p)
        } else {
            Err(Diagnostic::at(self.pos(), msg(self.peek())))
        }
    }

    fn enter(&mut self, pos: Pos) -> PResult<()> {
        if self.nest >= MAX_NEST {
            return Err(Diagnostic::at(
                pos,
                format!("expression is nested too deeply (limit {MAX_NEST})"),
            ));
        }
        self.nest += 1;
        Ok(())
    }

    fn mk(&self, kind: ExprKind, pos: Pos) -> PResult<Expr> {
        let depth = 1 + kind.max_child_depth();
        if depth as usize > MAX_EXPR_DEPTH {
            return Err(Diagnostic::at(
                pos,
                format!(
                    "expression is too complex (depth limit {MAX_EXPR_DEPTH}); split it with `let`"
                ),
            ));
        }
        Ok(Expr { kind, pos, depth })
    }

    fn statement(&mut self) -> PResult<Stmt> {
        match self.peek().clone() {
            Tok::Let => self.let_stmt(),
            Tok::Ident(name) if *self.peek_at(1) == Tok::Assign => Err(Diagnostic::at(
                self.pos(),
                format!("to define a variable write `let {name} = ...`"),
            )),
            _ => Ok(Stmt::Expr(self.expr()?)),
        }
    }

    fn ident(&mut self, what: &str) -> PResult<(String, Pos)> {
        match self.peek().clone() {
            Tok::Ident(n) => {
                let p = self.pos();
                self.advance();
                Ok((n, p))
            }
            t => Err(Diagnostic::at(
                self.pos(),
                format!("expected {what}, found {}", t.describe()),
            )),
        }
    }

    fn let_stmt(&mut self) -> PResult<Stmt> {
        self.advance(); // `let`
        let (names, tuple) = if *self.peek() == Tok::LParen {
            let open = self.pos();
            self.advance();
            let mut names = Vec::new();
            loop {
                names.push(self.ident("a variable name")?);
                if names.len() > MAX_ARGS {
                    return Err(Diagnostic::at(open, "too many names in pattern"));
                }
                match self.peek() {
                    Tok::Comma => self.advance(),
                    Tok::RParen => {
                        self.advance();
                        break;
                    }
                    t => {
                        return Err(Diagnostic::at(
                            self.pos(),
                            format!("expected `,` or `)` in `let (...)`, found {}", t.describe()),
                        ));
                    }
                }
            }
            (names, true)
        } else {
            (vec![self.ident("a variable name after `let`")?], false)
        };
        self.expect(Tok::Assign, |t| {
            format!(
                "expected `=` after the name in `let`, found {}",
                t.describe()
            )
        })?;
        self.skip_newlines();
        let value = self.expr()?;
        Ok(Stmt::Let {
            names,
            tuple,
            value,
        })
    }

    fn expr(&mut self) -> PResult<Expr> {
        self.enter(self.pos())?;
        let r = self.ternary();
        self.nest -= 1;
        r
    }

    fn ternary(&mut self) -> PResult<Expr> {
        let cond = self.or_expr()?;
        if *self.peek() != Tok::Question {
            return Ok(cond);
        }
        self.advance();
        self.skip_newlines();
        let a = self.expr()?;
        self.skip_newlines();
        self.expect(Tok::Colon, |t| {
            format!("expected `:` in `cond ? a : b`, found {}", t.describe())
        })?;
        self.skip_newlines();
        let b = self.expr()?;
        let pos = cond.pos;
        self.mk(
            ExprKind::Ternary(Box::new(cond), Box::new(a), Box::new(b)),
            pos,
        )
    }

    fn binary_loop(
        &mut self,
        op_of: fn(&Tok) -> Option<BinOp>,
        next: fn(&mut Self) -> PResult<Expr>,
    ) -> PResult<Expr> {
        let mut lhs = next(self)?;
        while let Some(op) = op_of(self.peek()) {
            self.advance();
            self.skip_newlines();
            let rhs = next(self)?;
            let pos = lhs.pos;
            lhs = self.mk(ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), pos)?;
        }
        Ok(lhs)
    }

    fn or_expr(&mut self) -> PResult<Expr> {
        self.binary_loop(|t| (*t == Tok::Or).then_some(BinOp::Or), Self::and_expr)
    }

    fn and_expr(&mut self) -> PResult<Expr> {
        self.binary_loop(|t| (*t == Tok::And).then_some(BinOp::And), Self::not_expr)
    }

    fn not_expr(&mut self) -> PResult<Expr> {
        if *self.peek() != Tok::Not {
            return self.comparison();
        }
        let pos = self.pos();
        self.advance();
        self.enter(pos)?;
        let r = self.not_expr();
        self.nest -= 1;
        self.mk(ExprKind::Unary(UnOp::Not, Box::new(r?)), pos)
    }

    fn comparison(&mut self) -> PResult<Expr> {
        let lhs = self.additive()?;
        let Some(op) = cmp_op(self.peek()) else {
            return Ok(lhs);
        };
        self.advance();
        self.skip_newlines();
        let rhs = self.additive()?;
        if cmp_op(self.peek()).is_some() {
            return Err(Diagnostic::at(
                self.pos(),
                "comparisons can't be chained (a < b < c); combine them with `and`",
            ));
        }
        let pos = lhs.pos;
        self.mk(ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), pos)
    }

    fn additive(&mut self) -> PResult<Expr> {
        self.binary_loop(
            |t| match t {
                Tok::Plus => Some(BinOp::Add),
                Tok::Minus => Some(BinOp::Sub),
                _ => None,
            },
            Self::multiplicative,
        )
    }

    fn multiplicative(&mut self) -> PResult<Expr> {
        self.binary_loop(
            |t| match t {
                Tok::Star => Some(BinOp::Mul),
                Tok::Slash => Some(BinOp::Div),
                Tok::Percent => Some(BinOp::Mod),
                _ => None,
            },
            Self::unary,
        )
    }

    fn unary(&mut self) -> PResult<Expr> {
        let neg = match self.peek() {
            Tok::Minus => true,
            Tok::Plus => false,
            _ => return self.postfix(),
        };
        let pos = self.pos();
        self.advance();
        self.enter(pos)?;
        let r = self.unary();
        self.nest -= 1;
        let e = r?;
        if !neg {
            return Ok(e);
        }
        if let ExprKind::Num(v) = e.kind {
            return self.mk(ExprKind::Num(-v), pos);
        }
        self.mk(ExprKind::Unary(UnOp::Neg, Box::new(e)), pos)
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut e = self.primary()?;
        while *self.peek() == Tok::LBracket {
            let open = self.pos();
            self.advance();
            let off = self.expr()?;
            self.expect(Tok::RBracket, |t| {
                format!(
                    "expected `]` to close `[` at {}:{}, found {}",
                    open.line,
                    open.col,
                    t.describe()
                )
            })?;
            let pos = e.pos;
            e = self.mk(ExprKind::Index(Box::new(e), Box::new(off)), pos)?;
        }
        Ok(e)
    }

    fn primary(&mut self) -> PResult<Expr> {
        let pos = self.pos();
        let tok = self.peek().clone();
        let kind = match tok {
            Tok::Num(v) => ExprKind::Num(v),
            Tok::Str(s) => ExprKind::Str(s),
            Tok::Color(c) => ExprKind::Color(c),
            Tok::True => ExprKind::Bool(true),
            Tok::False => ExprKind::Bool(false),
            Tok::Na => {
                self.advance();
                if *self.peek() == Tok::LParen {
                    self.advance();
                    let args = self.call_args(pos)?;
                    return self.mk(ExprKind::Call("na".into(), args), pos);
                }
                return self.mk(ExprKind::Na, pos);
            }
            Tok::Ident(name) => {
                self.advance();
                if *self.peek() == Tok::LParen {
                    self.advance();
                    let args = self.call_args(pos)?;
                    return self.mk(ExprKind::Call(name, args), pos);
                }
                return self.mk(ExprKind::Ident(name), pos);
            }
            Tok::LParen => {
                self.advance();
                let e = self.expr()?;
                self.expect(Tok::RParen, |t| {
                    format!(
                        "expected `)` to close `(` at {}:{}, found {}",
                        pos.line,
                        pos.col,
                        t.describe()
                    )
                })?;
                return Ok(e);
            }
            Tok::Let => {
                return Err(Diagnostic::at(pos, "`let` can only start a statement"));
            }
            t => {
                return Err(Diagnostic::at(
                    pos,
                    format!("expected an expression, found {}", t.describe()),
                ));
            }
        };
        self.advance();
        self.mk(kind, pos)
    }

    /// Parses arguments after the opening `(` up to and including `)`.
    fn call_args(&mut self, open: Pos) -> PResult<Vec<Arg>> {
        let mut args = Vec::new();
        loop {
            if *self.peek() == Tok::RParen {
                self.advance();
                break;
            }
            if args.len() >= MAX_ARGS {
                return Err(Diagnostic::at(
                    self.pos(),
                    format!("too many arguments (limit {MAX_ARGS})"),
                ));
            }
            let name = match self.peek().clone() {
                Tok::Ident(n) if *self.peek_at(1) == Tok::Assign => {
                    let p = self.pos();
                    self.advance();
                    self.advance();
                    Some((n, p))
                }
                _ => None,
            };
            let value = self.arg_value()?;
            args.push(Arg { name, value });
            match self.peek() {
                Tok::Comma => self.advance(),
                Tok::RParen => {
                    self.advance();
                    break;
                }
                t => {
                    return Err(Diagnostic::at(
                        self.pos(),
                        format!(
                            "expected `,` or `)` in the argument list opened at {}:{}, found {}",
                            open.line,
                            open.col,
                            t.describe()
                        ),
                    ));
                }
            }
        }
        Ok(args)
    }

    fn arg_value(&mut self) -> PResult<Expr> {
        let lo = self.expr()?;
        if *self.peek() != Tok::DotDot {
            return Ok(lo);
        }
        self.advance();
        let hi = self.expr()?;
        let pos = lo.pos;
        self.mk(ExprKind::Range(Box::new(lo), Box::new(hi)), pos)
    }
}
