//! Syntax tree produced by the parser.

use crate::lexer::Pos;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    /// Start of the expression.
    pub pos: Pos,
    /// Height of the tree rooted here (leaf = 1). Bounded by the parser so
    /// every later recursive pass has bounded stack use.
    pub depth: u32,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Num(f64),
    Str(String),
    Color([u8; 4]),
    Bool(bool),
    Na,
    Ident(String),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `base[offset]`
    Index(Box<Expr>, Box<Expr>),
    Call(String, Vec<Arg>),
    /// `lo..hi`, only meaningful as `indicator(range = lo..hi)`.
    Range(Box<Expr>, Box<Expr>),
}

impl ExprKind {
    pub fn max_child_depth(&self) -> u32 {
        match self {
            ExprKind::Num(_)
            | ExprKind::Str(_)
            | ExprKind::Color(_)
            | ExprKind::Bool(_)
            | ExprKind::Na
            | ExprKind::Ident(_) => 0,
            ExprKind::Unary(_, e) => e.depth,
            ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Range(a, b) => {
                a.depth.max(b.depth)
            }
            ExprKind::Ternary(a, b, c) => a.depth.max(b.depth).max(c.depth),
            ExprKind::Call(_, args) => args.iter().map(|a| a.value.depth).max().unwrap_or(0),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Arg {
    pub name: Option<(String, Pos)>,
    pub value: Expr,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    /// `let x = e` (tuple = false, one name) or `let (a, b) = e`.
    Let {
        names: Vec<(String, Pos)>,
        tuple: bool,
        value: Expr,
    },
    Expr(Expr),
}
