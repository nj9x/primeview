//! PrimeView chart engine: an immediate-mode financial chart widget for egui.
//!
//! Renders candles/bars/line/area/Heikin-Ashi, volume, overlay and separate
//! pane indicators, a crosshair, price/time axes and user drawings. Handles
//! pan, zoom, price-axis scaling and drawing-tool interaction.

// Index loops over visible bar ranges read more clearly than zipped iterators here.
#![allow(clippy::needless_range_loop)]

pub mod drawing;
pub mod scale;
pub mod theme;

use std::borrow::Cow;

use egui::{
    Align2, Color32, CornerRadius, FontId, Key, Mesh, Painter, Pos2, Rect, Response, Sense, Shape,
    Stroke, Ui, Vec2, pos2, vec2,
};
use primeview_core::{Bar, IndicatorOutput, Pane, PlotStyle, Series, Timeframe, time};
use serde::{Deserialize, Serialize};

pub use drawing::{Anchor, Drawing, DrawingKind, Tool};
use drawing::{FIB_LEVELS, Handle, hit_test};
use scale::{
    XMap, YMap, fmt_compact, fmt_fixed, nice_step, price_decimals, step_decimals, ticks,
    time_tick_unit,
};
pub use theme::Theme;
use theme::{rgba, with_alpha};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChartType {
    Candles,
    HollowCandles,
    Bars,
    Line,
    Area,
    HeikinAshi,
}

impl ChartType {
    pub const ALL: [ChartType; 6] = [
        ChartType::Candles,
        ChartType::HollowCandles,
        ChartType::Bars,
        ChartType::Line,
        ChartType::Area,
        ChartType::HeikinAshi,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ChartType::Candles => "Candles",
            ChartType::HollowCandles => "Hollow candles",
            ChartType::Bars => "Bars",
            ChartType::Line => "Line",
            ChartType::Area => "Area",
            ChartType::HeikinAshi => "Heikin Ashi",
        }
    }
}

const MIN_BAR_W: f32 = 1.0;
const MAX_BAR_W: f32 = 80.0;
const AXIS_W: f32 = 78.0;
const TIME_H: f32 = 24.0;

/// Persistent per-chart view state. Owned by the app and passed in each frame.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ChartState {
    pub chart_type: ChartType,
    pub bar_width: f32,
    /// Empty bars to the right of the last bar. Negative = scrolled into history.
    pub right_offset: f64,
    pub auto_scale: bool,
    pub price_range: (f64, f64),
    pub show_volume: bool,
    pub show_grid: bool,
    pub tool: Tool,
    /// Keep the drawing tool active after placing a drawing.
    pub stay_in_draw_mode: bool,
    pub drawings: Vec<Drawing>,

    #[serde(skip)]
    pending: Option<Anchor>,
    #[serde(skip)]
    drag: Option<Drag>,
    #[serde(skip)]
    pub selected: Option<usize>,
    /// Set on right-click; the app reads it while its context menu is open.
    #[serde(skip)]
    pub context: Option<ContextHit>,
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    Pan,
    PriceAxis,
    TimeAxis,
    Drawing { index: usize, handle: Handle },
}

impl Default for ChartState {
    fn default() -> Self {
        Self {
            chart_type: ChartType::Candles,
            bar_width: 8.0,
            right_offset: 8.0,
            auto_scale: true,
            price_range: (0.0, 1.0),
            show_volume: true,
            show_grid: true,
            tool: Tool::Cursor,
            stay_in_draw_mode: false,
            drawings: Vec::new(),
            pending: None,
            drag: None,
            selected: None,
            context: None,
        }
    }
}

impl ChartState {
    /// Jump back to the latest bar with auto-scaling on.
    pub fn reset_view(&mut self) {
        self.right_offset = 8.0;
        self.auto_scale = true;
    }

    pub fn cancel_drawing(&mut self) {
        self.pending = None;
        self.tool = Tool::Cursor;
    }
}

/// Where the user right-clicked, for the app's context menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContextHit {
    pub price: f64,
    pub time: i64,
    /// Index into `ChartState::drawings` if a drawing was under the pointer.
    pub drawing: Option<usize>,
}

/// What the chart reports back to the app after each frame.
#[derive(Clone, Debug, Default)]
pub struct ChartOutput {
    /// The bar index under the crosshair, if any.
    pub hovered: Option<usize>,
    /// Time (ms) under the crosshair, for syncing other charts.
    pub hovered_time: Option<i64>,
    /// The user scrolled close to the oldest loaded bar; load more history.
    pub wants_history: bool,
    /// The chart's interaction response (e.g. to attach a context menu).
    pub response: Option<Response>,
}

pub struct Chart<'a> {
    series: &'a Series,
    timeframe: Timeframe,
    indicators: &'a [IndicatorOutput],
    title: &'a str,
    theme: Theme,
    synced_time: Option<i64>,
    alert_levels: &'a [f64],
    active: bool,
}

/// Per-frame geometry shared by the render passes.
struct Frame<'a> {
    painter: Painter,
    theme: Theme,
    bars: &'a [Bar],
    xm: XMap,
    plot: Rect,
    i0: usize,
    i1: usize,
    decimals: usize,
}

impl<'a> Chart<'a> {
    pub fn new(series: &'a Series, timeframe: Timeframe) -> Self {
        Self {
            series,
            timeframe,
            indicators: &[],
            title: "",
            theme: Theme::dark(),
            synced_time: None,
            alert_levels: &[],
            active: true,
        }
    }

