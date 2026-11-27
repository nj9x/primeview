use std::collections::HashMap;

use eframe::egui::{self, Align, Color32, DragValue, Layout as EguiLayout, RichText, UiBuilder};
use primeview_chart::scale::{fmt_fixed, price_decimals};
use primeview_chart::{Chart, ChartState, ChartType, Drawing, DrawingKind, Theme, Tool};
use primeview_core::{Indicator, IndicatorOutput, Timeframe};
use primeview_data::{Provider, SymbolId, twelvedata};
use serde::{Deserialize, Serialize};

use crate::alerts::{AlertSettings, Alerts};
use crate::layout::{Layout, MAX_PANES};
use crate::market::{Market, Status, Watchlist};
use crate::scripts::{self, Editor, ScriptCache, ScriptInstance};

const UP: Color32 = Color32::from_rgb(46, 189, 133);
const DOWN: Color32 = Color32::from_rgb(229, 72, 77);
const ACCENT: Color32 = Color32::from_rgb(76, 141, 255);

/// Persistent configuration of one chart pane.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct PaneConfig {
    /// Canonical symbol, e.g. `BINANCE:BTCUSDT`.
    symbol: String,
    timeframe: Timeframe,
    indicators: Vec<Indicator>,
    scripts: Vec<ScriptInstance>,
    chart: ChartState,
}

impl Default for PaneConfig {
    fn default() -> Self {
        Self::new("BINANCE:BTCUSDT", Timeframe::H1, vec![])
    }
}

impl PaneConfig {
    fn new(symbol: &str, timeframe: Timeframe, indicators: Vec<Indicator>) -> Self {
        Self {
            symbol: symbol.into(),
            timeframe,
            indicators,
            scripts: Vec::new(),
            chart: ChartState::default(),
        }
    }

    fn symbol_id(&self) -> SymbolId {
        SymbolId::parse(&self.symbol).unwrap_or_else(SymbolId::demo)
    }
}

/// Everything that survives a restart.
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    layout: Layout,
    panes: Vec<PaneConfig>,
    active: usize,
    /// Drawings per canonical symbol, shared by every pane showing it.
    drawings: HashMap<String, Vec<Drawing>>,
    watchlist: Vec<String>,
    dark: bool,
    show_watchlist: bool,
    sync_crosshair: bool,
    sync_symbol: bool,
    sync_interval: bool,
    twelvedata_key: String,
    alerts: AlertSettings,
    sidebar: SideTab,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum SideTab {
    Watchlist,
    Alerts,
}

impl Default for Settings {
    fn default() -> Self {
        let mut watchlist = vec!["DEMO".to_string()];
        watchlist.extend(
            primeview_data::binance::default_symbols()
                .into_iter()
                .map(|s| format!("BINANCE:{s}")),
        );
        watchlist.extend([
            "TWELVEDATA:AAPL".to_string(),
            "TWELVEDATA:EUR/USD".to_string(),
        ]);
        let rsi = Indicator::Rsi { period: 14 };
        Self {
            layout: Layout::Single,
            panes: vec![
                PaneConfig::new(
                    "BINANCE:BTCUSDT",
                    Timeframe::H1,
                    vec![
                        Indicator::Ema { period: 50 },
                        Indicator::Bollinger {
                            period: 20,
                            mult: 2.0,
                        },
                        rsi.clone(),
                    ],
                ),
                PaneConfig::new(
                    "BINANCE:ETHUSDT",
                    Timeframe::H1,
                    vec![Indicator::Ema { period: 50 }],
                ),
                PaneConfig::new(
                    "TWELVEDATA:AAPL",
                    Timeframe::D1,
                    vec![Indicator::Sma { period: 50 }, rsi],
                ),
                PaneConfig::new(
                    "TWELVEDATA:EUR/USD",
                    Timeframe::H1,
                    vec![Indicator::Ema { period: 20 }],
                ),
            ],
            active: 0,
            drawings: HashMap::new(),
            watchlist,
            dark: true,
            show_watchlist: true,
            sync_crosshair: true,
            sync_symbol: false,
            sync_interval: false,
            twelvedata_key: twelvedata::DEMO_KEY.into(),
            alerts: AlertSettings::default(),
            sidebar: SideTab::Watchlist,
        }
    }
}

