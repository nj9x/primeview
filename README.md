# PrimeView

A fast, native charting and market-analysis platform written in Rust. One
codebase builds for **web (WebAssembly)**, **Windows**, **macOS** and Linux,
with an optional **alert server** that keeps alerts firing around the clock.

![status](https://img.shields.io/badge/status-early%20prototype-orange)

## Features

- **Markets**
  - **Crypto** via Binance: no key, real-time WebSocket
  - **Stocks, ETFs, indices, forex and commodities** via [Twelve Data](https://twelvedata.com): bring your own free key (⚙ menu); updates are polled every minute
  - Symbols are `PROVIDER:TICKER` (`BINANCE:BTCUSDT`, `TWELVEDATA:AAPL`, `TWELVEDATA:EUR/USD`); type a bare `BTCUSDT`, `AAPL` or `EUR/USD` and the provider is picked for you
  - An offline `DEMO` feed
- **Multi-chart layouts:**
  - 1, 2 (side by side / stacked), 3 (columns / 1 large + 2) or a 2×2 grid
  - each pane has its own symbol, interval, indicators and scripts
  - crosshair, symbol and interval can be synced across panes
- **Charting:**
  - candles, hollow candles, bars, line, area, Heikin Ashi
  - volume, multi-pane indicators, infinite scroll-back history
  - zoom/pan, axis scaling, crosshair legend
- **Indicators:** SMA, EMA, Bollinger, VWAP, RSI, MACD, ATR and Stochastic
- **PrimeScript:** a safe, vectorized custom-indicator language with an in-app editor, live diagnostics, editable inputs and 8 examples. See [LANGUAGE.md](crates/primeview-script/LANGUAGE.md)
- **Drawing tools:** trend line, horizontal/vertical line, rectangle, Fibonacci. Drawings are editable, saved per symbol and shared across panes
- **Alerts:**
  - crossing / crossing up / crossing down / above / below, triggering once or every time
  - create them from the alert panel or by right-clicking a chart
  - shown as lines on the chart, with toasts when they fire
  - they run locally while the app is open, or on the **alert server** around the clock, which also posts to webhooks
- **Installers:**
  - macOS `.app` / `.dmg` (universal)
  - Windows NSIS + MSI
  - Linux `.deb` + AppImage
  - a tag-triggered release workflow with optional code signing
- Watchlist with live quotes, dark/light themes, and everything persists across restarts

## Architecture

```
crates/
  primeview-core     Bars, Series, Timeframe, UTC calendar math, built-in indicators (pure Rust)
  primeview-data     Data providers: Binance (REST + WS), Twelve Data (REST), demo feed; symbol model
  primeview-chart    The chart engine: an egui widget (layout, scales, rendering, input, drawings, alert lines)
  primeview-script   PrimeScript: lexer, parser, type checker, vectorized evaluator (no deps, wasm-safe)
  primeview-alerts   Alert model + evaluation engine, shared by the app and the server (wasm-safe)
app/                 The application (eframe): layouts, toolbar, watchlist, alerts UI, script editor
server/              primeview-server: axum alert server (REST + SSE + webhooks, Binance WS + Twelve Data feeds)
packaging/           Icons, cargo-packager config, build script for installers
```

Everything except `app` and `server` is platform-agnostic, and the same chart,
indicator, script and alert code runs in the browser, on the desktop and on the server.

## Run it

Prerequisites: Rust (stable), plus `wasm32-unknown-unknown` and [`trunk`](https://trunkrs.dev) for the web build.

```bash
# Desktop app
cargo run --release -p primeview

# Web app: builds to ./dist and serves at http://127.0.0.1:8080
cd app && trunk serve --release

# Alert server (see server/README.md for flags, webhooks, auth, Docker)
cargo run --release -p primeview-server

# Installers (see packaging/README.md)
cargo build --release -p primeview && packaging/package.sh

# Tests and lints
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

To use stock/forex data, get a free Twelve Data key and paste it into **⚙ → Data providers**.
The shared `demo` key only covers a few symbols (for example AAPL and EUR/USD).

To use server alerts, run `primeview-server`, then open **🔔 Alerts → Alert server**
in the app and enter its URL (and token, if you set `PRIMEVIEW_TOKEN`).

## Controls

| Action | Input |
|---|---|
| Zoom | Mouse wheel / pinch over the chart, or drag the time axis |
| Pan | Drag the chart, horizontal trackpad scroll, ← → keys |
| Scale price | Drag or scroll the price axis (double-click = auto) |
| Reset view | `Home`, or double-click the time axis |
| Active pane | Click a chart: the toolbar, tools and indicator menu act on it |
| Draw | Pick a tool on the left, click on the chart (two clicks for 2-point tools) |
| Edit drawing | Drag a handle or the line; `Delete` removes the selected one |
| Context menu | Right-click a chart: add alert here, horizontal line, remove drawing, reset view |
| Cancel tool | `Esc` |

## Roadmap

- **Scripting:** strategies and a backtester, script-defined alerts, markers/shapes, user functions, a public script library
- **Data:** more providers (Polygon, Alpaca, Coinbase, Kraken, IBKR), real-time stock streaming, symbol search, our own aggregation/cache relay
- **Charting:** log/percent scales, compare symbols, more drawing tools, magnet snap, bar replay, session breaks
- **Platform:** accounts and cloud sync, mobile/push delivery for alerts, screener, news, auto-update (see `packaging/README.md`)

## License

MIT OR Apache-2.0