    /// Show a crosshair at this time when another chart is being hovered.
    pub fn synced_time(mut self, t: Option<i64>) -> Self {
        self.synced_time = t;
        self
    }

    /// Price levels of alerts on this symbol, drawn as dashed lines.
    pub fn alert_levels(mut self, levels: &'a [f64]) -> Self {
        self.alert_levels = levels;
        self
    }

    /// Highlight this chart as the active one in a multi-chart layout.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn indicators(mut self, indicators: &'a [IndicatorOutput]) -> Self {
        self.indicators = indicators;
        self
    }

    pub fn title(mut self, title: &'a str) -> Self {
        self.title = title;
        self
    }

    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    pub fn show(self, ui: &mut Ui, st: &mut ChartState) -> ChartOutput {
        let theme = self.theme;
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme.bg);

        let n = self.series.len();
        if n == 0 {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Loading…",
                FontId::proportional(16.0),
                theme.text_dim,
            );
            return ChartOutput {
                response: Some(resp),
                ..Default::default()
            };
        }

        let display: Cow<[Bar]> = if st.chart_type == ChartType::HeikinAshi {
            Cow::Owned(self.series.heikin_ashi())
        } else {
            Cow::Borrowed(&self.series.bars)
        };
        let bars: &[Bar] = &display;

        // ---------------------------------------------------------------- layout
        let plot = Rect::from_min_max(
            rect.min,
            pos2(rect.right() - AXIS_W, rect.bottom() - TIME_H),
        );
        // Ignore outputs computed for a different series (e.g. mid symbol switch).
        let fresh = |o: &&IndicatorOutput| o.plots.iter().all(|p| p.values.len() == n);
        let separate: Vec<&IndicatorOutput> = self
            .indicators
            .iter()
            .filter(fresh)
            .filter(|o| matches!(o.pane, Pane::Separate { .. }))
            .collect();
        let overlays: Vec<&IndicatorOutput> = self
            .indicators
            .iter()
            .filter(fresh)
            .filter(|o| o.pane == Pane::Overlay)
            .collect();
        let pane_h = if separate.is_empty() {
            0.0
        } else {
            (plot.height() * 0.2)
                .clamp(70.0, 160.0)
                .min(plot.height() * 0.55 / separate.len() as f32)
        };
        let main = Rect::from_min_max(
            plot.min,
            pos2(plot.right(), plot.bottom() - pane_h * separate.len() as f32),
        );
        let pane_rects: Vec<Rect> = (0..separate.len())
            .map(|k| {
                let top = main.bottom() + pane_h * k as f32;
                Rect::from_min_max(pos2(plot.left(), top), pos2(plot.right(), top + pane_h))
            })
            .collect();
        let price_axis = Rect::from_min_max(
            pos2(plot.right(), main.top()),
            pos2(rect.right(), main.bottom()),
        );
        let time_axis = Rect::from_min_max(
            pos2(plot.left(), plot.bottom()),
            pos2(plot.right(), rect.bottom()),
        );

        // ------------------------------------------------- view input (pan/zoom)
        let hover = resp.hover_pos();
        self.handle_view_input(ui, &resp, st, plot, main, price_axis, time_axis, hover);

        let last = (n - 1) as f64;
        let visible_bars = (plot.width() / st.bar_width) as f64;
        st.right_offset = st.right_offset.clamp(-last + 2.0, visible_bars - 3.0);
        let xm = XMap {
            right: plot.right(),
            bar_w: st.bar_width,
            last,
            offset: st.right_offset,
        };
        let i0 = xm.idx(plot.left()).floor().max(0.0) as usize;
        let i1 = (xm.idx(plot.right()).ceil().max(0.0) as usize).min(n - 1);
        let i0 = i0.min(i1);

        // --------------------------------------------------------- main y-scale
        let (lo, hi) = if st.auto_scale {
            let mut lo = f64::MAX;
            let mut hi = f64::MIN;
            for b in &bars[i0..=i1] {
                lo = lo.min(b.low);
                hi = hi.max(b.high);
            }
            for o in &overlays {
                for p in &o.plots {
                    for v in p.values[i0..=i1].iter().filter(|v| v.is_finite()) {
                        lo = lo.min(*v);
                        hi = hi.max(*v);
                    }
                }
            }
            if hi <= lo {
                hi = lo + lo.abs().max(1.0) * 0.01;
            }
            let span = hi - lo;
            let bottom_pad = if st.show_volume { 0.18 } else { 0.08 };
            st.price_range = (lo - span * bottom_pad, hi + span * 0.08);
            st.price_range
        } else {
            st.price_range
        };
        let ym = YMap {
            top: main.top() + 4.0,
            bottom: main.bottom() - 4.0,
            min: lo,
            max: hi,
        };
        let decimals = price_decimals(bars[n - 1].close);

        let fr = Frame {
            painter: painter.clone(),
            theme,
            bars,
            xm,
            plot,
            i0,
            i1,
            decimals,
        };

        // ------------------------------------------- drawings & tool interaction
        self.handle_drawing_input(ui, &resp, st, &fr, &ym, main, hover);

        // -------------------------------------------------------------- render
        let tick_unit = time_tick_unit(self.timeframe.millis(), st.bar_width, 90.0);
        let time_ticks = self.time_ticks(&fr, tick_unit);
        if st.show_grid {
            for &(x, _) in &time_ticks {
                painter.vline(x, plot.y_range(), Stroke::new(1.0, theme.grid));
            }
        }

        let main_p = painter.with_clip_rect(main);
        let price_step = nice_step(ym.span(), (main.height() / 56.0).max(2.0) as f64);
        let tick_dec = step_decimals(price_step).max(if decimals > 2 { 2 } else { 0 });
        if st.show_grid {
            for v in ticks(ym.min, ym.max, price_step) {
                main_p.hline(
                    main.x_range(),
                    ym.y(v).round() + 0.5,
                    Stroke::new(1.0, theme.grid),
                );
            }
        }

        if st.show_volume {
            self.draw_volume(&fr, &main_p, main);
        }
        self.draw_price(&fr, &main_p, &ym, st.chart_type);
        for o in &overlays {
            draw_plots(&fr, &main_p, &ym, o);
        }
        self.draw_drawings(&fr, &main_p, &ym, st, hover);
        self.draw_alert_levels(&fr, &main_p, &ym, main);
        self.draw_last_price(&fr, &main_p, &ym, main);

        // separate panes
        let mut pane_maps = Vec::with_capacity(separate.len());
        for (o, r) in separate.iter().zip(&pane_rects) {
            painter.hline(
                plot.left()..=rect.right(),
                r.top(),
                Stroke::new(1.0, theme.separator),
            );
            let ym = pane_scale(o, *r, i0, i1);
            let pp = painter.with_clip_rect(*r);
            if let Pane::Separate { levels, .. } = &o.pane {
                for l in levels {
                    let pts = [pos2(r.left(), ym.y(*l)), pos2(r.right(), ym.y(*l))];
                    pp.extend(Shape::dashed_line(
                        &pts,
                        Stroke::new(1.0, theme.crosshair),
                        4.0,
                        4.0,
                    ));
                }
            }
            draw_plots(&fr, &pp, &ym, o);
            pane_maps.push(ym);
        }

        // ----------------------------------------------------------- axes
        painter.vline(
            plot.right(),
            rect.y_range(),
            Stroke::new(1.0, theme.separator),
        );
        painter.hline(
            rect.x_range(),
            plot.bottom(),
            Stroke::new(1.0, theme.separator),
        );
        let axis_font = FontId::monospace(11.0);
        let axis_p = painter.with_clip_rect(price_axis);
        for v in ticks(ym.min, ym.max, price_step) {
            axis_p.text(
                pos2(price_axis.left() + 8.0, ym.y(v)),
                Align2::LEFT_CENTER,
                fmt_fixed(v, tick_dec),
                axis_font.clone(),
                theme.text_dim,
            );
        }
        for (ym, r) in pane_maps.iter().zip(&pane_rects) {
            let step = nice_step(ym.span(), (r.height() / 40.0).max(2.0) as f64);
            for v in ticks(ym.min, ym.max, step) {
                let y = ym.y(v);
                if y < r.top() + 8.0 || y > r.bottom() - 8.0 {
                    continue; // keep labels fully inside their pane
                }
                let label = if step >= 1.0 && v.abs() < 1e4 {
                    format!("{v:.0}")
                } else {
                    fmt_compact(v)
                };
                painter.text(
                    pos2(plot.right() + 8.0, y),
                    Align2::LEFT_CENTER,
                    label,
                    axis_font.clone(),
                    theme.text_dim,
                );
            }
        }
        let tp = painter.with_clip_rect(time_axis);
        for (x, label) in &time_ticks {
            tp.text(
                pos2(*x, time_axis.center().y),
                Align2::CENTER_CENTER,
                label,
                axis_font.clone(),
                theme.text_dim,
            );
        }
        if !st.auto_scale {
            painter.text(
                pos2(rect.right() - 4.0, rect.bottom() - 4.0),
                Align2::RIGHT_BOTTOM,
                "A",
                FontId::monospace(10.0),
                theme.accent,
            );
        }

        // ------------------------------------------------------- crosshair
        let mut hovered = None;
        let mut hovered_time = None;
        if hover.is_none_or(|p| !plot.contains(p))
            && let Some(t) = self.synced_time
        {
            let x = xm.x(self
                .series
                .index_of_time(t as f64, self.timeframe.millis())
                .round());
            if plot.x_range().contains(x) {
                painter.extend(Shape::dashed_line(
                    &[pos2(x, plot.top()), pos2(x, plot.bottom())],
                    Stroke::new(1.0, theme.crosshair),
                    4.0,
                    3.0,
                ));
            }
        }
        if let Some(p) = hover.filter(|p| plot.contains(*p)) {
            let idx = xm.idx(p.x).round();
            let x = xm.x(idx);
            let cross = Stroke::new(1.0, theme.crosshair);
            painter.extend(Shape::dashed_line(
                &[pos2(x, plot.top()), pos2(x, plot.bottom())],
                cross,
                4.0,
                3.0,
            ));
            painter.extend(Shape::dashed_line(
                &[pos2(plot.left(), p.y), pos2(plot.right(), p.y)],
                cross,
                4.0,
                3.0,
            ));

            let value_label = if main.contains(p) {
                Some(fmt_fixed(ym.v(p.y), decimals))
            } else {
                pane_rects
                    .iter()
                    .zip(&pane_maps)
                    .find(|(r, _)| r.contains(p))
                    .map(|(_, m)| fmt_compact(m.v(p.y)))
            };
            if let Some(text) = value_label {
                axis_label(
                    &painter,
                    pos2(plot.right(), p.y),
                    text,
                    theme.label_bg,
                    theme.label_text,
                    true,
                );
            }
            let t = self.series.time_of_index(idx, self.timeframe.millis()) as i64;
            hovered_time = Some(t);
            let tlabel = time::format_full(t, self.timeframe.is_intraday());
            let g = painter.layout_no_wrap(tlabel, FontId::monospace(11.0), theme.label_text);
            let r =
                Rect::from_center_size(pos2(x, time_axis.center().y), g.size() + vec2(12.0, 6.0));
            painter.rect_filled(r, 3.0, theme.label_bg);
            painter.galley(r.min + vec2(6.0, 3.0), g, theme.label_text);

            if idx >= 0.0 && (idx as usize) < n {
                hovered = Some(idx as usize);
            }
        }

        // ---------------------------------------------------------- legends
        let li = hovered.unwrap_or(n - 1);
        self.draw_legend(&fr, main, li, &overlays);
        for (o, r) in separate.iter().zip(&pane_rects) {
            legend_line(
                &painter,
                pos2(r.left() + 8.0, r.top() + 4.0),
                &o.label,
                o,
                li,
                theme,
                None,
            );
        }

        if st.tool != Tool::Cursor {
            let msg = if st.pending.is_some() {
                "Click to place the second point · Esc to cancel"
            } else {
                "Click on the chart to draw · Esc to cancel"
            };
            painter.text(
                pos2(main.center().x, main.bottom() - 10.0),
                Align2::CENTER_BOTTOM,
                msg,
                FontId::proportional(12.0),
                theme.text_dim,
            );
        }

        if !self.active {
            painter.rect_filled(rect, 0.0, with_alpha(theme.bg, 40));
        }

        ChartOutput {
            hovered,
            hovered_time,
            wants_history: i0 < 150 && xm.idx(plot.left()) < 150.0,
            response: Some(resp),
        }
    }

    // ---------------------------------------------------------------------
    // Input

    #[allow(clippy::too_many_arguments)]
    fn handle_view_input(
        &self,
        ui: &Ui,
        resp: &Response,
        st: &mut ChartState,
        plot: Rect,
        main: Rect,
        price_axis: Rect,
        time_axis: Rect,
        hover: Option<Pos2>,
    ) {
        let n = self.series.len() as f64;
        let zoom_at = |st: &mut ChartState, factor: f32, anchor_x: f32| {
            let old = XMap {
                right: plot.right(),
                bar_w: st.bar_width,
                last: n - 1.0,
                offset: st.right_offset,
            };
            let idx = old.idx(anchor_x);
            st.bar_width = (st.bar_width * factor).clamp(MIN_BAR_W, MAX_BAR_W);
            // keep the bar under the cursor fixed: solve x(idx) == anchor_x for offset
            st.right_offset =
                ((plot.right() - anchor_x) / st.bar_width) as f64 - (n - 1.0) + idx - 0.5;
        };

        if let Some(p) = hover {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            if plot.contains(p) {
                if zoom != 1.0 {
                    zoom_at(st, zoom, p.x);
                }
                if scroll.y != 0.0 {
                    zoom_at(st, (scroll.y * 0.003).exp(), p.x);
                }
                if scroll.x != 0.0 {
                    st.right_offset -= (scroll.x / st.bar_width) as f64;
                }
            } else if price_axis.contains(p) && scroll.y != 0.0 {
                scale_price(st, (-scroll.y * 0.003).exp());
            }
        }

        if resp.drag_started() {
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
            st.drag = if price_axis.contains(origin) {
                Some(Drag::PriceAxis)
            } else if time_axis.contains(origin) {
                Some(Drag::TimeAxis)
            } else if plot.contains(origin) {
                None // decided in handle_drawing_input (could be a drawing)
            } else {
                Some(Drag::Pan)
            };
        }
        if resp.dragged() {
            let d = resp.drag_delta();
            match st.drag {
                Some(Drag::PriceAxis) => scale_price(st, (d.y * 0.005).exp()),
                Some(Drag::TimeAxis) => zoom_at(st, (-d.x * 0.01).exp(), plot.right()),
                Some(Drag::Pan) => {
                    st.right_offset -= (d.x / st.bar_width) as f64;
                    if !st.auto_scale {
                        let per_px = (st.price_range.1 - st.price_range.0) / main.height() as f64;
                        st.price_range.0 += d.y as f64 * per_px;
                        st.price_range.1 += d.y as f64 * per_px;
                    }
                }
                _ => {}
            }
        }
        if resp.double_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            if price_axis.contains(p) {
                st.auto_scale = true;
            } else if time_axis.contains(p) {
                st.reset_view();
            }
        }
        if resp.hovered() || resp.has_focus() {
            let (left, right, esc, del, home) = ui.input(|i| {
                (
                    i.key_down(Key::ArrowLeft),
                    i.key_down(Key::ArrowRight),
                    i.key_pressed(Key::Escape),
                    i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
                    i.key_pressed(Key::Home),
                )
            });
            let step = (8.0 / st.bar_width.max(1.0)) as f64;
            if left {
                st.right_offset -= step;
            }
            if right {
                st.right_offset += step;
            }
            if left || right {
                ui.ctx().request_repaint();
            }
            if esc {
                st.cancel_drawing();
                st.selected = None;
            }
            if del
                && let Some(i) = st.selected.take()
                && i < st.drawings.len()
            {
                st.drawings.remove(i);
            }
            if home {
                st.reset_view();
            }
        }
        if resp.drag_stopped() {
            st.drag = None;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_drawing_input(
        &self,
        ui: &Ui,
        resp: &Response,
        st: &mut ChartState,
        fr: &Frame,
        ym: &YMap,
        main: Rect,
        hover: Option<Pos2>,
    ) {
        let tf = self.timeframe.millis();
        let to_anchor = |p: Pos2| Anchor {
            time: self.series.time_of_index(fr.xm.idx(p.x), tf),
            price: ym.v(p.y),
        };

        // Creating drawings
        if let Some(kind) = st.tool.kind() {
            if resp.clicked()
                && let Some(p) = resp.interact_pointer_pos().filter(|p| main.contains(*p))
            {
                let anchor = to_anchor(p);
                let finish = |st: &mut ChartState, a: Anchor, b: Anchor| {
                    st.drawings.push(Drawing { kind, a, b });
                    st.selected = Some(st.drawings.len() - 1);
                    st.pending = None;
                    if !st.stay_in_draw_mode {
                        st.tool = Tool::Cursor;
                    }
                };
                if kind.one_point() {
                    finish(st, anchor, anchor);
                } else if let Some(a) = st.pending {
                    finish(st, a, anchor);
                } else {
                    st.pending = Some(anchor);
                }
            }
            if st.pending.is_some() {
                ui.ctx().request_repaint();
            }
            return;
        }

        // Selecting / editing drawings in cursor mode
        let hit_at = |st: &ChartState, p: Pos2| -> Option<(usize, Handle)> {
            st.drawings.iter().enumerate().rev().find_map(|(i, d)| {
                let (pa, pb) = self.anchor_pos(fr, ym, d);
                hit_test(d.kind, pa, pb, p, main.left(), main.right()).map(|h| (i, h))
            })
        };

        if resp.drag_started() && st.drag.is_none() {
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
            st.drag = Some(match hit_at(st, origin).filter(|_| main.contains(origin)) {
                Some((index, handle)) => {
                    st.selected = Some(index);
                    Drag::Drawing { index, handle }
                }
                None => Drag::Pan,
            });
            // pan for this first frame too
            if matches!(st.drag, Some(Drag::Pan)) {
                st.right_offset -= (resp.drag_delta().x / st.bar_width) as f64;
            }
        }
        if resp.dragged()
            && let Some(Drag::Drawing { index, handle }) = st.drag
            && let Some(p) = resp.interact_pointer_pos()
            && index < st.drawings.len()
        {
            let d = &mut st.drawings[index];
            match handle {
                Handle::A => d.a = to_anchor(p),
                Handle::B => d.b = to_anchor(p),
                Handle::Body => {
                    let delta = resp.drag_delta();
                    let didx = (delta.x / st.bar_width) as f64;
                    let dprice = ym.v(p.y) - ym.v(p.y - delta.y);
                    for a in [&mut d.a, &mut d.b] {
                        let idx = self.series.index_of_time(a.time, tf) + didx;
                        a.time = self.series.time_of_index(idx, tf);
                        a.price += dprice;
                    }
                }
            }
        }
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            st.selected = hit_at(st, p).map(|(i, _)| i);
        }
        if resp.secondary_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let drawing = hit_at(st, p).map(|(i, _)| i);
            if drawing.is_some() {
                st.selected = drawing;
            }
            st.context = Some(ContextHit {
                price: ym.v(p.y),
                time: self.series.time_of_index(fr.xm.idx(p.x).round(), tf) as i64,
                drawing,
            });
        }
        if let Some(p) = hover
            && main.contains(p)
            && st.drag.is_none()
            && hit_at(st, p).is_some()
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }

    fn anchor_pos(&self, fr: &Frame, ym: &YMap, d: &Drawing) -> (Pos2, Pos2) {
        let tf = self.timeframe.millis();
        let p = |a: &Anchor| {
            pos2(
                fr.xm.x(self.series.index_of_time(a.time, tf)),
                ym.y(a.price),
            )
        };
        (p(&d.a), p(&d.b))
    }

    // ---------------------------------------------------------------------
    // Rendering

    fn time_ticks(&self, fr: &Frame, unit: scale::TickUnit) -> Vec<(f32, String)> {
        let mut out: Vec<(f32, String)> = Vec::new();
        let start = fr.i0.max(1);
        for i in start..=fr.i1 {
            let (t0, t1) = (fr.bars[i - 1].time, fr.bars[i].time);
            if unit.key(t0) == unit.key(t1) {
                continue;
            }
            let x = fr.xm.x(i as f64);
            if x < fr.plot.left() + 20.0 || x > fr.plot.right() - 20.0 {
                continue;
            }
            if out.last().is_some_and(|(px, _)| x - px < 60.0) {
                continue;
            }
            out.push((x.round() + 0.5, time::format_tick(t1, unit.approx_ms())));
        }
        out
    }

    fn draw_volume(&self, fr: &Frame, p: &Painter, main: Rect) {
        let max_vol = fr.bars[fr.i0..=fr.i1]
            .iter()
            .map(|b| b.volume)
            .fold(0.0, f64::max);
        if max_vol <= 0.0 {
            return;
        }
        let h = main.height() * 0.16;
        let body_w = (fr.xm.bar_w * 0.8).max(1.0);
        for i in fr.i0..=fr.i1 {
            let b = &self.series.bars[i];
            let x = fr.xm.x(i as f64);
            let top = main.bottom() - (b.volume / max_vol) as f32 * h;
            let c = with_alpha(fr.theme.candle(b.is_up()), 70);
            p.rect_filled(
                Rect::from_min_max(
                    pos2(x - body_w / 2.0, top),
                    pos2(x + body_w / 2.0, main.bottom()),
                ),
                0.0,
                c,
            );
        }
    }

    fn draw_price(&self, fr: &Frame, p: &Painter, ym: &YMap, kind: ChartType) {
        let bw = fr.xm.bar_w;
        let body_w = (bw * 0.72).max(1.0);
        let px = |v: f32| v.round() + 0.5; // crisp 1px lines
        match kind {
            ChartType::Line | ChartType::Area => {
                let pts: Vec<Pos2> = (fr.i0..=fr.i1)
                    .map(|i| pos2(fr.xm.x(i as f64), ym.y(fr.bars[i].close)))
                    .collect();
                if kind == ChartType::Area && pts.len() > 1 {
                    let mut mesh = Mesh::default();
                    let top_c = with_alpha(fr.theme.accent, 90);
                    let bot_c = with_alpha(fr.theme.accent, 0);
                    for (k, pt) in pts.iter().enumerate() {
                        mesh.colored_vertex(*pt, top_c);
                        mesh.colored_vertex(pos2(pt.x, ym.bottom), bot_c);
                        if k > 0 {
                            let j = (k * 2) as u32;
                            mesh.add_triangle(j - 2, j - 1, j);
                            mesh.add_triangle(j - 1, j, j + 1);
                        }
                    }
                    p.add(Shape::mesh(mesh));
                }
                p.add(Shape::line(pts, Stroke::new(1.8, fr.theme.accent)));
            }
            ChartType::Bars => {
                let tick = (bw * 0.35).max(1.0);
                for i in fr.i0..=fr.i1 {
                    let b = &fr.bars[i];
                    let x = px(fr.xm.x(i as f64));
                    let s =
                        Stroke::new(if bw > 6.0 { 1.5 } else { 1.0 }, fr.theme.candle(b.is_up()));
                    p.line_segment([pos2(x, ym.y(b.high)), pos2(x, ym.y(b.low))], s);
                    p.line_segment([pos2(x - tick, ym.y(b.open)), pos2(x, ym.y(b.open))], s);
                    p.line_segment([pos2(x, ym.y(b.close)), pos2(x + tick, ym.y(b.close))], s);
                }
            }
            ChartType::Candles | ChartType::HollowCandles | ChartType::HeikinAshi => {
                let hollow = kind == ChartType::HollowCandles;
                for i in fr.i0..=fr.i1 {
                    let b = &fr.bars[i];
                    let x = fr.xm.x(i as f64);
                    let color = if hollow {
                        let prev = if i > 0 { fr.bars[i - 1].close } else { b.open };
                        fr.theme.candle(b.close >= prev)
                    } else {
                        fr.theme.candle(b.is_up())
                    };
                    let (yo, yc) = (ym.y(b.open), ym.y(b.close));
                    let (top, bot) = (yo.min(yc), yo.max(yc).max(yo.min(yc) + 1.0));
                    if body_w <= 2.0 {
                        p.line_segment(
                            [pos2(px(x), ym.y(b.high)), pos2(px(x), ym.y(b.low))],
                            Stroke::new(1.0, color),
                        );
                        continue;
                    }
                    let xw = px(x);
                    p.line_segment(
                        [pos2(xw, ym.y(b.high)), pos2(xw, top)],
                        Stroke::new(1.0, color),
                    );
                    p.line_segment(
                        [pos2(xw, bot), pos2(xw, ym.y(b.low))],
                        Stroke::new(1.0, color),
                    );
                    let body = Rect::from_min_max(
                        pos2(x - body_w / 2.0, top),
                        pos2(x + body_w / 2.0, bot),
                    );
                    if hollow && b.is_up() {
                        p.rect_stroke(body, 0.0, Stroke::new(1.0, color), egui::StrokeKind::Inside);
                    } else {
                        p.rect_filled(body, CornerRadius::ZERO, color);
                    }
                }
            }
        }
    }

    fn draw_alert_levels(&self, fr: &Frame, p: &Painter, ym: &YMap, main: Rect) {
        let c = Color32::from_rgb(255, 170, 0);
        for &level in self.alert_levels {
            let y = ym.y(level).round() + 0.5;
            if y < main.top() || y > main.bottom() {
                continue;
            }
            p.extend(Shape::dashed_line(
                &[pos2(main.left(), y), pos2(main.right(), y)],
                Stroke::new(1.0, c),
                6.0,
                4.0,
            ));
            p.text(
                pos2(main.right() - 6.0, y - 2.0),
                Align2::RIGHT_BOTTOM,
                format!("🔔 {}", fmt_fixed(level, fr.decimals)),
                FontId::monospace(10.5),
                c,
            );
        }
    }

    fn draw_last_price(&self, fr: &Frame, p: &Painter, ym: &YMap, main: Rect) {
        let raw_last = self.series.bars[self.series.len() - 1];
        let color = fr.theme.candle(raw_last.is_up());
        let y = ym.y(raw_last.close).round() + 0.5;
        p.extend(Shape::dashed_line(
            &[pos2(main.left(), y), pos2(main.right(), y)],
            Stroke::new(1.0, with_alpha(color, 160)),
            2.0,
            3.0,
        ));
        if y >= main.top() && y <= main.bottom() {
            axis_label(
                &fr.painter,
                pos2(main.right(), y),
                fmt_fixed(raw_last.close, fr.decimals),
                color,
                Color32::WHITE,
                true,
            );
        }
    }

    fn draw_drawings(
        &self,
        fr: &Frame,
        p: &Painter,
        ym: &YMap,
        st: &ChartState,
        hover: Option<Pos2>,
    ) {
        let base = fr.theme.drawing;
        let mut items: Vec<(Drawing, bool)> = st
            .drawings
            .iter()
            .enumerate()
            .map(|(i, d)| (d.clone(), st.selected == Some(i)))
            .collect();
        // live preview of the drawing being placed
        if let (Some(a), Some(kind), Some(h)) = (st.pending, st.tool.kind(), hover) {
            let b = Anchor {
                time: self
                    .series
                    .time_of_index(fr.xm.idx(h.x), self.timeframe.millis()),
                price: ym.v(h.y),
            };
            items.push((Drawing { kind, a, b }, true));
        }
        let font = FontId::monospace(10.5);
        for (d, selected) in &items {
            let (pa, pb) = self.anchor_pos(fr, ym, d);
            let stroke = Stroke::new(if *selected { 2.0 } else { 1.5 }, base);
            match d.kind {
                DrawingKind::TrendLine => {
                    p.line_segment([pa, pb], stroke);
                }
                DrawingKind::HorizontalLine => {
                    p.hline(fr.plot.x_range(), pa.y, stroke);
                    axis_label(
                        &fr.painter,
                        pos2(fr.plot.right(), pa.y),
                        fmt_fixed(d.a.price, fr.decimals),
                        base,
                        Color32::WHITE,
                        true,
                    );
                }
                DrawingKind::VerticalLine => {
                    fr.painter
                        .with_clip_rect(fr.plot)
                        .vline(pa.x, fr.plot.y_range(), stroke);
                }
                DrawingKind::Rectangle => {
                    let r = Rect::from_two_pos(pa, pb);
                    p.rect_filled(r, 0.0, with_alpha(base, 36));
                    p.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Middle);
                }
                DrawingKind::Fibonacci => {
                    let (x0, x1) = (pa.x.min(pb.x), pa.x.max(pb.x));
                    let fib_colors = [
                        fr.theme.text_dim,
                        fr.theme.down,
                        Color32::from_rgb(255, 152, 0),
                        fr.theme.up,
                        Color32::from_rgb(0, 172, 193),
                        fr.theme.accent,
                        fr.theme.text_dim,
                    ];
                    for (k, lvl) in FIB_LEVELS.iter().enumerate() {
                        let price = d.b.price + (d.a.price - d.b.price) * lvl;
                        let y = ym.y(price);
                        let c = fib_colors[k];
                        p.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(1.2, c));
                        if k + 1 < FIB_LEVELS.len() {
                            let y2 = ym.y(d.b.price + (d.a.price - d.b.price) * FIB_LEVELS[k + 1]);
                            p.rect_filled(
                                Rect::from_x_y_ranges(x0..=x1, y.min(y2)..=y.max(y2)),
                                0.0,
                                with_alpha(c, 22),
                            );
                        }
                        p.text(
                            pos2(x0 - 4.0, y),
                            Align2::RIGHT_CENTER,
                            format!("{lvl} ({})", fmt_fixed(price, fr.decimals)),
                            font.clone(),
                            c,
                        );
                    }
                    p.extend(Shape::dashed_line(
                        &[pa, pb],
                        Stroke::new(1.0, fr.theme.text_dim),
                        4.0,
                        4.0,
                    ));
                }
            }
            if *selected && !d.kind.one_point() {
                for h in [pa, pb] {
                    p.circle(h, 4.5, fr.theme.bg, Stroke::new(1.5, base));
                }
            }
        }
    }

    fn draw_legend(&self, fr: &Frame, main: Rect, i: usize, overlays: &[&IndicatorOutput]) {
        let p = &fr.painter;
        let t = fr.theme;
        let b = fr.bars[i];
        let prev_close = if i > 0 { fr.bars[i - 1].close } else { b.open };
        let ch = b.close - prev_close;
        let pct = if prev_close != 0.0 {
            ch / prev_close * 100.0
        } else {
            0.0
        };
        let c = t.candle(ch >= 0.0);
        let f = FontId::monospace(12.0);
        let mut x = main.left() + 8.0;
        let y = main.top() + 6.0;
        let mut put = |text: String, color: Color32, font: &FontId| {
            let r = p.text(pos2(x, y), Align2::LEFT_TOP, text, font.clone(), color);
            x = r.right() + 8.0;
        };
        put(
            format!("{} · {}", self.title, self.timeframe.label()),
            t.text,
            &FontId::proportional(13.0),
        );
        for (k, v) in [("O", b.open), ("H", b.high), ("L", b.low), ("C", b.close)] {
            put(format!("{k} {}", fmt_fixed(v, fr.decimals)), c, &f);
        }
        put(format!("{:+.*} ({pct:+.2}%)", fr.decimals, ch), c, &f);
        put(
            format!("Vol {}", fmt_compact(self.series.bars[i].volume)),
            t.text_dim,
            &f,
        );

        let mut y = y + 20.0;
        for o in overlays {
            legend_line(
                p,
                pos2(main.left() + 8.0, y),
                &o.label,
                o,
                i,
                t,
                Some(fr.decimals),
            );
            y += 17.0;
        }
    }
}

