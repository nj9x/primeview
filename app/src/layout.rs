//! Multi-chart layouts.

use eframe::egui::{Rect, pos2};
use serde::{Deserialize, Serialize};

pub const MAX_PANES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layout {
    Single,
    Columns2,
    Rows2,
    Columns3,
    /// One large chart on the left, two stacked on the right.
    MainLeft3,
    Grid4,
}

impl Layout {
    pub const ALL: [Layout; 6] = [
        Layout::Single,
        Layout::Columns2,
        Layout::Rows2,
        Layout::Columns3,
        Layout::MainLeft3,
        Layout::Grid4,
    ];

    pub fn count(self) -> usize {
        match self {
            Layout::Single => 1,
            Layout::Columns2 | Layout::Rows2 => 2,
            Layout::Columns3 | Layout::MainLeft3 => 3,
            Layout::Grid4 => 4,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Layout::Single => "▣",
            Layout::Columns2 => "◫",
            Layout::Rows2 => "⊟",
            Layout::Columns3 => "⫼",
            Layout::MainLeft3 => "◧",
            Layout::Grid4 => "⊞",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Layout::Single => "1 chart",
            Layout::Columns2 => "2 side by side",
            Layout::Rows2 => "2 stacked",
            Layout::Columns3 => "3 columns",
            Layout::MainLeft3 => "1 large + 2",
            Layout::Grid4 => "2 × 2 grid",
        }
    }

    /// Pane rectangles inside `r`, separated by `gap` pixels.
    pub fn rects(self, r: Rect, gap: f32) -> Vec<Rect> {
        let cols = |n: usize, r: Rect| -> Vec<Rect> {
            let w = (r.width() - gap * (n - 1) as f32) / n as f32;
            (0..n)
                .map(|i| {
                    let x = r.left() + i as f32 * (w + gap);
                    Rect::from_min_max(pos2(x, r.top()), pos2(x + w, r.bottom()))
                })
                .collect()
        };
        let rows = |n: usize, r: Rect| -> Vec<Rect> {
            let h = (r.height() - gap * (n - 1) as f32) / n as f32;
            (0..n)
                .map(|i| {
                    let y = r.top() + i as f32 * (h + gap);
                    Rect::from_min_max(pos2(r.left(), y), pos2(r.right(), y + h))
                })
                .collect()
        };
        match self {
            Layout::Single => vec![r],
            Layout::Columns2 => cols(2, r),
            Layout::Rows2 => rows(2, r),
            Layout::Columns3 => cols(3, r),
            Layout::MainLeft3 => {
                let split = r.left() + r.width() * 0.62;
                let left = Rect::from_min_max(r.min, pos2(split - gap / 2.0, r.bottom()));
                let right = Rect::from_min_max(pos2(split + gap / 2.0, r.top()), r.max);
                let mut v = vec![left];
                v.extend(rows(2, right));
                v
            }
            Layout::Grid4 => rows(2, r)
                .into_iter()
                .flat_map(|row| cols(2, row))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_counts_match_and_fit() {
        let r = Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 600.0));
        for l in Layout::ALL {
            let rs = l.rects(r, 2.0);
            assert_eq!(rs.len(), l.count(), "{l:?}");
            assert!(l.count() <= MAX_PANES);
            for p in &rs {
                assert!(r.expand(0.01).contains_rect(*p), "{l:?} {p:?}");
                assert!(p.width() > 100.0 && p.height() > 100.0);
            }
        }
    }
}