/// Runtime state of one pane (not persisted).
struct PaneRuntime {
    market: Market,
    outputs: Vec<IndicatorOutput>,
    outputs_key: (u64, Vec<Indicator>, Vec<ScriptInstance>),
    script_errors: Vec<Option<String>>,
}

pub struct PrimeViewApp {
    s: Settings,
    panes: Vec<PaneRuntime>,
    watch: Watchlist,
    alerts: Alerts,
    script_cache: ScriptCache,
    editor: Editor,
    symbol_input: String,
    input_for_pane: Option<usize>,
    add_symbol_input: String,
    key_input: String,
    /// Crosshair time from whichever pane was hovered last frame.
    sync_time: Option<(usize, i64)>,
}

impl PrimeViewApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut s: Settings = cc
            .storage
            .and_then(|st| eframe::get_value(st, eframe::APP_KEY))
            .unwrap_or_default();
        s.panes.resize_with(MAX_PANES, PaneConfig::default);
        s.active = s.active.min(s.layout.count() - 1);
        cc.egui_ctx.set_theme(if s.dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });

        let panes = s
            .panes
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut market = Market::new(cc.egui_ctx.clone(), s.twelvedata_key.clone());
                if i < s.layout.count() {
                    market.load(&p.symbol_id(), p.timeframe);
                }
                PaneRuntime {
                    market,
                    outputs: Vec::new(),
                    outputs_key: (u64::MAX, Vec::new(), Vec::new()),
                    script_errors: Vec::new(),
                }
            })
            .collect();
        Self {
            key_input: s.twelvedata_key.clone(),
            s,
            panes,
            watch: Watchlist::new(cc.egui_ctx.clone()),
            alerts: Alerts::new(cc.egui_ctx.clone()),
            script_cache: ScriptCache::default(),
            editor: Editor::default(),
            symbol_input: String::new(),
            input_for_pane: None,
            add_symbol_input: String::new(),
            sync_time: None,
        }
    }

    fn visible(&self) -> usize {
        self.s.layout.count()
    }

    /// Panes a symbol/interval change applies to.
    fn targets(&self, sync: bool) -> Vec<usize> {
        if sync {
            (0..self.visible()).collect()
        } else {
            vec![self.s.active]
        }
    }

    fn set_symbol(&mut self, sym: &SymbolId) {
        for i in self.targets(self.s.sync_symbol) {
            self.s.panes[i].symbol = sym.canonical();
            self.s.panes[i].chart.selected = None;
            self.s.panes[i].chart.reset_view();
            self.panes[i].market.load(sym, self.s.panes[i].timeframe);
        }
        self.input_for_pane = None;
    }

    fn set_timeframe(&mut self, tf: Timeframe) {
        for i in self.targets(self.s.sync_interval) {
            self.s.panes[i].timeframe = tf;
            self.s.panes[i].chart.reset_view();
            let sym = self.s.panes[i].symbol_id();
            self.panes[i].market.load(&sym, tf);
        }
    }

    fn set_layout(&mut self, layout: Layout) {
        let before = self.visible();
        self.s.layout = layout;
        // Start feeds for panes that just became visible.
        for i in before..self.visible() {
            if self.panes[i].market.series.is_empty() {
                let sym = self.s.panes[i].symbol_id();
                self.panes[i].market.load(&sym, self.s.panes[i].timeframe);
            }
        }
        self.s.active = self.s.active.min(layout.count() - 1);
    }

    fn apply_key(&mut self) {
        let key = self.key_input.trim().to_string();
        self.s.twelvedata_key = if key.is_empty() {
            twelvedata::DEMO_KEY.into()
        } else {
            key
        };
        for i in 0..self.visible() {
            self.panes[i].market.td_key = self.s.twelvedata_key.clone();
            if self.panes[i].market.symbol.provider == Provider::TwelveData {
                self.panes[i].market.reload();
            }
        }
    }

    fn refresh_indicators(&mut self, i: usize) {
        let cfg = &self.s.panes[i];
        let rt = &mut self.panes[i];
        let key = (
            rt.market.revision,
            cfg.indicators.clone(),
            cfg.scripts.clone(),
        );
        if key != rt.outputs_key {
            let bars = &rt.market.series.bars;
            rt.outputs = cfg.indicators.iter().map(|ind| ind.compute(bars)).collect();
            rt.script_errors.clear();
            for inst in &cfg.scripts {
                match self.script_cache.run(inst, bars) {
                    Ok(out) => {
                        rt.outputs.push(out);
                        rt.script_errors.push(None);
                    }
                    Err(e) => rt.script_errors.push(Some(e)),
                }
            }
            rt.outputs_key = key;
        }
    }

    // ------------------------------------------------------------------ UI

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let a = self.s.active;
        if self.input_for_pane != Some(a) {
            self.symbol_input = self.panes[a].market.symbol.ticker.clone();
            self.input_for_pane = Some(a);
        }
        ui.horizontal_centered(|ui| {
            ui.label(RichText::new("◆ PrimeView").strong().size(16.0).color(ACCENT));
            ui.separator();

            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.symbol_input)
                    .desired_width(120.0)
                    .hint_text("BTCUSDT, AAPL, EUR/USD")
                    .char_limit(32),
            );
            let submitted = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            resp.on_hover_text(
                "Crypto pairs (BTCUSDT) use Binance.\nStocks, ETFs, indices and forex (AAPL, EUR/USD) use Twelve Data.\nForce a provider with a prefix: BINANCE:ETHBTC, TD:XAU/USD.",
            );
            if submitted
                && let Some(sym) = SymbolId::parse(&self.symbol_input)
                && sym != self.panes[a].market.symbol
            {
                self.set_symbol(&sym);
            }
            ui.label(RichText::new(self.panes[a].market.symbol.provider.label()).small().weak());
            ui.separator();

            for tf in Timeframe::ALL {
                if ui.selectable_label(self.s.panes[a].timeframe == tf, tf.label()).clicked()
                    && self.s.panes[a].timeframe != tf
                {
                    self.set_timeframe(tf);
                }
            }
            ui.separator();

            let chart = &mut self.s.panes[a].chart;
            egui::ComboBox::from_id_salt("chart_type")
                .selected_text(chart.chart_type.label())
                .width(120.0)
                .show_ui(ui, |ui| {
                    for t in ChartType::ALL {
                        ui.selectable_value(&mut chart.chart_type, t, t.label());
                    }
                });
            ui.menu_button("ƒx Indicators", |ui| {
                indicator_menu(ui, &mut self.s.panes[a].indicators);
                scripts::menu_section(
                    ui,
                    &mut self.s.panes[a].scripts,
                    &self.panes[a].script_errors,
                    &mut self.script_cache,
                    &mut self.editor,
                );
            });
            ui.toggle_value(&mut self.s.panes[a].chart.show_volume, "Vol");
            ui.separator();
            ui.menu_button(RichText::new(self.s.layout.icon()).size(16.0), |ui| self.layout_menu(ui))
                .response
                .on_hover_text("Layout");

            ui.with_layout(EguiLayout::right_to_left(Align::Center), |ui| {
                let theme_icon = if self.s.dark { "☀" } else { "🌙" };
                if ui.button(theme_icon).on_hover_text("Toggle light/dark").clicked() {
                    self.s.dark = !self.s.dark;
                    ui.ctx().set_theme(if self.s.dark { egui::Theme::Dark } else { egui::Theme::Light });
                }
                ui.menu_button("⚙", |ui| self.settings_menu(ui));
                ui.toggle_value(&mut self.s.show_watchlist, "☰").on_hover_text("Watchlist & alerts");
                ui.separator();
                let m = &self.panes[a].market;
                let (color, text) = match &m.status {
                    Status::Live => (UP, "Live"),
                    Status::Polling => (UP, "Delayed · 1 min"),
                    Status::Connecting => (Color32::from_rgb(255, 170, 0), "Connecting"),
                    Status::Loading => (Color32::from_rgb(255, 170, 0), "Loading"),
                    Status::Demo => (Color32::from_rgb(150, 120, 255), "Demo data"),
                    Status::Error(_) => (DOWN, "Offline"),
                };
                ui.label(RichText::new(text).small());
                ui.label(RichText::new("●").color(color))
                    .on_hover_text(format!("{} bars loaded", m.series.len()));
            });
        });
    }

    fn layout_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(220.0);
        ui.label(RichText::new("Layout").strong());
        for l in Layout::ALL {
            let text = format!("{}  {}", l.icon(), l.label());
            if ui.selectable_label(self.s.layout == l, text).clicked() {
                self.set_layout(l);
                ui.close();
            }
        }
        ui.separator();
        ui.label(RichText::new("Sync across charts").strong());
        ui.checkbox(&mut self.s.sync_crosshair, "Crosshair");
        ui.checkbox(&mut self.s.sync_symbol, "Symbol");
        ui.checkbox(&mut self.s.sync_interval, "Interval");
    }

    fn settings_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(320.0);
        ui.label(RichText::new("Data providers").strong());
        ui.add_space(4.0);
        ui.label("Binance (crypto): no key needed, real-time.");
        ui.add_space(6.0);
        ui.label("Twelve Data (stocks, ETFs, indices, forex) API key:");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.key_input)
                    .password(true)
                    .desired_width(220.0),
            );
            if ui.button("Apply").clicked() {
                self.apply_key();
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Free keys:").small().weak());
            ui.hyperlink_to(
                RichText::new("twelvedata.com").small(),
                "https://twelvedata.com/pricing",
            );
        });
        if self.s.twelvedata_key == twelvedata::DEMO_KEY {
            ui.label(
                RichText::new(
                    "Using the shared 'demo' key: only a few symbols (e.g. AAPL, EUR/USD).",
                )
                .small()
                .color(Color32::from_rgb(255, 170, 0)),
            );
        }
        #[cfg(target_arch = "wasm32")]
        ui.label(
            RichText::new("Keys are stored in this browser's local storage.")
                .small()
                .weak(),
        );
    }

    fn tools_panel(&mut self, ui: &mut egui::Ui) {
        let sym = self.s.panes[self.s.active].symbol.clone();
        let drawings = &mut self.s.drawings;
        let st = &mut self.s.panes[self.s.active].chart;
        ui.vertical_centered(|ui| {
            ui.add_space(6.0);
            for tool in Tool::ALL {
                let btn = egui::Button::new(RichText::new(tool.icon()).size(17.0))
                    .selected(st.tool == tool)
                    .min_size(egui::vec2(32.0, 30.0));
                if ui.add(btn).on_hover_text(tool.label()).clicked() {
                    st.tool = tool;
                }
            }
            ui.separator();
            ui.toggle_value(&mut st.stay_in_draw_mode, RichText::new("🔒").size(15.0))
                .on_hover_text("Keep drawing tool active after placing");
            if ui
                .button(RichText::new("🗑").size(15.0))
                .on_hover_text("Remove all drawings on this symbol")
                .clicked()
            {
                drawings.remove(&sym);
                st.selected = None;
            }
            ui.separator();
            if ui
                .button(RichText::new("⟲").size(15.0))
                .on_hover_text("Reset view (Home)")
                .clicked()
            {
                st.reset_view();
            }
            ui.toggle_value(&mut st.show_grid, RichText::new("#").size(15.0))
                .on_hover_text("Grid");
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.s.sidebar, SideTab::Watchlist, "Watchlist");
            let n = self
                .s
                .alerts
                .engine
                .alerts()
                .iter()
                .filter(|a| a.active)
                .count()
                + self.alerts.server_alerts.len();
            let label = if n > 0 {
                format!("🔔 Alerts ({n})")
            } else {
                "🔔 Alerts".into()
            };
            ui.selectable_value(&mut self.s.sidebar, SideTab::Alerts, label);
        });
        ui.separator();
        match self.s.sidebar {
            SideTab::Watchlist => self.watchlist_panel(ui),
            SideTab::Alerts => {
                let a = self.s.active;
                let sym = self.s.panes[a].symbol.clone();
                let last = self.panes[a].market.last_price().map(|(p, _)| p);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        self.alerts.panel_ui(ui, &mut self.s.alerts, &sym, last);
                    });
            }
        }
    }

    fn watchlist_panel(&mut self, ui: &mut egui::Ui) {
        let active_symbol = self.s.panes[self.s.active].symbol.clone();
        let mut clicked = None;
        let mut remove = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("watch")
                    .num_columns(3)
                    .striped(true)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        for sym in &self.s.watchlist {
                            let Some(id) = SymbolId::parse(sym) else {
                                continue;
                            };
                            let canonical = id.canonical();
                            let r = ui
                                .selectable_label(
                                    canonical == active_symbol,
                                    RichText::new(&id.ticker).monospace(),
                                )
                                .on_hover_text(format!("{} · {}", id.ticker, id.provider.label()));
                            if r.clicked() {
                                clicked = Some(id.clone());
                            }
                            r.context_menu(|ui| {
                                if ui.button("Remove from watchlist").clicked() {
                                    remove = Some(sym.clone());
                                }
                            });
                            match (id.provider, self.watch.tickers.get(&canonical)) {
                                (Provider::Demo, _) => {
                                    ui.label(RichText::new("synthetic").weak().small());
                                    ui.label("");
                                }
                                (_, Some(t)) => {
                                    ui.with_layout(
                                        EguiLayout::right_to_left(Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(fmt_fixed(
                                                    t.last,
                                                    price_decimals(t.last),
                                                ))
                                                .monospace(),
                                            );
                                        },
                                    );
                                    let ch = t.change_pct();
                                    ui.label(
                                        RichText::new(format!("{ch:+.2}%"))
                                            .monospace()
                                            .color(if ch >= 0.0 { UP } else { DOWN }),
                                    );
                                }
                                (_, None) => {
                                    ui.label(RichText::new("…").weak());
                                    ui.label("");
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.add_symbol_input)
                    .hint_text("Add symbol…")
                    .desired_width(140.0),
            );
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("＋").clicked() || enter)
                && let Some(id) = SymbolId::parse(&self.add_symbol_input)
            {
                let c = id.canonical();
                if !self.s.watchlist.contains(&c) {
                    self.s.watchlist.push(c);
                }
                self.add_symbol_input.clear();
            }
        });
        if let Some(sym) = remove {
            self.s.watchlist.retain(|s| *s != sym);
        }
        if let Some(id) = clicked {
            self.set_symbol(&id);
        }
    }

    fn show_pane(&mut self, ui: &mut egui::Ui, i: usize, theme: Theme, multi: bool) {
        // Error banner for this pane.
        if let Status::Error(err) = &self.panes[i].market.status {
            let err = err.clone();
            let sym = self.panes[i].market.symbol.clone();
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(229, 72, 77, 40))
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!("Couldn't load {sym}: {}", truncate(&err, 140)))
                                .color(DOWN),
                        );
                        if sym.provider == Provider::TwelveData
                            && err.to_lowercase().contains("key")
                        {
                            ui.label(RichText::new("Add your own Twelve Data key in ⚙.").weak());
                        }
                        if ui.button("Retry").clicked() {
                            self.panes[i].market.reload();
                        }
                    });
                });
            if self.panes[i].market.series.is_empty() {
                let r = ui.available_rect_before_wrap();
                ui.painter().rect_filled(r, 0.0, theme.bg);
                ui.painter().text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    "No data",
                    egui::FontId::proportional(15.0),
                    theme.text_dim,
                );
                if ui.allocate_rect(r, egui::Sense::click()).clicked() {
                    self.s.active = i;
                }
                return;
            }
        }

        let sym = self.s.panes[i].symbol.clone();
        let cfg = &mut self.s.panes[i];
        cfg.chart.drawings = self.s.drawings.remove(&sym).unwrap_or_default();
        let rt = &self.panes[i];
        let title = rt.market.symbol.ticker.clone();
        let levels = self.alerts.levels(&self.s.alerts, &sym);
        let mut add_alert_at = None;
        let synced = match self.sync_time {
            Some((src, t)) if self.s.sync_crosshair && src != i => Some(t),
            _ => None,
        };
        let out = Chart::new(&rt.market.series, rt.market.timeframe)
            .indicators(&rt.outputs)
            .title(&title)
            .theme(theme)
            .synced_time(synced)
            .alert_levels(&levels)
            .active(!multi || i == self.s.active)
            .show(ui, &mut cfg.chart);

        if let Some(r) = &out.response {
            if r.is_pointer_button_down_on() || r.secondary_clicked() {
                self.s.active = i;
            }
            let decimals = price_decimals(rt.market.series.last().map_or(1.0, |b| b.close));
            r.context_menu(|ui| {
                let Some(hit) = cfg.chart.context else {
                    ui.close();
                    return;
                };
                ui.label(
                    RichText::new(format!("{title} @ {}", fmt_fixed(hit.price, decimals))).weak(),
                );
                ui.separator();
                if ui.button("🔔  Add alert here").clicked() {
                    add_alert_at = Some(hit.price);
                    ui.close();
                }
                if ui.button("─  Horizontal line here").clicked() {
                    let a = primeview_chart::Anchor {
                        time: hit.time as f64,
                        price: hit.price,
                    };
                    cfg.chart.drawings.push(Drawing {
                        kind: DrawingKind::HorizontalLine,
                        a,
                        b: a,
                    });
                    ui.close();
                }
                if let Some(d) = hit.drawing
                    && ui.button("🗑  Remove drawing").clicked()
                {
                    if d < cfg.chart.drawings.len() {
                        cfg.chart.drawings.remove(d);
                    }
                    cfg.chart.selected = None;
                    ui.close();
                }
                if ui.button("⟲  Reset view").clicked() {
                    cfg.chart.reset_view();
                    ui.close();
                }
            });
        }
        if let Some(price) = add_alert_at {
            self.alerts.quick_add(&mut self.s.alerts, &sym, price);
            self.s.sidebar = SideTab::Alerts;
            self.s.show_watchlist = true;
        }
        if let Some(t) = out.hovered_time {
            self.sync_time = Some((i, t));
        } else if self.sync_time.is_some_and(|(src, _)| src == i) {
            self.sync_time = None;
        }
        if out.wants_history {
            self.panes[i].market.request_older();
        }
        let drawings = std::mem::take(&mut self.s.panes[i].chart.drawings);
        if !drawings.is_empty() {
            self.s.drawings.insert(sym, drawings);
        }
    }
}