fn scale_price(st: &mut ChartState, factor: f32) {
    let (lo, hi) = st.price_range;
    let mid = (lo + hi) / 2.0;
    let half = (hi - lo) / 2.0 * factor as f64;
    st.price_range = (mid - half, mid + half);
    st.auto_scale = false;
}

fn pane_scale(o: &IndicatorOutput, r: Rect, i0: usize, i1: usize) -> YMap {
    let (min, max) = match &o.pane {
        Pane::Separate { range: Some(r), .. } => *r,
        _ => {
            let mut lo = f64::MAX;
            let mut hi = f64::MIN;
            for p in &o.plots {
                for v in p.values[i0..=i1].iter().filter(|v| v.is_finite()) {
                    lo = lo.min(*v);
                    hi = hi.max(*v);
                }
            }
            if let Pane::Separate { levels, .. } = &o.pane {
                for l in levels {
                    lo = lo.min(*l);
                    hi = hi.max(*l);
                }
            }
            if lo > hi {
                (0.0, 1.0)
            } else {
                let pad = ((hi - lo) * 0.1).max(1e-9);
                (lo - pad, hi + pad)
            }
        }
    };
    YMap {
        top: r.top() + 6.0,
        bottom: r.bottom() - 6.0,
        min,
        max,
    }
}

