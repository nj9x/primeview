//! PrimeScript integration: per-pane script instances, a compile cache, and
//! the script editor window.

use std::collections::{BTreeMap, HashMap};

use eframe::egui::{self, Color32, RichText};
use primeview_core::{Bar, IndicatorOutput};
use primeview_script::{Diagnostic, EXAMPLES, Script, compile};
use serde::{Deserialize, Serialize};

const ERR: Color32 = Color32::from_rgb(229, 72, 77);
const OK: Color32 = Color32::from_rgb(46, 189, 133);

const NEW_SCRIPT: &str = r#"indicator("My indicator", overlay = true)
let len = input("Length", 20, 1, 500)
plot(ema(close, len), "EMA", color = orange)
"#;

/// A script attached to a chart pane, with the user's input overrides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ScriptInstance {
    pub source: String,
    pub inputs: BTreeMap<String, f64>,
}

/// Compiled scripts keyed by source text.
#[derive(Default)]
pub struct ScriptCache {
    compiled: HashMap<String, Result<Script, Vec<Diagnostic>>>,
}

impl ScriptCache {
    pub fn get(&mut self, source: &str) -> &Result<Script, Vec<Diagnostic>> {
        if self.compiled.len() > 64 {
            self.compiled.clear();
        }
        self.compiled
            .entry(source.to_string())
            .or_insert_with(|| compile(source))
    }

    /// Run one instance; `Err` holds a short message for the UI.
    pub fn run(&mut self, inst: &ScriptInstance, bars: &[Bar]) -> Result<IndicatorOutput, String> {
        match self.get(&inst.source) {
            Err(diags) => Err(diags
                .first()
                .map_or("compile error".into(), |d| d.to_string())),
            Ok(script) => {
                let inputs: HashMap<String, f64> = inst.inputs.clone().into_iter().collect();
                script.run(bars, &inputs).map_err(|d| d.to_string())
            }
        }
    }

    pub fn title(&mut self, source: &str) -> String {
        match self.get(source) {
            Ok(s) => s.title().to_string(),
            Err(_) => "Script (error)".into(),
        }
    }
}

/// The editor window state.
#[derive(Default)]
pub struct Editor {
    pub open: bool,
    source: String,
    /// Index of the script being edited in the active pane; `None` = new.
    target: Option<usize>,
    result: Option<Result<Script, Vec<Diagnostic>>>,
    compiled_for: String,
}

impl Editor {
    pub fn open_new(&mut self) {
        self.source = NEW_SCRIPT.to_string();
        self.target = None;
        self.open = true;
    }

    pub fn open_edit(&mut self, index: usize, source: &str) {
        self.source = source.to_string();
        self.target = Some(index);
        self.open = true;
    }