impl eframe::App for PrimeViewApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let n = self.visible();
        for p in &mut self.panes[..n] {
            p.market.update();
        }
        if self.s.show_watchlist {
            self.watch.update(&self.s.watchlist, &self.s.twelvedata_key);
        }
        // Feed every price we see into the local alert engine.
        for i in 0..n {
            if let Some((price, _)) = self.panes[i].market.last_price() {
                let sym = self.panes[i].market.symbol.canonical();
                self.alerts.feed(&mut self.s.alerts, &sym, price);
            }
        }
        let quotes: Vec<(String, f64)> = self
            .watch
            .tickers
            .values()
            .map(|t| (t.symbol.clone(), t.last))
            .collect();
        for (sym, price) in quotes {
            self.alerts.feed(&mut self.s.alerts, &sym, price);
        }
        self.alerts.update(&mut self.s.alerts);

        egui::Panel::top("toolbar")
            .exact_size(40.0)
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::left("tools")
            .exact_size(44.0)
            .resizable(false)
            .show(ui, |ui| self.tools_panel(ui));
        if self.s.show_watchlist {
            egui::Panel::right("watchlist")
                .default_size(250.0)
                .min_size(200.0)
                .show(ui, |ui| self.side_panel(ui));
        }

        // After the panels: they may have switched symbol/timeframe this frame.
        let n = self.visible();
        for i in 0..n {
            self.refresh_indicators(i);
        }
        let theme = Theme::for_dark_mode(self.s.dark);
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let area = ui.available_rect_before_wrap();
            let gap = if n > 1 { 3.0 } else { 0.0 };
            ui.painter().rect_filled(
                area,
                0.0,
                if self.s.dark {
                    Color32::from_rgb(40, 44, 54)
                } else {
                    Color32::from_rgb(210, 214, 222)
                },
            );
            for (i, r) in self.s.layout.rects(area, gap).into_iter().enumerate() {
                ui.scope_builder(UiBuilder::new().id_salt(("pane", i)).max_rect(r), |ui| {
                    ui.set_clip_rect(r);
                    self.show_pane(ui, i, theme, n > 1);
                });
                if n > 1 && i == self.s.active {
                    ui.painter().rect_stroke(
                        r.shrink(0.5),
                        0.0,
                        egui::Stroke::new(1.5, ACCENT),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        });
        let a = self.s.active;
        let label = self.panes[a].market.symbol.ticker.clone();
        self.editor
            .ui(ui.ctx(), &mut self.s.panes[a].scripts, &label);
        self.alerts.toasts_ui(ui.ctx());
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.s);
    }
}