fn draw_plots(fr: &Frame, p: &Painter, ym: &YMap, o: &IndicatorOutput) {
    let mut k = 0;
    while k < o.plots.len() {
        let plot = &o.plots[k];
        let color = rgba(plot.color);
        match plot.style {
            PlotStyle::Line => polyline(fr, p, ym, &plot.values, Stroke::new(1.4, color)),
            PlotStyle::Histogram => {
                let w = (fr.xm.bar_w * 0.6).max(1.0);
                let zero = ym.y(0.0);
                for i in fr.i0..=fr.i1 {
                    let v = plot.values[i];
                    if !v.is_finite() {
                        continue;
                    }
                    let x = fr.xm.x(i as f64);
                    let rising = i > 0 && plot.values[i - 1].is_finite() && v >= plot.values[i - 1];
                    let c = match (v >= 0.0, rising) {
                        (true, true) => fr.theme.up,
                        (true, false) => with_alpha(fr.theme.up, 120),
                        (false, false) => fr.theme.down,
                        (false, true) => with_alpha(fr.theme.down, 120),
                    };
                    let y = ym.y(v);
                    p.rect_filled(
                        Rect::from_x_y_ranges(x - w / 2.0..=x + w / 2.0, y.min(zero)..=y.max(zero)),
                        0.0,
                        c,
                    );
                }
            }
            PlotStyle::BandFill => {
                if let Some(other) = o.plots.get(k + 1) {
                    let mut mesh = Mesh::default();
                    let fill = with_alpha(color, 24);
                    let mut prev = false;
                    for i in fr.i0..=fr.i1 {
                        let (a, b) = (plot.values[i], other.values[i]);
                        let ok = a.is_finite() && b.is_finite();
                        if ok {
                            let x = fr.xm.x(i as f64);
                            mesh.colored_vertex(pos2(x, ym.y(a)), fill);
                            mesh.colored_vertex(pos2(x, ym.y(b)), fill);
                            if prev {
                                let j = mesh.vertices.len() as u32;
                                mesh.add_triangle(j - 4, j - 3, j - 2);
                                mesh.add_triangle(j - 3, j - 2, j - 1);
                            }
                        }
                        prev = ok;
                    }
                    p.add(Shape::mesh(mesh));
                }
                polyline(fr, p, ym, &plot.values, Stroke::new(1.2, color));
            }
        }
        k += 1;
    }
}

