//! Deterministic synthetic market data: a random walk with volatility
//! clustering, plus a "live" ticker that keeps the last candle moving.

use primeview_core::{Bar, Series, Timeframe};

/// Small, fast, deterministic PRNG (xorshift64*). No external deps.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal via Box-Muller.
    pub fn normal(&mut self) -> f64 {
        let u1 = self.f64().max(1e-12);
        let u2 = self.f64();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

pub struct DemoFeed {
    rng: Rng,
    tf: Timeframe,
    vol: f64,
    /// Price the walk mean-reverts toward, so long histories stay in range.
    anchor: f64,
}

impl DemoFeed {
    pub fn new(seed: u64, tf: Timeframe) -> Self {
        Self {
            rng: Rng::new(seed),
            tf,
            vol: 0.002,
            anchor: 100.0,
        }
    }

    fn step_vol(&mut self) -> f64 {
        // Mean-reverting volatility with random shocks (crude GARCH-like).
        let base = 0.002 * (self.tf.millis() as f64 / 60_000.0).sqrt().min(12.0);
        self.vol = (0.94 * self.vol + 0.06 * base + 0.002 * base * self.rng.normal().abs())
            .max(base * 0.2);
        self.vol
    }

    /// Generate `n` historical bars ending at the bar containing `now_ms`.
    pub fn history(&mut self, n: usize, now_ms: i64, start_price: f64) -> Series {
        let step = self.tf.millis();
        let last_open = now_ms - now_ms.rem_euclid(step);
        self.anchor = start_price;
        let mut price = start_price;
        let mut bars = Vec::with_capacity(n);
        for i in 0..n {
            let time = last_open - (n - 1 - i) as i64 * step;
            let vol = self.step_vol();
            let open = price;
            let pull = 0.01 * (self.anchor / open).ln();
            let close = open * (vol * self.rng.normal() + pull).exp();
            let wick = vol * open;
            let high = open.max(close) + wick * self.rng.f64();
            let low = open.min(close) - wick * self.rng.f64();
            let volume =
                50.0 + 1_000.0 * (close - open).abs() / open / vol.max(1e-9) * self.rng.f64();
            bars.push(Bar {
                time,
                open,
                high,
                low,
                close,
                volume,
            });
            price = close;
        }
        Series::new(bars)
    }

    /// Advance the live candle by one tick. Starts a new candle when the
    /// wall clock crosses into the next period.
    pub fn tick(&mut self, series: &mut Series, now_ms: i64) {
        let step = self.tf.millis();
        let Some(last) = series.last().copied() else {
            return;
        };
        let open_time = now_ms - now_ms.rem_euclid(step);
        let vol = self.vol * 0.15;
        let mut bar = if open_time > last.time {
            Bar {
                time: open_time,
                open: last.close,
                high: last.close,
                low: last.close,
                close: last.close,
                volume: 0.0,
            }
        } else {
            last
        };
        bar.close *= 1.0 + vol * self.rng.normal();
        bar.high = bar.high.max(bar.close);
        bar.low = bar.low.min(bar.close);
        bar.volume += 5.0 * self.rng.f64();
        series.upsert(bar);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_valid_and_deterministic() {
        let a = DemoFeed::new(7, Timeframe::H1).history(500, 1_700_000_000_000, 100.0);
        let b = DemoFeed::new(7, Timeframe::H1).history(500, 1_700_000_000_000, 100.0);
        assert_eq!(a.bars, b.bars);
        assert_eq!(a.len(), 500);
        for w in a.bars.windows(2) {
            assert_eq!(w[1].time - w[0].time, Timeframe::H1.millis());
        }
        for bar in &a.bars {
            assert!(bar.low <= bar.open.min(bar.close) && bar.high >= bar.open.max(bar.close));
            assert!(bar.low > 0.0);
        }
    }

    #[test]
    fn tick_rolls_over() {
        let tf = Timeframe::M1;
        let mut f = DemoFeed::new(1, tf);
        let now = 1_700_000_000_000;
        let mut s = f.history(10, now, 50.0);
        f.tick(&mut s, now);
        assert_eq!(s.len(), 10);
        f.tick(&mut s, now + tf.millis());
        assert_eq!(s.len(), 11);
    }
}
