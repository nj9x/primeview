//! Price alerts: evaluated locally while the app runs, and optionally on a
//! PrimeView alert server (`server/`) so they keep firing 24/7.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, RichText};
use primeview_alerts::{Alert, Condition, Engine, Fired, Frequency};
use serde::{Deserialize, Serialize};
use web_time::{Duration, Instant};

use crate::market::now_ms;

const AMBER: Color32 = Color32::from_rgb(255, 170, 0);
const TOAST_FOR: Duration = Duration::from_secs(10);
const SERVER_POLL: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CondKind {
    Crossing,
    CrossingUp,
    CrossingDown,
    Above,
    Below,
}

impl CondKind {
    const ALL: [CondKind; 5] = [
        CondKind::Crossing,
        CondKind::CrossingUp,
        CondKind::CrossingDown,
        CondKind::Above,
        CondKind::Below,
    ];

    fn label(self) -> &'static str {
        match self {
            CondKind::Crossing => "Crossing",
            CondKind::CrossingUp => "Crossing up",
            CondKind::CrossingDown => "Crossing down",
            CondKind::Above => "Greater than",
            CondKind::Below => "Less than",
        }
    }

    fn make(self, level: f64) -> Condition {
        match self {
            CondKind::Crossing => Condition::Crossing(level),
            CondKind::CrossingUp => Condition::CrossingUp(level),
            CondKind::CrossingDown => Condition::CrossingDown(level),
            CondKind::Above => Condition::Above(level),
            CondKind::Below => Condition::Below(level),
        }
    }
}

/// Persistent alert settings and the local engine.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct AlertSettings {
    pub engine: Engine,
    /// Base URL of a PrimeView alert server, e.g. `http://127.0.0.1:8787`. Empty = none.
    pub server_url: String,
    pub server_token: String,
    /// Create new alerts on the server instead of this device.
    pub prefer_server: bool,
    pub history: Vec<Fired>,
}

impl Default for AlertSettings {
    fn default() -> Self {
        Self {
            engine: Engine::new(),
            server_url: String::new(),
            server_token: String::new(),
            prefer_server: false,
            history: Vec::new(),
        }
    }
}

/// Body of `POST /api/alerts`.
#[derive(Serialize)]
struct NewAlert<'a> {
    symbol: &'a str,
    condition: &'a Condition,
    frequency: Frequency,
    message: &'a str,
}

enum ServerMsg {
    List(Vec<Alert>),
    Error(String),
    Changed,
}

/// Non-persistent alert UI/runtime state.
pub struct Alerts {
    toasts: Vec<(Fired, Instant)>,
    last_fed: HashMap<String, f64>,
    form_symbol: String,
    form_kind: CondKind,
    form_level: f64,
    form_every_time: bool,
    form_message: String,
    pub server_alerts: Vec<Alert>,
    server_error: Option<String>,
    server_inbox: Arc<Mutex<Vec<ServerMsg>>>,
    last_server_poll: Option<Instant>,
    ctx: egui::Context,
}

