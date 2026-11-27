//! Price alert engine shared by the PrimeView server and the desktop/web app.
//!
//! The engine is pure: it never reads the clock or touches the network. Callers
//! feed it prices with [`Engine::on_price`] (time in Unix milliseconds) and get
//! back the alerts that fired.
//!
//! # Condition semantics
//!
//! Let `prev` be the previous price recorded for the symbol and `price` the new
//! one, `level` the alert level:
//!
//! * [`Condition::CrossingUp`]: `prev < level && price >= level`
//! * [`Condition::CrossingDown`]: `prev > level && price <= level`
//! * [`Condition::Crossing`]: either of the above
//! * [`Condition::Above`]: `price > level` (strict, no previous price needed)
//! * [`Condition::Below`]: `price < level` (strict, no previous price needed)
//!
//! Touching the level counts as crossing it, but starting *at* the level does
//! not, so a price that goes `below -> level -> above` fires a crossing exactly
//! once (on the tick that reaches the level). Crossing conditions never fire on
//! the first price seen for a symbol; that tick only records the price.
//!
//! # Frequency, expiry and activity
//!
//! * Inactive alerts never fire.
//! * An alert with `expires_ms = Some(t)` is expired once `time_ms >= t`: it is
//!   deactivated and does not fire.
//! * [`Frequency::Once`] fires once, then the alert is deactivated.
//! * [`Frequency::EveryTime`] fires whenever the condition holds and at least
//!   `cooldown_ms` has passed since `last_fired_ms`.
//!
//! Non-finite prices (NaN, infinities) are ignored entirely.
//!
//! # Symbols
//!
//! Symbols are `PROVIDER:TICKER`, uppercase: `BINANCE:BTCUSDT`,
//! `TWELVEDATA:AAPL`, `TWELVEDATA:EUR/USD` (tickers may contain `/`). A bare
//! ticker means `BINANCE`. See [`normalize_symbol`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Provider used when a symbol has no `PROVIDER:` prefix.
pub const DEFAULT_PROVIDER: &str = "BINANCE";

/// Canonicalize a symbol to `PROVIDER:TICKER`, uppercase, trimmed.
/// A bare ticker (or one with an empty provider) gets the `BINANCE:` prefix.
/// The bare ticker `DEMO` maps to `DEMO:DEMO` (the app's synthetic feed).
pub fn normalize_symbol(s: &str) -> String {
    let s = s.trim().to_uppercase();
    match s.split_once(':') {
        Some((provider, ticker)) => {
            let provider = provider.trim();
            let provider = if provider.is_empty() {
                DEFAULT_PROVIDER
            } else {
                provider
            };
            format!("{provider}:{}", ticker.trim())
        }
        // The app's offline demo feed is its own provider.
        None if s == "DEMO" => "DEMO:DEMO".to_string(),
        None => format!("{DEFAULT_PROVIDER}:{s}"),
    }
}

