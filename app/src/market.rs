//! Owns one chart's series and keeps it fed: initial history, live updates
//! (WebSocket for Binance, polling for Twelve Data), scroll-back history,
//! reconnects, and the offline demo feed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui;
use primeview_core::{Bar, Series, Timeframe};
use primeview_data::binance::{self, Stream};
use primeview_data::demo::DemoFeed;
use primeview_data::{Provider, StreamStatus, SymbolId, Ticker, twelvedata};
use web_time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RECONNECT_AFTER: Duration = Duration::from_secs(3);
/// Twelve Data free tier: 8 requests/minute shared by every chart + watchlist.
const TD_POLL: Duration = Duration::from_secs(60);
const TD_HISTORY: usize = 1500;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Copy)]
enum Kind {
    Initial,
    Older,
    Latest,
}

struct Msg {
    generation: u64,
    kind: Kind,
    result: Result<Vec<Bar>, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Loading,
    Connecting,
    Live,
    /// Updated by polling (not tick-by-tick).
    Polling,
    Demo,
    Error(String),
}

pub struct Market {
    pub symbol: SymbolId,
    pub timeframe: Timeframe,
    pub series: Series,
    /// Bumped whenever `series` changes, so dependents can cache.
    pub revision: u64,
    pub status: Status,
    pub td_key: String,
    generation: u64,
    inbox: Arc<Mutex<Vec<Msg>>>,
    stream: Option<Stream>,
    last_connect: Option<Instant>,
    last_poll: Option<Instant>,
    demo: Option<DemoFeed>,
    last_demo_tick: Option<Instant>,
    loading_older: bool,
    history_exhausted: bool,
    ctx: egui::Context,
}

impl Market {
    pub fn new(ctx: egui::Context, td_key: String) -> Self {
        Self {
            symbol: SymbolId::demo(),
            timeframe: Timeframe::H1,
            series: Series::default(),
            revision: 0,
            status: Status::Loading,
            td_key,
            generation: 0,
            inbox: Arc::default(),
            stream: None,
            last_connect: None,
            last_poll: None,
            demo: None,
            last_demo_tick: None,
            loading_older: false,
            history_exhausted: false,
            ctx,
        }
    }

    /// Switch to a symbol/timeframe and start loading it.
    pub fn load(&mut self, symbol: &SymbolId, timeframe: Timeframe) {
        self.generation += 1;
        self.symbol = symbol.clone();
        self.timeframe = timeframe;
        self.series = Series::default();
        self.revision += 1;
        self.stream = None;
        self.demo = None;
        self.last_poll = None;
        self.loading_older = false;
        self.history_exhausted = false;

        match symbol.provider {
            Provider::Demo => {
                let seed = symbol
                    .ticker
                    .bytes()
                    .fold(timeframe.millis() as u64, |h, b| {
                        h.wrapping_mul(31).wrapping_add(b as u64)
                    });
                let mut feed = DemoFeed::new(seed, timeframe);
                self.series = feed.history(3000, now_ms(), 100.0);
                self.demo = Some(feed);
                self.status = Status::Demo;
                self.history_exhausted = true;
                self.revision += 1;
            }
            Provider::Binance | Provider::TwelveData => {
                self.status = Status::Loading;
                self.fetch(Kind::Initial, None);
            }
        }
    }

    pub fn reload(&mut self) {
        let symbol = self.symbol.clone();
        self.load(&symbol, self.timeframe);
    }

    fn fetch(&self, kind: Kind, end_time: Option<i64>) {
        let inbox = self.inbox.clone();
        let ctx = self.ctx.clone();
        let generation = self.generation;
        let done = move |result: Result<Vec<Bar>, String>| {
            inbox.lock().unwrap().push(Msg {
                generation,
                kind,
                result,
            });
            ctx.request_repaint();
        };
        let ticker = &self.symbol.ticker;
        match self.symbol.provider {
            Provider::Binance => {
                binance::fetch_klines(ticker, self.timeframe, end_time, binance::MAX_LIMIT, done)
            }
            Provider::TwelveData => {
                let size = if matches!(kind, Kind::Latest) {
                    3
                } else {
                    TD_HISTORY
                };
                twelvedata::fetch_series(ticker, self.timeframe, end_time, size, &self.td_key, done)
            }
            Provider::Demo => {}
        }
    }

    /// Ask for older bars (when the user scrolls near the left edge).
    pub fn request_older(&mut self) {
        if self.loading_older || self.history_exhausted || self.series.is_empty() {
            return;
        }
        if !matches!(
            self.status,
            Status::Live | Status::Connecting | Status::Polling
        ) {
            return;
        }
        self.loading_older = true;
        let first = self.series.bars[0].time;
        self.fetch(Kind::Older, Some(first - 1));
    }

    fn connect(&mut self) {
        let ctx = self.ctx.clone();
        self.last_connect = Some(Instant::now());
        match Stream::klines(&self.symbol.ticker, self.timeframe, move || {
            ctx.request_repaint()
        }) {
            Ok(s) => {
                self.stream = Some(s);
                self.status = Status::Connecting;
            }
            Err(e) => self.status = Status::Error(format!("stream: {e}")),
        }
    }

    fn start_live(&mut self) {
        match self.symbol.provider {
            Provider::Binance => self.connect(),
            Provider::TwelveData => {
                self.status = Status::Polling;
                self.last_poll = Some(Instant::now());
            }
            Provider::Demo => {}
        }
    }

