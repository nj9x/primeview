//! Coordinate mapping, tick generation and number formatting.

use primeview_core::time::MS_PER_DAY;

/// Maps fractional bar indices to screen x. Bar `i`'s centre is at `x(i)`.
#[derive(Clone, Copy, Debug)]
pub struct XMap {
    pub right: f32,
    pub bar_w: f32,
    pub last: f64,
    /// Empty bars between the last bar and the right edge (negative = scrolled back).
    pub offset: f64,
}

impl XMap {
    pub fn x(&self, idx: f64) -> f32 {
        self.right - ((self.last - idx + self.offset + 0.5) * self.bar_w as f64) as f32
    }

    pub fn idx(&self, x: f32) -> f64 {
        self.last + self.offset + 0.5 - ((self.right - x) / self.bar_w) as f64
    }
}

/// Maps values to screen y within a pane.
#[derive(Clone, Copy, Debug)]
pub struct YMap {
    pub top: f32,
    pub bottom: f32,
    pub min: f64,
    pub max: f64,
}

impl YMap {
    pub fn y(&self, v: f64) -> f32 {
        let t = (v - self.min) / (self.max - self.min).max(f64::EPSILON);
        self.bottom - (t as f32) * (self.bottom - self.top)
    }

    pub fn v(&self, y: f32) -> f64 {
        let t = ((self.bottom - y) / (self.bottom - self.top).max(1.0)) as f64;
        self.min + t * (self.max - self.min)
    }

    pub fn span(&self) -> f64 {
        self.max - self.min
    }
}

/// A "nice" step (1, 2, 2.5, 5 × 10ⁿ) giving roughly `target` ticks over `range`.
pub fn nice_step(range: f64, target: f64) -> f64 {
    if !(range.is_finite() && range > 0.0) {
        return 1.0;
    }
    let raw = range / target.max(1.0);
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let nice = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 2.5 {
        2.5
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

/// Tick values that are multiples of `step` within `[min, max]`.
pub fn ticks(min: f64, max: f64, step: f64) -> impl Iterator<Item = f64> {
    let first = (min / step).ceil() as i64;
    let last = (max / step).floor() as i64;
    (first..=last.max(first - 1))
        .take(500)
        .map(move |k| k as f64 * step)
}

/// Decimals needed to show a price of this magnitude with sensible precision.
pub fn price_decimals(p: f64) -> usize {
    let p = p.abs();
    if !(p.is_finite() && p > 0.0) {
        return 2;
    }
    ((-p.log10()).ceil() as i32 + 4).clamp(2, 10) as usize
}

/// Decimals needed to distinguish ticks `step` apart.
pub fn step_decimals(step: f64) -> usize {
    if step >= 1.0 {
        if step.fract() == 0.0 { 0 } else { 1 }
    } else {
        let d = (-step.log10()).ceil() as usize;
        // 2.5e-n needs one more digit
        let scaled = step * 10f64.powi(d as i32);
        d + usize::from((scaled - scaled.round()).abs() > 1e-9)
    }
}

pub fn fmt_fixed(v: f64, decimals: usize) -> String {
    format!("{v:.decimals$}")
}

/// Compact formatting for volumes and indicator values: 1.23K, 4.5M, 0.0123.
pub fn fmt_compact(v: f64) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.2}B", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.2}M", v / 1e6)
    } else if a >= 1e4 {
        format!("{:.2}K", v / 1e3)
    } else if a >= 1.0 || a == 0.0 {
        format!("{v:.2}")
    } else {
        format!("{:.*}", price_decimals(v), v)
    }
}

/// A calendar unit for time-axis ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickUnit {
    Millis(i64),
    Days(i64),
    Months(i64),
}

impl TickUnit {
    pub fn approx_ms(self) -> i64 {
        match self {
            TickUnit::Millis(ms) => ms,
            TickUnit::Days(d) => d * MS_PER_DAY,
            TickUnit::Months(m) => m * 30 * MS_PER_DAY,
        }
    }