fn indicator_menu(ui: &mut egui::Ui, indicators: &mut Vec<Indicator>) {
    ui.set_min_width(300.0);
    ui.label(RichText::new("Add indicator").strong());
    for ind in Indicator::catalogue() {
        if ui.button(format!("＋ {}", ind.name())).clicked() {
            indicators.push(ind);
        }
    }
    if indicators.is_empty() {
        return;
    }
    ui.separator();
    ui.label(RichText::new("On chart").strong());
    let mut remove = None;
    for (i, ind) in indicators.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if ui.small_button("✕").on_hover_text("Remove").clicked() {
                remove = Some(i);
            }
            ui.label(ind.name());
            params_ui(ui, ind);
        });
    }
    if let Some(i) = remove {
        indicators.remove(i);
    }
}

fn params_ui(ui: &mut egui::Ui, ind: &mut Indicator) {
    let period = |ui: &mut egui::Ui, v: &mut usize| {
        ui.add(DragValue::new(v).range(1..=500).speed(0.2));
    };
    match ind {
        Indicator::Sma { period: p }
        | Indicator::Ema { period: p }
        | Indicator::Rsi { period: p }
        | Indicator::Atr { period: p } => period(ui, p),
        Indicator::Bollinger { period: p, mult } => {
            period(ui, p);
            ui.add(
                DragValue::new(mult)
                    .range(0.5..=5.0)
                    .speed(0.05)
                    .max_decimals(2),
            );
        }
        Indicator::Macd { fast, slow, signal } => {
            period(ui, fast);
            period(ui, slow);
            period(ui, signal);
        }
        Indicator::Stochastic { k, d, smooth } => {
            period(ui, k);
            period(ui, d);
            period(ui, smooth);
        }
        Indicator::Vwap => {}
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}