    /// Draw the window. Applies to `scripts` (the active pane's) on "Add/Update".
    pub fn ui(&mut self, ctx: &egui::Context, scripts: &mut Vec<ScriptInstance>, pane_label: &str) {
        if !self.open {
            return;
        }
        if self.source != self.compiled_for || self.result.is_none() {
            self.result = Some(compile(&self.source));
            self.compiled_for = self.source.clone();
        }
        if self.target.is_some_and(|i| i >= scripts.len()) {
            self.target = None;
        }

        let mut open = self.open;
        egui::Window::new("PrimeScript editor")
            .open(&mut open)
            .default_size([620.0, 520.0])
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("script_examples")
                        .selected_text("Examples")
                        .show_ui(ui, |ui| {
                            for (name, src) in EXAMPLES {
                                if ui.selectable_label(false, *name).clicked() {
                                    self.source = src.to_string();
                                }
                            }
                        });
                    if ui.button("New").clicked() {
                        self.source = NEW_SCRIPT.to_string();
                        self.target = None;
                    }
                    ui.separator();
                    let what = match self.target {
                        Some(i) => format!("Editing script #{} on {pane_label}", i + 1),
                        None => format!("New script for {pane_label}"),
                    };
                    ui.label(RichText::new(what).weak());
                });
                ui.separator();

                let diag_h = 110.0;
                egui::ScrollArea::vertical()
                    .id_salt("script_src")
                    .max_height((ui.available_height() - diag_h).max(120.0))
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.source)
                                .code_editor()
                                .desired_rows(18)
                                .desired_width(f32::INFINITY),
                        );
                    });
                ui.separator();

                let ok = matches!(self.result, Some(Ok(_)));
                match &self.result {
                    Some(Ok(script)) => {
                        let place = if script.overlay() {
                            "on the price chart"
                        } else {
                            "in its own pane"
                        };
                        ui.label(
                            RichText::new(format!(
                                "✔ “{}” compiles · draws {place} · {} input(s)",
                                script.title(),
                                script.inputs().len()
                            ))
                            .color(OK),
                        );
                    }
                    Some(Err(diags)) => {
                        egui::ScrollArea::vertical()
                            .id_salt("script_diags")
                            .max_height(70.0)
                            .show(ui, |ui| {
                                for d in diags.iter().take(20) {
                                    ui.label(
                                        RichText::new(format!("✖ {d}")).color(ERR).monospace(),
                                    );
                                }
                            });
                    }
                    None => {}
                }
                ui.horizontal(|ui| {
                    let label = if self.target.is_some() {
                        "Update chart"
                    } else {
                        "Add to chart"
                    };
                    if ui.add_enabled(ok, egui::Button::new(label)).clicked() {
                        match self.target {
                            Some(i) => {
                                scripts[i].source = self.source.clone();
                                if let Some(Ok(s)) = &self.result {
                                    let names: Vec<&str> =
                                        s.inputs().iter().map(|d| d.name.as_str()).collect();
                                    scripts[i].inputs.retain(|k, _| names.contains(&k.as_str()));
                                }
                            }
                            None => {
                                scripts.push(ScriptInstance {
                                    source: self.source.clone(),
                                    inputs: BTreeMap::new(),
                                });
                                self.target = Some(scripts.len() - 1);
                            }
                        }
                    }
                    ui.collapsing("Quick reference", |ui| {
                        ui.label(RichText::new(QUICK_REF).monospace().small());
                    });
                });
            });
        self.open = open;
    }
}

const QUICK_REF: &str = "\
indicator(\"Title\", overlay = true|false, range = 0..100)
let x = expr        // immutable binding
input(\"Name\", default, min, max)
series: open high low close volume hl2 hlc3 ohlc4 time bar_index
ops: + - * / %  < <= > >= == !=  and or not  c ? a : b  x[n]
ta: sma ema rma wma rsi stdev highest lowest sum change roc
    atr tr crossover crossunder cum nz stoch supertrend
let (m, s, h) = macd(close, 12, 26, 9)   let (mid, up, lo) = bb(close, 20, 2)
plot(x, \"Name\", color = #2962ff | teal, style = line | histogram)
let p = plot(...) ; fill(p1, p2) ; hline(70)";

/// The "PrimeScript" section of the indicators menu for one pane.
pub fn menu_section(
    ui: &mut egui::Ui,
    scripts: &mut Vec<ScriptInstance>,
    errors: &[Option<String>],
    cache: &mut ScriptCache,
    editor: &mut Editor,
) {
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(RichText::new("PrimeScript").strong());
        if ui.button("✎ New script…").clicked() {
            editor.open_new();
            ui.close();
        }
    });
    let mut remove = None;
    for (i, inst) in scripts.iter_mut().enumerate() {
        let title = cache.title(&inst.source);
        ui.horizontal(|ui| {
            if ui.small_button("✕").on_hover_text("Remove").clicked() {
                remove = Some(i);
            }
            if ui.small_button("✎").on_hover_text("Edit").clicked() {
                editor.open_edit(i, &inst.source);
                ui.close();
            }
            ui.label(&title);
            if let Some(Some(e)) = errors.get(i) {
                ui.label(RichText::new("⚠").color(ERR)).on_hover_text(e);
            }
        });
        if let Ok(script) = cache.get(&inst.source) {
            let defs = script.inputs().to_vec();
            if !defs.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(24.0);
                    for d in defs {
                        let v = inst.inputs.entry(d.name.clone()).or_insert(d.default);
                        ui.label(RichText::new(&d.name).small());
                        let mut dv =
                            egui::DragValue::new(v).speed((d.default.abs() * 0.01).max(0.1));
                        if let (Some(lo), Some(hi)) = (d.min, d.max) {
                            dv = dv.range(lo..=hi);
                        }
                        ui.add(dv);
                    }
                });
            }
        }
    }
    if let Some(i) = remove {
        scripts.remove(i);
    }
}
