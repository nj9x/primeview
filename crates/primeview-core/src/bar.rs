use serde::{Deserialize, Serialize};

/// One OHLCV candle. `time` is the bar's open time in Unix milliseconds (UTC).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    pub time: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

impl Bar {
    pub fn is_up(&self) -> bool {
        self.close >= self.open
    }

    pub fn typical_price(&self) -> f64 {
        (self.high + self.low + self.close) / 3.0
    }
}

/// A time-ordered list of bars with helpers for streaming updates.
#[derive(Clone, Debug, Default)]
pub struct Series {
    pub bars: Vec<Bar>,
}

impl Series {
    pub fn new(mut bars: Vec<Bar>) -> Self {
        bars.sort_by_key(|b| b.time);
        bars.dedup_by_key(|b| b.time);
        Self { bars }
    }

    pub fn len(&self) -> usize {
        self.bars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bars.is_empty()
    }

    pub fn last(&self) -> Option<&Bar> {
        self.bars.last()
    }

    /// Insert or replace a bar by its open time. Streaming updates of the
    /// forming candle hit the fast path (replace last / append).
    pub fn upsert(&mut self, bar: Bar) {
        match self.bars.last_mut() {
            None => self.bars.push(bar),
            Some(last) if last.time == bar.time => *last = bar,
            Some(last) if last.time < bar.time => self.bars.push(bar),
            Some(_) => match self.bars.binary_search_by_key(&bar.time, |b| b.time) {
                Ok(i) => self.bars[i] = bar,
                Err(i) => self.bars.insert(i, bar),
            },
        }
    }

    /// Prepend older history (e.g. when the user scrolls back in time).
    /// Bars that overlap what we already have are dropped.
    pub fn prepend(&mut self, older: Vec<Bar>) -> usize {
        let first_time = self.bars.first().map_or(i64::MAX, |b| b.time);
        let mut older: Vec<Bar> = older.into_iter().filter(|b| b.time < first_time).collect();
        older.sort_by_key(|b| b.time);
        older.dedup_by_key(|b| b.time);
        let added = older.len();
        older.append(&mut self.bars);
        self.bars = older;
        added
    }

    /// Fractional bar index for a timestamp. Extrapolates past either end
    /// using `step_ms`, so drawings anchored in the future stay put.
    pub fn index_of_time(&self, t: f64, step_ms: i64) -> f64 {
        let step = step_ms.max(1) as f64;
        let (Some(first), Some(last)) = (self.bars.first(), self.bars.last()) else {
            return 0.0;
        };
        if t <= first.time as f64 {
            return (t - first.time as f64) / step;
        }
        if t >= last.time as f64 {
            return (self.bars.len() - 1) as f64 + (t - last.time as f64) / step;
        }
        let i = self.bars.partition_point(|b| (b.time as f64) <= t) - 1;
        let a = self.bars[i].time as f64;
        let b = self.bars[i + 1].time as f64;
        i as f64 + (t - a) / (b - a)
    }

    /// Inverse of [`Self::index_of_time`].
    pub fn time_of_index(&self, idx: f64, step_ms: i64) -> f64 {
        let step = step_ms.max(1) as f64;
        let (Some(first), Some(last)) = (self.bars.first(), self.bars.last()) else {
            return 0.0;
        };
        let n = self.bars.len();
        if idx <= 0.0 {
            return first.time as f64 + idx * step;
        }
        if idx >= (n - 1) as f64 {
            return last.time as f64 + (idx - (n - 1) as f64) * step;
        }
        let i = idx.floor() as usize;
        let frac = idx - i as f64;
        let a = self.bars[i].time as f64;
        let b = self.bars[i + 1].time as f64;
        a + frac * (b - a)
    }

    /// Heikin-Ashi transform of the series.
    pub fn heikin_ashi(&self) -> Vec<Bar> {
        let mut out: Vec<Bar> = Vec::with_capacity(self.bars.len());
        for b in &self.bars {
            let close = (b.open + b.high + b.low + b.close) / 4.0;
            let open = match out.last() {
                Some(p) => (p.open + p.close) / 2.0,
                None => (b.open + b.close) / 2.0,
            };
            out.push(Bar {
                time: b.time,
                open,
                high: b.high.max(open).max(close),
                low: b.low.min(open).min(close),
                close,
                volume: b.volume,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(time: i64, c: f64) -> Bar {
        Bar {
            time,
            open: c,
            high: c,
            low: c,
            close: c,
            volume: 1.0,
        }
    }

    #[test]
    fn upsert_replaces_appends_and_inserts() {
        let mut s = Series::new(vec![bar(0, 1.0), bar(60, 2.0)]);
        s.upsert(bar(60, 3.0));
        assert_eq!(s.len(), 2);
        assert_eq!(s.last().unwrap().close, 3.0);
        s.upsert(bar(120, 4.0));
        assert_eq!(s.len(), 3);
        s.upsert(bar(30, 9.0));
        assert_eq!(
            s.bars.iter().map(|b| b.time).collect::<Vec<_>>(),
            vec![0, 30, 60, 120]
        );
    }

    #[test]
    fn prepend_skips_overlap() {
        let mut s = Series::new(vec![bar(120, 1.0), bar(180, 1.0)]);
        let added = s.prepend(vec![bar(0, 1.0), bar(60, 1.0), bar(120, 5.0)]);
        assert_eq!(added, 2);
        assert_eq!(s.bars[2].close, 1.0);
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn time_index_roundtrip() {
        let s = Series::new((0..10).map(|i| bar(i * 60, 1.0)).collect());
        for idx in [-3.0, 0.0, 2.5, 9.0, 14.25] {
            let t = s.time_of_index(idx, 60);
            assert!((s.index_of_time(t, 60) - idx).abs() < 1e-9, "idx {idx}");
        }
    }

    #[test]
    fn heikin_ashi_first_bar() {
        let s = Series::new(vec![Bar {
            time: 0,
            open: 10.0,
            high: 12.0,
            low: 9.0,
            close: 11.0,
            volume: 0.0,
        }]);
        let ha = s.heikin_ashi();
        assert_eq!(ha[0].open, 10.5);
        assert_eq!(ha[0].close, 10.5);
        assert_eq!(ha[0].high, 12.0);
        assert_eq!(ha[0].low, 9.0);
    }
}