fn polyline(fr: &Frame, p: &Painter, ym: &YMap, values: &[f64], stroke: Stroke) {
    let mut run: Vec<Pos2> = Vec::new();
    let flush = |run: &mut Vec<Pos2>| {
        if run.len() > 1 {
            p.add(Shape::line(std::mem::take(run), stroke));
        }
        run.clear();
    };
    for i in fr.i0..=fr.i1 {
        let v = values[i];
        if v.is_finite() {
            run.push(pos2(fr.xm.x(i as f64), ym.y(v)));
        } else {
            flush(&mut run);
        }
    }
    flush(&mut run);
}

/// `price_decimals`: format values as prices (overlays) instead of compactly.
fn legend_line(
    p: &Painter,
    pos: Pos2,
    label: &str,
    o: &IndicatorOutput,
    i: usize,
    t: Theme,
    price_decimals: Option<usize>,
) {
    let f = FontId::monospace(11.5);
    let r = p.text(pos, Align2::LEFT_TOP, label, f.clone(), t.text_dim);
    let mut x = r.right() + 8.0;
    for plot in &o.plots {
        let v = plot.values.get(i).copied().unwrap_or(f64::NAN);
        let text = match price_decimals {
            Some(d) if v.is_finite() => fmt_fixed(v, d),
            _ => fmt_compact(v),
        };
        let r = p.text(
            pos2(x, pos.y),
            Align2::LEFT_TOP,
            text,
            f.clone(),
            rgba(plot.color),
        );
        x = r.right() + 8.0;
    }
}

/// A filled value tag on the price axis, vertically centred at `at.y`.
fn axis_label(p: &Painter, at: Pos2, text: String, bg: Color32, fg: Color32, right_axis: bool) {
    let g = p.layout_no_wrap(text, FontId::monospace(11.0), fg);
    let size = g.size() + vec2(10.0, 6.0);
    let min = if right_axis {
        pos2(at.x + 1.0, at.y - size.y / 2.0)
    } else {
        at - Vec2::new(size.x, size.y / 2.0)
    };
    let r = Rect::from_min_size(min, vec2(AXIS_W - 2.0, size.y));
    p.with_clip_rect(p.clip_rect().union(r))
        .rect_filled(r, 2.0, bg);
    p.with_clip_rect(p.clip_rect().union(r))
        .galley(r.min + vec2(7.0, 3.0), g, fg);
}