impl Alerts {
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            toasts: Vec::new(),
            last_fed: HashMap::new(),
            form_symbol: String::new(),
            form_kind: CondKind::Crossing,
            form_level: 0.0,
            form_every_time: false,
            form_message: String::new(),
            server_alerts: Vec::new(),
            server_error: None,
            server_inbox: Arc::default(),
            last_server_poll: None,
            ctx,
        }
    }

    /// Feed a price for a canonical symbol into the local engine.
    pub fn feed(&mut self, s: &mut AlertSettings, symbol: &str, price: f64) {
        if !price.is_finite() || self.last_fed.get(symbol) == Some(&price) {
            return;
        }
        self.last_fed.insert(symbol.to_string(), price);
        for f in s.engine.on_price(symbol, price, now_ms()) {
            log::info!("alert fired: {}", f.message);
            s.history.insert(0, f.clone());
            s.history.truncate(100);
            self.toasts.push((f, Instant::now()));
            self.ctx.request_repaint();
        }
    }

    /// Price levels of active alerts (local + server) for a symbol.
    pub fn levels(&self, s: &AlertSettings, symbol: &str) -> Vec<f64> {
        let symbol = primeview_alerts::normalize_symbol(symbol);
        s.engine
            .alerts()
            .iter()
            .chain(&self.server_alerts)
            .filter(|a| a.active && a.symbol == symbol)
            .map(|a| a.condition.level())
            .collect()
    }

    /// Create a "crossing" alert immediately (context-menu shortcut).
    pub fn quick_add(&mut self, s: &mut AlertSettings, symbol: &str, price: f64) {
        // Round to the instrument's precision so the level reads cleanly.
        let d = primeview_chart::scale::price_decimals(price) as i32;
        let f = 10f64.powi(d);
        let cond = Condition::Crossing((price * f).round() / f);
        self.create(s, symbol, cond, Frequency::Once, String::new());
    }

    fn create(
        &mut self,
        s: &mut AlertSettings,
        symbol: &str,
        cond: Condition,
        freq: Frequency,
        message: String,
    ) {
        if s.prefer_server && !s.server_url.trim().is_empty() {
            let body = NewAlert {
                symbol,
                condition: &cond,
                frequency: freq,
                message: &message,
            };
            let bytes = serde_json::to_vec(&body).unwrap_or_default();
            let mut req = ehttp::Request::post(self.url(s, "/api/alerts"), bytes);
            req.headers.insert("Content-Type", "application/json");
            self.send(s, req);
        } else {
            let mut alert = Alert::new(symbol, cond, freq, now_ms());
            alert.message = message;
            s.engine.add(alert);
            // Re-evaluate from the next tick (crossings need a fresh previous price).
            self.last_fed.remove(symbol);
        }
    }

    fn url(&self, s: &AlertSettings, path: &str) -> String {
        format!("{}{path}", s.server_url.trim().trim_end_matches('/'))
    }

    fn send(&self, s: &AlertSettings, mut req: ehttp::Request) {
        if !s.server_token.is_empty() {
            req.headers
                .insert("Authorization", format!("Bearer {}", s.server_token.trim()));
        }
        let inbox = self.server_inbox.clone();
        let ctx = self.ctx.clone();
        let is_get = matches!(req.method, ehttp::Method::GET);
        ehttp::fetch(req, move |res| {
            let msg = match res {
                Err(e) => ServerMsg::Error(e),
                Ok(r) if !r.ok => {
                    ServerMsg::Error(format!("HTTP {} {}", r.status, r.text().unwrap_or("")))
                }
                Ok(r) if is_get => match serde_json::from_slice::<Vec<Alert>>(&r.bytes) {
                    Ok(list) => ServerMsg::List(list),
                    Err(e) => ServerMsg::Error(format!("bad response: {e}")),
                },
                Ok(_) => ServerMsg::Changed,
            };
            inbox.lock().unwrap().push(msg);
            ctx.request_repaint();
        });
    }

    fn refresh_server(&mut self, s: &AlertSettings) {
        self.last_server_poll = Some(Instant::now());
        self.send(s, ehttp::Request::get(self.url(s, "/api/alerts")));
    }

    /// Call once per frame.
    pub fn update(&mut self, s: &mut AlertSettings) {
        if s.server_url.trim().is_empty() {
            self.server_alerts.clear();
            self.server_error = None;
        } else {
            if self
                .last_server_poll
                .is_none_or(|t| t.elapsed() >= SERVER_POLL)
            {
                self.refresh_server(s);
            }
            self.ctx.request_repaint_after(SERVER_POLL);
        }
        let msgs: Vec<ServerMsg> = std::mem::take(&mut *self.server_inbox.lock().unwrap());
        for m in msgs {
            match m {
                ServerMsg::List(list) => {
                    // Toast alerts that fired on the server since the last poll.
                    for a in &list {
                        let before = self.server_alerts.iter().find(|b| b.id == a.id);
                        if let (Some(b), Some(t)) = (before, a.last_fired_ms)
                            && b.last_fired_ms != Some(t)
                        {
                            let f = Fired {
                                alert_id: a.id,
                                symbol: a.symbol.clone(),
                                price: a.condition.level(),
                                time_ms: t,
                                message: format!("{} (server)", a.effective_message()),
                            };
                            s.history.insert(0, f.clone());
                            self.toasts.push((f, Instant::now()));
                        }
                    }
                    self.server_alerts = list;
                    self.server_error = None;
                }
                ServerMsg::Error(e) => self.server_error = Some(e),
                ServerMsg::Changed => self.refresh_server(s),
            }
        }
        self.toasts.retain(|(_, t)| t.elapsed() < TOAST_FOR);
        if !self.toasts.is_empty() {
            self.ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    pub fn toasts_ui(&mut self, ctx: &egui::Context) {
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::unique("primeview_alert_toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let mut dismiss = None;
                for (i, (f, _)) in self.toasts.iter().enumerate() {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_width(300.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("🔔").size(18.0).color(AMBER));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(pretty(&f.message)).strong());
                                let d = primeview_chart::scale::price_decimals(f.price);
                                ui.label(
                                    RichText::new(format!(
                                        "{} · {:.*}",
                                        ticker_of(&f.symbol),
                                        d,
                                        f.price
                                    ))
                                    .small()
                                    .weak(),
                                );
                            });
                            if ui.small_button("✕").clicked() {
                                dismiss = Some(i);
                            }
                        });
                    });
                    ui.add_space(6.0);
                }
                if let Some(i) = dismiss {
                    self.toasts.remove(i);
                }
            });
    }

    pub fn panel_ui(
        &mut self,
        ui: &mut egui::Ui,
        s: &mut AlertSettings,
        active_symbol: &str,
        last_price: Option<f64>,
    ) {
        if self.form_symbol.is_empty() {
            self.form_symbol = active_symbol.to_string();
        }
        if self.form_level == 0.0
            && let Some(p) = last_price
        {
            self.form_level = p;
        }

        ui.label(RichText::new("New alert").strong());
        egui::Grid::new("alert_form")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Symbol");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.form_symbol).desired_width(130.0));
                    if ui
                        .small_button("⟲")
                        .on_hover_text("Use active chart")
                        .clicked()
                    {
                        self.form_symbol = active_symbol.to_string();
                        if let Some(p) = last_price {
                            self.form_level = p;
                        }
                    }
                });
                ui.end_row();
                ui.label("Condition");
                egui::ComboBox::from_id_salt("alert_kind")
                    .selected_text(self.form_kind.label())
                    .show_ui(ui, |ui| {
                        for k in CondKind::ALL {
                            ui.selectable_value(&mut self.form_kind, k, k.label());
                        }
                    });
                ui.end_row();
                ui.label("Price");
                let speed = self.form_level.abs() * 0.0005 + 1e-6;
                ui.add(
                    egui::DragValue::new(&mut self.form_level)
                        .speed(speed)
                        .max_decimals(8),
                );
                ui.end_row();
                ui.label("Trigger");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.form_every_time, false, "Once");
                    ui.radio_value(&mut self.form_every_time, true, "Every time");
                });
                ui.end_row();
                ui.label("Message");
                ui.add(
                    egui::TextEdit::singleline(&mut self.form_message)
                        .hint_text("optional")
                        .desired_width(150.0),
                );
                ui.end_row();
            });
        let server_ok = !s.server_url.trim().is_empty();
        if server_ok {
            ui.checkbox(
                &mut s.prefer_server,
                "Create on server (fires when the app is closed)",
            );
        }
        if ui.button("🔔 Create alert").clicked() {
            let symbol = primeview_data::SymbolId::parse(&self.form_symbol)
                .map(|id| id.canonical())
                .unwrap_or_else(|| self.form_symbol.trim().to_uppercase());
            let freq = if self.form_every_time {
                Frequency::EveryTime {
                    cooldown_ms: 60_000,
                }
            } else {
                Frequency::Once
            };
            let msg = std::mem::take(&mut self.form_message);
            self.create(s, &symbol, self.form_kind.make(self.form_level), freq, msg);
        }

        ui.separator();
        ui.label(RichText::new("On this device").strong());
        ui.label(
            RichText::new("Checked while PrimeView is open.")
                .small()
                .weak(),
        );
        let mut remove = None;
        let mut toggle = None;
        for a in s.engine.alerts() {
            alert_row(ui, a, &mut remove, &mut toggle);
        }
        if s.engine.alerts().is_empty() {
            ui.label(RichText::new("No alerts yet. Right-click a chart → Add alert.").weak());
        }
        if let Some(id) = remove {
            s.engine.remove(id);
        }
        if let Some((id, on)) = toggle {
            s.engine.set_active(id, on);
        }

        ui.separator();
        ui.collapsing("Alert server", |ui| {
            ui.label(
                RichText::new("Run `primeview-server` to keep alerts firing 24/7 (webhooks).")
                    .small()
                    .weak(),
            );
            ui.horizontal(|ui| {
                ui.label("URL");
                ui.add(
                    egui::TextEdit::singleline(&mut s.server_url)
                        .hint_text("http://127.0.0.1:8787")
                        .desired_width(170.0),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Token");
                ui.add(
                    egui::TextEdit::singleline(&mut s.server_token)
                        .password(true)
                        .desired_width(160.0),
                );
            });
            if let Some(e) = &self.server_error {
                ui.label(
                    RichText::new(format!("⚠ {e}"))
                        .small()
                        .color(Color32::from_rgb(229, 72, 77)),
                );
            }
        });
        if server_ok {
            ui.label(RichText::new("On server").strong());
            let mut remove = None;
            let mut toggle = None;
            for a in &self.server_alerts {
                alert_row(ui, a, &mut remove, &mut toggle);
            }
            if let Some(id) = remove {
                let mut req = ehttp::Request::get(self.url(s, &format!("/api/alerts/{id}")));
                req.method = ehttp::Method::DELETE;
                self.send(s, req);
            }
            if let Some((id, on)) = toggle {
                let body =
                    serde_json::to_vec(&serde_json::json!({ "active": on })).unwrap_or_default();
                let mut req = ehttp::Request::post(self.url(s, &format!("/api/alerts/{id}")), body);
                req.method = ehttp::Method::PATCH;
                req.headers.insert("Content-Type", "application/json");
                self.send(s, req);
            }
        }

        if !s.history.is_empty() {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Recently fired").strong());
                if ui.small_button("Clear").clicked() {
                    s.history.clear();
                }
            });
            for f in s.history.iter().take(20) {
                let t = primeview_core::time::format_full(f.time_ms, true);
                ui.label(RichText::new(format!("{t} · {}", pretty(&f.message))).small());
            }
        }
    }
}

fn alert_row(
    ui: &mut egui::Ui,
    a: &Alert,
    remove: &mut Option<u64>,
    toggle: &mut Option<(u64, bool)>,
) {
    ui.horizontal(|ui| {
        let mut on = a.active;
        if ui.checkbox(&mut on, "").on_hover_text("Active").changed() {
            *toggle = Some((a.id, on));
        }
        let ticker = ticker_of(&a.symbol);
        let text = RichText::new(format!("{ticker} {}", a.condition.describe())).monospace();
        ui.label(if a.active { text } else { text.weak() });
        if ui.small_button("✕").on_hover_text("Delete").clicked() {
            *remove = Some(a.id);
        }
    });
}

fn ticker_of(symbol: &str) -> &str {
    symbol.split_once(':').map_or(symbol, |(_, t)| t)
}

/// Drop the provider prefix from a leading symbol: "BINANCE:BTCUSDT crossing 1" → "BTCUSDT crossing 1".
fn pretty(message: &str) -> String {
    match message.split_once(' ') {
        Some((head, rest)) if head.contains(':') => format!("{} {rest}", ticker_of(head)),
        _ => message.to_string(),
    }
}