    /// Call once per frame.
    pub fn update(&mut self) {
        let msgs: Vec<Msg> = std::mem::take(&mut *self.inbox.lock().unwrap());
        for Msg {
            generation,
            kind,
            result,
        } in msgs
        {
            if generation != self.generation {
                continue;
            }
            match (kind, result) {
                (Kind::Initial, Ok(bars)) if bars.is_empty() => {
                    self.status = Status::Error("no data for this symbol/timeframe".into());
                }
                (Kind::Initial, Ok(bars)) => {
                    self.series = Series::new(bars);
                    self.revision += 1;
                    self.start_live();
                }
                (Kind::Initial, Err(e)) => self.status = Status::Error(e),
                (Kind::Older, Ok(bars)) => {
                    self.loading_older = false;
                    if self.series.prepend(bars) == 0 {
                        self.history_exhausted = true;
                    } else {
                        self.revision += 1;
                    }
                }
                (Kind::Older, Err(e)) => {
                    self.loading_older = false;
                    self.history_exhausted = true;
                    log::warn!("older history failed: {e}");
                }
                (Kind::Latest, Ok(bars)) => {
                    for b in bars {
                        self.series.upsert(b);
                    }
                    self.revision += 1;
                    self.status = Status::Polling;
                }
                (Kind::Latest, Err(e)) => log::warn!("poll failed: {e}"),
            }
        }

        if let Some(stream) = &mut self.stream {
            let mut changed = false;
            for text in stream.poll() {
                if let Some(bar) = binance::parse_kline_event(&text) {
                    self.series.upsert(bar);
                    changed = true;
                }
            }
            if changed {
                self.revision += 1;
            }
            self.status = match &stream.status {
                StreamStatus::Live => Status::Live,
                StreamStatus::Connecting => Status::Connecting,
                StreamStatus::Closed | StreamStatus::Error(_) => {
                    if self
                        .last_connect
                        .is_none_or(|t| t.elapsed() >= RECONNECT_AFTER)
                    {
                        self.connect();
                    } else {
                        self.ctx.request_repaint_after(RECONNECT_AFTER);
                    }
                    Status::Connecting
                }
            };
        }

        if let Some(last) = self.last_poll {
            if last.elapsed() >= TD_POLL {
                self.last_poll = Some(Instant::now());
                self.fetch(Kind::Latest, None);
            }
            self.ctx.request_repaint_after(TD_POLL);
        }

        if let Some(feed) = &mut self.demo {
            if self
                .last_demo_tick
                .is_none_or(|t| t.elapsed() >= Duration::from_millis(400))
            {
                feed.tick(&mut self.series, now_ms());
                self.revision += 1;
                self.last_demo_tick = Some(Instant::now());
            }
            self.ctx.request_repaint_after(Duration::from_millis(400));
        }
    }

    /// Last traded price, for alerts.
    pub fn last_price(&self) -> Option<(f64, i64)> {
        self.series.last().map(|b| (b.close, b.time))
    }
}

/// Live quotes for the watchlist: one Binance WebSocket for crypto, one
/// batched Twelve Data request per minute for everything else.
pub struct Watchlist {
    /// Keyed by canonical symbol (`BINANCE:BTCUSDT`).
    pub tickers: HashMap<String, Ticker>,
    stream: Option<Stream>,
    subscribed: Vec<String>,
    last_connect: Option<Instant>,
    td_inbox: Arc<Mutex<Vec<Ticker>>>,
    td_last_poll: Option<Instant>,
    td_subscribed: Vec<String>,
    ctx: egui::Context,
}

impl Watchlist {
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            tickers: HashMap::new(),
            stream: None,
            subscribed: Vec::new(),
            last_connect: None,
            td_inbox: Arc::default(),
            td_last_poll: None,
            td_subscribed: Vec::new(),
            ctx,
        }
    }

    pub fn update(&mut self, symbols: &[String], td_key: &str) {
        let ids: Vec<SymbolId> = symbols.iter().filter_map(|s| SymbolId::parse(s)).collect();
        let tickers_of = |p: Provider| -> Vec<String> {
            ids.iter()
                .filter(|s| s.provider == p)
                .map(|s| s.ticker.clone())
                .collect()
        };

        // Binance
        let wanted = tickers_of(Provider::Binance);
        let dead = self
            .stream
            .as_ref()
            .is_some_and(|s| matches!(s.status, StreamStatus::Closed | StreamStatus::Error(_)));
        let retry_due = self
            .last_connect
            .is_none_or(|t| t.elapsed() >= RECONNECT_AFTER * 2);
        if (wanted != self.subscribed || (dead && retry_due)) && !wanted.is_empty() {
            let ctx = self.ctx.clone();
            self.stream = Stream::tickers(&wanted, move || ctx.request_repaint()).ok();
            self.subscribed = wanted;
            self.last_connect = Some(Instant::now());
        }
        if let Some(s) = &mut self.stream {
            for text in s.poll() {
                if let Some(mut t) = binance::parse_mini_ticker(&text) {
                    t.symbol = format!("BINANCE:{}", t.symbol);
                    self.tickers.insert(t.symbol.clone(), t);
                }
            }
        }

        // Twelve Data
        let td = tickers_of(Provider::TwelveData);
        let due =
            self.td_last_poll.is_none_or(|t| t.elapsed() >= TD_POLL) || td != self.td_subscribed;
        if due && !td.is_empty() {
            self.td_last_poll = Some(Instant::now());
            self.td_subscribed = td.clone();
            let inbox = self.td_inbox.clone();
            let ctx = self.ctx.clone();
            twelvedata::fetch_quotes(&td, td_key, move |res| match res {
                Ok(q) => {
                    inbox.lock().unwrap().extend(q);
                    ctx.request_repaint();
                }
                Err(e) => log::warn!("watchlist quotes: {e}"),
            });
        }
        for t in std::mem::take(&mut *self.td_inbox.lock().unwrap()) {
            self.tickers.insert(t.symbol.clone(), t);
        }
    }
}