    /// A bucket key: a tick goes where the key changes between bars.
    pub fn key(self, t: i64) -> i64 {
        match self {
            TickUnit::Millis(ms) => t.div_euclid(ms),
            TickUnit::Days(d) => {
                // Bucket by day-of-month so ticks land on 1, 1+d, … each month.
                let dt = primeview_core::time::DateTime::from_millis(t);
                (dt.year as i64 * 12 + dt.month as i64) * 100
                    + ((dt.day as i64 - 1) / d).min(31 / d - 1)
            }
            TickUnit::Months(m) => {
                let dt = primeview_core::time::DateTime::from_millis(t);
                (dt.year as i64 * 12 + dt.month as i64 - 1).div_euclid(m)
            }
        }
    }
}

/// Pick the finest tick unit whose labels are at least `min_px` apart.
pub fn time_tick_unit(bar_ms: i64, bar_w: f32, min_px: f32) -> TickUnit {
    const MIN: i64 = 60_000;
    const H: i64 = 60 * MIN;
    let candidates = [
        TickUnit::Millis(MIN),
        TickUnit::Millis(5 * MIN),
        TickUnit::Millis(15 * MIN),
        TickUnit::Millis(30 * MIN),
        TickUnit::Millis(H),
        TickUnit::Millis(2 * H),
        TickUnit::Millis(3 * H),
        TickUnit::Millis(6 * H),
        TickUnit::Millis(12 * H),
        TickUnit::Days(1),
        TickUnit::Days(2),
        TickUnit::Days(5),
        TickUnit::Days(10),
        TickUnit::Months(1),
        TickUnit::Months(3),
        TickUnit::Months(6),
        TickUnit::Months(12),
        TickUnit::Months(24),
        TickUnit::Months(60),
    ];
    let px_per_ms = bar_w as f64 / bar_ms.max(1) as f64;
    candidates
        .into_iter()
        .find(|u| u.approx_ms() >= bar_ms && u.approx_ms() as f64 * px_per_ms >= min_px as f64)
        .unwrap_or(TickUnit::Months(120))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x_roundtrip() {
        let m = XMap {
            right: 800.0,
            bar_w: 7.5,
            last: 999.0,
            offset: 4.0,
        };
        for i in [0.0, 500.0, 999.0, 1003.5] {
            assert!((m.idx(m.x(i)) - i).abs() < 1e-3);
        }
        // last bar centre sits offset+0.5 bars from the right edge
        assert!((m.x(999.0) - (800.0 - 4.5 * 7.5)).abs() < 1e-4);
    }

    #[test]
    fn y_roundtrip() {
        let m = YMap {
            top: 10.0,
            bottom: 410.0,
            min: 100.0,
            max: 200.0,
        };
        assert_eq!(m.y(100.0), 410.0);
        assert_eq!(m.y(200.0), 10.0);
        assert!((m.v(m.y(137.0)) - 137.0).abs() < 1e-3);
    }

    #[test]
    fn nice_steps() {
        assert_eq!(nice_step(100.0, 10.0), 10.0);
        assert_eq!(nice_step(90.0, 10.0), 10.0);
        assert_eq!(nice_step(0.03, 10.0), 0.005);
        assert_eq!(nice_step(23.0, 10.0), 2.5);
        assert_eq!(
            ticks(0.5, 3.1, 1.0).collect::<Vec<_>>(),
            vec![1.0, 2.0, 3.0]
        );
    }

    #[test]
    fn decimals() {
        assert_eq!(price_decimals(65_000.0), 2);
        assert_eq!(price_decimals(2.5), 4);
        assert_eq!(price_decimals(0.15), 5);
        assert_eq!(step_decimals(50.0), 0);
        assert_eq!(step_decimals(0.5), 1);
        assert_eq!(step_decimals(0.25), 2);
        assert_eq!(step_decimals(0.0025), 4);
    }

    #[test]
    fn tick_unit_selection() {
        // 1m bars, 6px wide → 15 minutes is 90px
        assert_eq!(
            time_tick_unit(60_000, 6.0, 80.0),
            TickUnit::Millis(15 * 60_000)
        );
        // daily bars, 8px → 10 days = 80px
        assert_eq!(time_tick_unit(MS_PER_DAY, 8.0, 80.0), TickUnit::Days(10));
    }
}
