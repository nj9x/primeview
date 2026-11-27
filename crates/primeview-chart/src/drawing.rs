//! User drawings (trend lines, levels, rectangles, Fibonacci retracements).
//! Anchors are stored in data space (time ms, price) so they survive zoom,
//! scroll, history loads and app restarts.

use egui::{Pos2, pos2};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tool {
    Cursor,
    TrendLine,
    HorizontalLine,
    VerticalLine,
    Rectangle,
    Fibonacci,
}

impl Tool {
    pub const ALL: [Tool; 6] = [
        Tool::Cursor,
        Tool::TrendLine,
        Tool::HorizontalLine,
        Tool::VerticalLine,
        Tool::Rectangle,
        Tool::Fibonacci,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Cursor => "Cursor",
            Tool::TrendLine => "Trend line",
            Tool::HorizontalLine => "Horizontal line",
            Tool::VerticalLine => "Vertical line",
            Tool::Rectangle => "Rectangle",
            Tool::Fibonacci => "Fib retracement",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Tool::Cursor => "➚",
            Tool::TrendLine => "╱",
            Tool::HorizontalLine => "─",
            Tool::VerticalLine => "│",
            Tool::Rectangle => "▭",
            Tool::Fibonacci => "≡",
        }
    }

    pub fn kind(self) -> Option<DrawingKind> {
        Some(match self {
            Tool::Cursor => return None,
            Tool::TrendLine => DrawingKind::TrendLine,
            Tool::HorizontalLine => DrawingKind::HorizontalLine,
            Tool::VerticalLine => DrawingKind::VerticalLine,
            Tool::Rectangle => DrawingKind::Rectangle,
            Tool::Fibonacci => DrawingKind::Fibonacci,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DrawingKind {
    TrendLine,
    HorizontalLine,
    VerticalLine,
    Rectangle,
    Fibonacci,
}

impl DrawingKind {
    /// Single-click drawings only need one anchor.
    pub fn one_point(self) -> bool {
        matches!(
            self,
            DrawingKind::HorizontalLine | DrawingKind::VerticalLine
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub time: f64,
    pub price: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Drawing {
    pub kind: DrawingKind,
    pub a: Anchor,
    pub b: Anchor,
}

pub const FIB_LEVELS: [f64; 7] = [0.0, 0.236, 0.382, 0.5, 0.618, 0.786, 1.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    A,
    B,
    Body,
}

pub fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 < 1e-6 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Screen-space hit test. `pa`/`pb` are the anchors already mapped to screen;
/// `left`/`right` are the plot's x bounds (for infinite horizontal lines).
pub fn hit_test(
    kind: DrawingKind,
    pa: Pos2,
    pb: Pos2,
    p: Pos2,
    left: f32,
    right: f32,
) -> Option<Handle> {
    const HANDLE_R: f32 = 7.0;
    const LINE_TOL: f32 = 5.0;
    if !kind.one_point() {
        if p.distance(pa) <= HANDLE_R {
            return Some(Handle::A);
        }
        if p.distance(pb) <= HANDLE_R {
            return Some(Handle::B);
        }
    }
    let near = |a: Pos2, b: Pos2| dist_to_segment(p, a, b) <= LINE_TOL;
    let body = match kind {
        DrawingKind::TrendLine => near(pa, pb),
        DrawingKind::HorizontalLine => {
            (p.y - pa.y).abs() <= LINE_TOL && p.x >= left && p.x <= right
        }
        DrawingKind::VerticalLine => (p.x - pa.x).abs() <= LINE_TOL,
        DrawingKind::Rectangle => {
            let (c, d) = (pos2(pa.x, pb.y), pos2(pb.x, pa.y));
            near(pa, c) || near(c, pb) || near(pb, d) || near(d, pa)
        }
        DrawingKind::Fibonacci => {
            let (x0, x1) = (pa.x.min(pb.x), pa.x.max(pb.x));
            FIB_LEVELS.iter().any(|l| {
                let y = pb.y + (pa.y - pb.y) * *l as f32;
                near(pos2(x0, y), pos2(x1, y))
            })
        }
    };
    body.then_some(Handle::Body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_distance() {
        let d = dist_to_segment(pos2(5.0, 5.0), pos2(0.0, 0.0), pos2(10.0, 0.0));
        assert!((d - 5.0).abs() < 1e-5);
        let d = dist_to_segment(pos2(-3.0, 4.0), pos2(0.0, 0.0), pos2(10.0, 0.0));
        assert!((d - 5.0).abs() < 1e-5);
    }

    #[test]
    fn hits() {
        let (a, b) = (pos2(0.0, 0.0), pos2(100.0, 100.0));
        assert_eq!(
            hit_test(DrawingKind::TrendLine, a, b, pos2(2.0, 1.0), 0.0, 500.0),
            Some(Handle::A)
        );
        assert_eq!(
            hit_test(DrawingKind::TrendLine, a, b, pos2(50.0, 52.0), 0.0, 500.0),
            Some(Handle::Body)
        );
        assert_eq!(
            hit_test(DrawingKind::TrendLine, a, b, pos2(50.0, 80.0), 0.0, 500.0),
            None
        );
        assert_eq!(
            hit_test(
                DrawingKind::HorizontalLine,
                a,
                b,
                pos2(300.0, 3.0),
                0.0,
                500.0
            ),
            Some(Handle::Body)
        );
        assert_eq!(
            hit_test(DrawingKind::Fibonacci, a, b, pos2(50.0, 50.0), 0.0, 500.0),
            Some(Handle::Body)
        );
    }
}