/// Split a symbol into `(provider, ticker)` after normalizing it.
pub fn split_symbol(s: &str) -> (String, String) {
    let n = normalize_symbol(s);
    let (p, t) = n.split_once(':').expect("normalized symbols contain ':'");
    (p.to_string(), t.to_string())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Condition {
    Crossing(f64),
    CrossingUp(f64),
    CrossingDown(f64),
    Above(f64),
    Below(f64),
}

impl Condition {
    /// The price level of the condition.
    pub fn level(&self) -> f64 {
        match *self {
            Condition::Crossing(l)
            | Condition::CrossingUp(l)
            | Condition::CrossingDown(l)
            | Condition::Above(l)
            | Condition::Below(l) => l,
        }
    }

    /// Human readable description, e.g. `"crossing up 65000"`.
    pub fn describe(&self) -> String {
        let (word, level) = match *self {
            Condition::Crossing(l) => ("crossing", l),
            Condition::CrossingUp(l) => ("crossing up", l),
            Condition::CrossingDown(l) => ("crossing down", l),
            Condition::Above(l) => ("above", l),
            Condition::Below(l) => ("below", l),
        };
        format!("{word} {level}")
    }

    /// Whether the condition is met for a move from `prev` to `price`.
    pub fn is_met(&self, prev: Option<f64>, price: f64) -> bool {
        let up = |l: f64| prev.is_some_and(|p| p < l && price >= l);
        let down = |l: f64| prev.is_some_and(|p| p > l && price <= l);
        match *self {
            Condition::Crossing(l) => up(l) || down(l),
            Condition::CrossingUp(l) => up(l),
            Condition::CrossingDown(l) => down(l),
            Condition::Above(l) => price > l,
            Condition::Below(l) => price < l,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Frequency {
    Once,
    EveryTime { cooldown_ms: i64 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    pub id: u64,
    pub symbol: String,
    pub condition: Condition,
    pub frequency: Frequency,
    pub message: String,
    pub active: bool,
    pub created_ms: i64,
    pub last_fired_ms: Option<i64>,
    pub expires_ms: Option<i64>,
}

impl Alert {
    /// Convenience constructor: active, never fired, no expiry, id assigned on add.
    pub fn new(
        symbol: impl Into<String>,
        condition: Condition,
        frequency: Frequency,
        created_ms: i64,
    ) -> Self {
        Alert {
            id: 0,
            symbol: symbol.into(),
            condition,
            frequency,
            message: String::new(),
            active: true,
            created_ms,
            last_fired_ms: None,
            expires_ms: None,
        }
    }

    /// Whether the alert is expired at `time_ms`.
    pub fn is_expired(&self, time_ms: i64) -> bool {
        self.expires_ms.is_some_and(|e| time_ms >= e)
    }

    /// The message used when firing: the alert's message, or
    /// `"<SYMBOL> <describe>"` when it is empty.
    pub fn effective_message(&self) -> String {
        if self.message.trim().is_empty() {
            format!("{} {}", self.symbol, self.condition.describe())
        } else {
            self.message.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fired {
    pub alert_id: u64,
    pub symbol: String,
    pub price: f64,
    pub time_ms: i64,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Engine {
    alerts: Vec<Alert>,
    /// Last price seen per normalized symbol.
    #[serde(default)]
    last_prices: BTreeMap<String, f64>,
    /// Last id handed out (ids start at 1).
    #[serde(default)]
    next_id: u64,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an alert. Assigns a fresh id (ignoring `alert.id`), normalizes the
    /// symbol and returns the id.
    pub fn add(&mut self, mut alert: Alert) -> u64 {
        // Guard against a deserialized engine whose counter lags its alerts.
        let max_id = self.alerts.iter().map(|a| a.id).max().unwrap_or(0);
        self.next_id = self.next_id.max(max_id) + 1;
        alert.id = self.next_id;
        alert.symbol = normalize_symbol(&alert.symbol);
        self.alerts.push(alert);
        self.next_id
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.alerts.len();
        self.alerts.retain(|a| a.id != id);
        self.alerts.len() != before
    }

    pub fn set_active(&mut self, id: u64, active: bool) -> bool {
        match self.alerts.iter_mut().find(|a| a.id == id) {
            Some(a) => {
                a.active = active;
                true
            }
            None => false,
        }
    }

    pub fn alerts(&self) -> &[Alert] {
        &self.alerts
    }

    /// Look up one alert by id.
    pub fn get(&self, id: u64) -> Option<&Alert> {
        self.alerts.iter().find(|a| a.id == id)
    }

    /// Distinct symbols that have at least one active alert, sorted.
    pub fn symbols(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .alerts
            .iter()
            .filter(|a| a.active)
            .map(|a| a.symbol.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Last price recorded for a symbol (normalized before lookup).
    pub fn last_price(&self, symbol: &str) -> Option<f64> {
        self.last_prices.get(&normalize_symbol(symbol)).copied()
    }

    /// Deactivate every active alert that is expired at `now_ms`.
    /// Returns how many were deactivated. `on_price` does this too, but only
    /// when prices arrive; callers with a clock can sweep periodically.
    pub fn expire(&mut self, now_ms: i64) -> usize {
        let mut n = 0;
        for a in self.alerts.iter_mut() {
            if a.active && a.is_expired(now_ms) {
                a.active = false;
                n += 1;
            }
        }
        n
    }

    /// Feed a price tick. Returns the alerts that fired, in alert order.
    pub fn on_price(&mut self, symbol: &str, price: f64, time_ms: i64) -> Vec<Fired> {
        if !price.is_finite() {
            return Vec::new();
        }
        let symbol = normalize_symbol(symbol);
        self.expire(time_ms);
        let prev = self.last_prices.insert(symbol.clone(), price);

        let mut fired = Vec::new();
        for a in self.alerts.iter_mut() {
            if !a.active || a.symbol != symbol || !a.condition.is_met(prev, price) {
                continue;
            }
            match a.frequency {
                Frequency::Once => a.active = false,
                Frequency::EveryTime { cooldown_ms } => {
                    if a.last_fired_ms
                        .is_some_and(|last| time_ms.saturating_sub(last) < cooldown_ms)
                    {
                        continue;
                    }
                }
            }
            a.last_fired_ms = Some(time_ms);
            fired.push(Fired {
                alert_id: a.id,
                symbol: symbol.clone(),
                price,
                time_ms,
                message: a.effective_message(),
            });
        }
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn once(sym: &str, c: Condition) -> Alert {
        Alert::new(sym, c, Frequency::Once, 0)
    }

    fn every(sym: &str, c: Condition, cooldown_ms: i64) -> Alert {
        Alert::new(sym, c, Frequency::EveryTime { cooldown_ms }, 0)
    }

    /// Feed a sequence of prices (time = index * 1000) and return the
    /// indices of ticks on which anything fired.
    fn fire_ticks(e: &mut Engine, sym: &str, prices: &[f64]) -> Vec<usize> {
        prices
            .iter()
            .enumerate()
            .filter(|(i, p)| !e.on_price(sym, **p, *i as i64 * 1000).is_empty())
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn normalize() {
        assert_eq!(normalize_symbol("btcusdt"), "BINANCE:BTCUSDT");
        assert_eq!(normalize_symbol(" BTCUSDT "), "BINANCE:BTCUSDT");
        assert_eq!(normalize_symbol("binance:ethusdt"), "BINANCE:ETHUSDT");
        assert_eq!(normalize_symbol("twelvedata:aapl"), "TWELVEDATA:AAPL");
        assert_eq!(normalize_symbol("TwelveData:eur/usd"), "TWELVEDATA:EUR/USD");
        assert_eq!(
            normalize_symbol(" twelvedata : btc/usd "),
            "TWELVEDATA:BTC/USD"
        );
        // only the first ':' separates provider from ticker
        assert_eq!(normalize_symbol("twelvedata:a:b"), "TWELVEDATA:A:B");
        assert_eq!(normalize_symbol(":btcusdt"), "BINANCE:BTCUSDT");
        assert_eq!(normalize_symbol("BINANCE:BTCUSDT"), "BINANCE:BTCUSDT");
        assert_eq!(
            split_symbol("twelvedata:eur/usd"),
            ("TWELVEDATA".to_string(), "EUR/USD".to_string())
        );
    }

    #[test]
    fn describe() {
        assert_eq!(
            Condition::CrossingUp(65000.0).describe(),
            "crossing up 65000"
        );
        assert_eq!(Condition::CrossingDown(1.5).describe(), "crossing down 1.5");
        assert_eq!(Condition::Crossing(10.0).describe(), "crossing 10");
        assert_eq!(Condition::Above(0.25).describe(), "above 0.25");
        assert_eq!(Condition::Below(-3.0).describe(), "below -3");
    }

    #[test]
    fn add_assigns_ids_and_normalizes() {
        let mut e = Engine::new();
        let mut a = once("btcusdt", Condition::Above(1.0));
        a.id = 999;
        let id1 = e.add(a);
        let id2 = e.add(once("twelvedata:aapl", Condition::Above(1.0)));
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(e.alerts()[0].id, 1);
        assert_eq!(e.alerts()[0].symbol, "BINANCE:BTCUSDT");
        assert_eq!(e.alerts()[1].symbol, "TWELVEDATA:AAPL");
        assert_eq!(e.get(2).unwrap().symbol, "TWELVEDATA:AAPL");
        // ids are not reused after removal
        assert!(e.remove(2));
        assert_eq!(e.add(once("x", Condition::Above(1.0))), 3);
    }

    #[test]
    fn remove_and_set_active() {
        let mut e = Engine::new();
        let id = e.add(once("BTCUSDT", Condition::Above(1.0)));
        assert!(e.set_active(id, false));
        assert!(!e.alerts()[0].active);
        assert!(!e.set_active(42, true));
        assert!(e.remove(id));
        assert!(!e.remove(id));
        assert!(e.alerts().is_empty());
    }

    #[test]
    fn symbols_distinct_active_only() {
        let mut e = Engine::new();
        e.add(once("btcusdt", Condition::Above(1.0)));
        e.add(once("BINANCE:BTCUSDT", Condition::Below(1.0)));
        e.add(once("twelvedata:aapl", Condition::Above(1.0)));
        let id = e.add(once("ethusdt", Condition::Above(1.0)));
        e.set_active(id, false);
        assert_eq!(e.symbols(), vec!["BINANCE:BTCUSDT", "TWELVEDATA:AAPL"]);
    }

    #[test]
    fn crossing_up_boundaries() {
        let mut e = Engine::new();
        e.add(every("S", Condition::CrossingUp(10.0), 0));
        // first tick only records
        assert_eq!(fire_ticks(&mut e, "S", &[11.0]), Vec::<usize>::new());

        let mut e = Engine::new();
        e.add(every("S", Condition::CrossingUp(10.0), 0));
        // below -> exactly level fires; level -> above does not fire again
        assert_eq!(fire_ticks(&mut e, "S", &[9.0, 10.0, 11.0]), vec![1]);

        let mut e = Engine::new();
        e.add(every("S", Condition::CrossingUp(10.0), 0));
        // starting at the level and moving up is not a crossing
        assert_eq!(fire_ticks(&mut e, "S", &[10.0, 11.0]), Vec::<usize>::new());

        let mut e = Engine::new();
        e.add(every("S", Condition::CrossingUp(10.0), 0));
        // level -> below -> above fires on re-cross
        assert_eq!(fire_ticks(&mut e, "S", &[10.0, 9.0, 12.0]), vec![2]);

        let mut e = Engine::new();
        e.add(every("S", Condition::CrossingUp(10.0), 0));
        // downward crossings are ignored
        assert_eq!(
            fire_ticks(&mut e, "S", &[11.0, 9.0, 8.0]),
            Vec::<usize>::new()
        );

        let mut e = Engine::new();
        e.add(every("S", Condition::CrossingUp(10.0), 0));
        // gap through the level
        assert_eq!(fire_ticks(&mut e, "S", &[5.0, 15.0, 5.0, 15.0]), vec![1, 3]);
    }

    #[test]
    fn crossing_down_boundaries() {
        let mut e = Engine::new();
        e.add(every("S", Condition::CrossingDown(10.0), 0));
        assert_eq!(fire_ticks(&mut e, "S", &[11.0, 10.0, 9.0]), vec![1]);

        let mut e = Engine::new();
        e.add(every("S", Condition::CrossingDown(10.0), 0));
        assert_eq!(fire_ticks(&mut e, "S", &[10.0, 9.0]), Vec::<usize>::new());

        let mut e = Engine::new();
        e.add(every("S", Condition::CrossingDown(10.0), 0));
        assert_eq!(
            fire_ticks(&mut e, "S", &[9.0, 11.0, 12.0]),
            Vec::<usize>::new()
        );

        let mut e = Engine::new();
        e.add(every("S", Condition::CrossingDown(10.0), 0));
        assert_eq!(fire_ticks(&mut e, "S", &[12.0, 8.0]), vec![1]);
    }

    #[test]
    fn crossing_either_direction() {
        let mut e = Engine::new();
        e.add(every("S", Condition::Crossing(10.0), 0));
        assert_eq!(
            fire_ticks(&mut e, "S", &[9.0, 11.0, 12.0, 9.0, 10.0, 10.0, 11.0]),
            vec![1, 3, 4]
        );
    }

    #[test]
    fn unchanged_price_never_crosses() {
        let mut e = Engine::new();
        e.add(every("S", Condition::Crossing(10.0), 0));
        assert_eq!(
            fire_ticks(&mut e, "S", &[10.0, 10.0, 10.0]),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn above_below_strict_and_no_prev_needed() {
        let mut e = Engine::new();
        e.add(every("S", Condition::Above(10.0), 0));
        // fires on the very first tick; level itself is not above
        assert_eq!(
            fire_ticks(&mut e, "S", &[11.0, 10.0, 9.0, 10.5]),
            vec![0, 3]
        );

        let mut e = Engine::new();
        e.add(every("S", Condition::Below(10.0), 0));
        assert_eq!(fire_ticks(&mut e, "S", &[9.0, 10.0, 11.0, 9.5]), vec![0, 3]);
    }

    #[test]
    fn once_fires_then_deactivates() {
        let mut e = Engine::new();
        let id = e.add(once("btcusdt", Condition::Above(1.0)));
        let f = e.on_price("BINANCE:BTCUSDT", 65000.0, 1234);
        assert_eq!(
            f,
            vec![Fired {
                alert_id: id,
                symbol: "BINANCE:BTCUSDT".into(),
                price: 65000.0,
                time_ms: 1234,
                message: "BINANCE:BTCUSDT above 1".into(),
            }]
        );
        let a = e.get(id).unwrap();
        assert!(!a.active);
        assert_eq!(a.last_fired_ms, Some(1234));
        assert!(e.on_price("btcusdt", 65001.0, 2000).is_empty());
        assert!(e.symbols().is_empty());
    }

    #[test]
    fn every_time_respects_cooldown() {
        let mut e = Engine::new();
        let id = e.add(every("S", Condition::Above(10.0), 5000));
        assert_eq!(e.on_price("S", 11.0, 0).len(), 1);
        assert!(e.on_price("S", 11.0, 4999).is_empty());
        assert_eq!(e.on_price("S", 11.0, 5000).len(), 1);
        assert!(e.on_price("S", 11.0, 6000).is_empty());
        assert_eq!(e.on_price("S", 11.0, 10_000).len(), 1);
        let a = e.get(id).unwrap();
        assert!(a.active);
        assert_eq!(a.last_fired_ms, Some(10_000));
    }

    #[test]
    fn cooldown_suppressed_crossing_is_not_queued() {
        // A crossing that happens during the cooldown is dropped, not deferred.
        let mut e = Engine::new();
        e.add(every("S", Condition::Crossing(10.0), 10_000));
        e.on_price("S", 9.0, 0);
        assert_eq!(e.on_price("S", 11.0, 1000).len(), 1);
        assert!(e.on_price("S", 9.0, 2000).is_empty()); // in cooldown
        assert!(e.on_price("S", 8.0, 20_000).is_empty()); // no crossing now
        assert_eq!(e.on_price("S", 12.0, 21_000).len(), 1);
    }

    #[test]
    fn every_time_zero_cooldown_fires_each_tick() {
        let mut e = Engine::new();
        e.add(every("S", Condition::Above(10.0), 0));
        for t in 0..5 {
            assert_eq!(e.on_price("S", 11.0, t).len(), 1);
        }
    }

    #[test]
    fn expired_alerts_deactivate_without_firing() {
        let mut e = Engine::new();
        let mut a = every("S", Condition::Above(10.0), 0);
        a.expires_ms = Some(1000);
        let id = e.add(a);
        assert_eq!(e.on_price("S", 11.0, 999).len(), 1);
        assert!(e.on_price("S", 11.0, 1000).is_empty());
        assert!(!e.get(id).unwrap().active);
        // stays inactive
        assert!(e.on_price("S", 11.0, 500).is_empty());
    }

    #[test]
    fn expiry_applies_on_any_symbol_tick_and_sweep() {
        let mut e = Engine::new();
        let mut a = once("A", Condition::Above(10.0));
        a.expires_ms = Some(100);
        let id = e.add(a);
        e.on_price("B", 1.0, 200);
        assert!(!e.get(id).unwrap().active);

        let mut e = Engine::new();
        let mut a = once("A", Condition::Above(10.0));
        a.expires_ms = Some(100);
        let id = e.add(a);
        assert_eq!(e.expire(99), 0);
        assert_eq!(e.expire(100), 1);
        assert!(!e.get(id).unwrap().active);
        assert_eq!(e.expire(100), 0);
    }

    #[test]
    fn inactive_never_fire_but_price_is_tracked() {
        let mut e = Engine::new();
        let id = e.add(every("S", Condition::CrossingUp(10.0), 0));
        e.set_active(id, false);
        assert!(e.on_price("S", 9.0, 0).is_empty());
        assert!(e.on_price("S", 11.0, 1).is_empty());
        assert_eq!(e.last_price("s"), Some(11.0));
        // re-activated: needs a fresh crossing relative to the tracked price
        e.set_active(id, true);
        assert!(e.on_price("S", 12.0, 2).is_empty());
        assert!(e.on_price("S", 9.0, 3).is_empty());
        assert_eq!(e.on_price("S", 10.0, 4).len(), 1);
    }

    #[test]
    fn prices_are_per_symbol() {
        let mut e = Engine::new();
        e.add(every("A", Condition::CrossingUp(10.0), 0));
        e.on_price("A", 9.0, 0);
        // another symbol's price must not count as A's previous price
        assert!(e.on_price("B", 11.0, 1).is_empty());
        assert_eq!(e.on_price("A", 11.0, 2).len(), 1);
    }

    #[test]
    fn alert_added_after_prices_uses_known_prev() {
        let mut e = Engine::new();
        e.on_price("S", 9.0, 0);
        e.add(once("S", Condition::CrossingUp(10.0)));
        assert_eq!(e.on_price("S", 10.0, 1).len(), 1);
    }

    #[test]
    fn non_finite_prices_ignored() {
        let mut e = Engine::new();
        e.add(every("S", Condition::Crossing(10.0), 0));
        e.on_price("S", 9.0, 0);
        assert!(e.on_price("S", f64::NAN, 1).is_empty());
        assert!(e.on_price("S", f64::INFINITY, 2).is_empty());
        assert_eq!(e.last_price("S"), Some(9.0));
        assert_eq!(e.on_price("S", 11.0, 3).len(), 1);
    }

    #[test]
    fn custom_message_and_multiple_fires_in_order() {
        let mut e = Engine::new();
        let mut a = once("S", Condition::Above(1.0));
        a.message = "hello".into();
        let id1 = e.add(a);
        let id2 = e.add(once("S", Condition::Above(2.0)));
        e.add(once("S", Condition::Above(100.0)));
        let f = e.on_price("S", 5.0, 0);
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].alert_id, f[0].message.as_str()), (id1, "hello"));
        assert_eq!(
            (f[1].alert_id, f[1].message.as_str()),
            (id2, "BINANCE:S above 2")
        );
    }

    #[test]
    fn serde_roundtrip_keeps_state() {
        let mut e = Engine::new();
        e.add(every("S", Condition::CrossingUp(10.0), 0));
        e.on_price("S", 9.0, 0);
        let json = serde_json::to_string(&e).unwrap();
        let mut back: Engine = serde_json::from_str(&json).unwrap();
        assert_eq!(back.alerts(), e.alerts());
        assert_eq!(back.on_price("S", 10.0, 1).len(), 1);
        assert_eq!(back.add(once("S", Condition::Above(1.0))), 2);
    }

    #[test]
    fn id_counter_survives_missing_field() {
        // An engine file without next_id must not hand out duplicate ids.
        let json = r#"{"alerts":[{"id":7,"symbol":"BINANCE:S","condition":{"Above":1.0},
            "frequency":"Once","message":"","active":true,"created_ms":0,
            "last_fired_ms":null,"expires_ms":null}]}"#;
        let mut e: Engine = serde_json::from_str(json).unwrap();
        assert_eq!(e.add(once("S", Condition::Above(1.0))), 8);
    }

    #[test]
    fn serde_shapes() {
        assert_eq!(
            serde_json::to_string(&Condition::CrossingUp(1.5)).unwrap(),
            r#"{"CrossingUp":1.5}"#
        );
        assert_eq!(
            serde_json::to_string(&Frequency::Once).unwrap(),
            r#""Once""#
        );
        assert_eq!(
            serde_json::to_string(&Frequency::EveryTime { cooldown_ms: 60000 }).unwrap(),
            r#"{"EveryTime":{"cooldown_ms":60000}}"#
        );
    }
}
