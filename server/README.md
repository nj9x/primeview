# primeview-server

Server-side price alerts for PrimeView. It runs the same alert engine as the
app (`crates/primeview-alerts`) against live prices, so alerts keep firing
while the app is closed. Fired alerts go out over Server-Sent Events, to
webhooks and to the log.

Price sources:

| Symbol prefix | Source | How |
|---|---|---|
| `BINANCE:` (or no prefix) | Binance spot | One combined websocket, `wss://data-stream.binance.vision/stream?streams=<sym>@aggTrade/...`. Reconnects with backoff (1 s up to 60 s) and resubscribes when the symbol set changes. |
| `TWELVEDATA:` | [Twelve Data](https://twelvedata.com) | Polls `https://api.twelvedata.com/price` with every symbol in one batched request, every 60 s by default. Backs off (up to 10 min) on 401/403/429. |

Symbols are `PROVIDER:TICKER`, uppercase: `BINANCE:BTCUSDT`, `TWELVEDATA:AAPL`,
`TWELVEDATA:EUR/USD`. A bare ticker such as `btcusdt` means `BINANCE:BTCUSDT`.

## Run

```sh
cargo run --release -p primeview-server
# or with options
cargo run --release -p primeview-server -- --bind 0.0.0.0:8787 --data /var/lib/primeview/alerts.json
```

`primeview-server --help` lists every flag. Logging uses `RUST_LOG`
(default `info,tower_http=warn`; `RUST_LOG=debug` shows every price).

## Configuration

Every flag can also be set with an environment variable.

| Flag | Env var | Default | Meaning |
|---|---|---|---|
| `--bind` | `PRIMEVIEW_BIND` | `127.0.0.1:8787` | Listen address |
| `--data` | `PRIMEVIEW_DATA` | `./primeview-data/alerts.json` | Alert store (created if missing) |
| `--webhooks` | `PRIMEVIEW_WEBHOOKS` | none | Comma-separated http(s) URLs that receive each fired alert |
| `--token` | `PRIMEVIEW_TOKEN` | none | When set, required on `/api/alerts*` and `/api/events` |
| `--twelvedata-key` | `PRIMEVIEW_TWELVEDATA_KEY` | `demo` | Twelve Data API key |
| `--poll-secs` | `PRIMEVIEW_POLL_SECS` | `60` | Seconds between Twelve Data polls |
| `--no-feeds` | `PRIMEVIEW_NO_FEEDS` | off | Serve the API without connecting to any price feed |

Twelve Data notes: the `demo` key only covers a handful of symbols (for
example `AAPL`, `EUR/USD`). The free plan allows 8 API credits per minute and a
batched request costs one credit per symbol, so keep at most 8 Twelve Data
symbols at the default 60 s interval, or raise `PRIMEVIEW_POLL_SECS`.
When the symbol set changes, the next poll happens within 10 s instead of
waiting for the full interval.

The data file is rewritten atomically (temp file + rename) after every change
and after alerts fire. If the file exists but cannot be parsed the server
refuses to start rather than overwrite it.

## API

All responses are JSON. Errors look like `{"error": "..."}`. CORS allows any
origin.

When `PRIMEVIEW_TOKEN` is set, send `Authorization: Bearer <token>` on
`/api/alerts*` and `/api/events`. Clients that cannot set headers (a browser
`EventSource`) may use `?token=<token>` instead. `/api/health` is always open.

### Alert shape

```json
{
  "id": 1,
  "symbol": "BINANCE:BTCUSDT",
  "condition": {"CrossingUp": 65000.0},
  "frequency": "Once",
  "message": "",
  "active": true,
  "created_ms": 1795773207946,
  "last_fired_ms": null,
  "expires_ms": null
}
```

- `condition`: one of `{"Crossing": x}`, `{"CrossingUp": x}`,
  `{"CrossingDown": x}`, `{"Above": x}`, `{"Below": x}`.
  Crossing up means `previous < x <= price`, crossing down
  `previous > x >= price`; crossings need a previous price, so the first price
  after start-up only records it. `Above`/`Below` are strict and fire on any
  price.
- `frequency`: `"Once"` (fires, then becomes inactive) or
  `{"EveryTime": {"cooldown_ms": 60000}}` (fires each time the condition holds,
  at most once per cooldown).
- `expires_ms`: optional Unix ms; the alert is deactivated at that time.
- An empty `message` fires as `"<SYMBOL> <condition>"`, e.g.
  `"BINANCE:BTCUSDT crossing up 65000"`.

### Endpoints

```sh
S=http://127.0.0.1:8787
AUTH='Authorization: Bearer my-token'   # only when PRIMEVIEW_TOKEN is set

# Health: version, alert count, symbols being watched
curl $S/api/health

# List alerts
curl -H "$AUTH" $S/api/alerts

# Create (201 + the alert). frequency defaults to "Once", message to "".
curl -H "$AUTH" -H 'content-type: application/json' -X POST $S/api/alerts \
  -d '{"symbol":"BINANCE:BTCUSDT","condition":{"CrossingUp":65000},"frequency":"Once","message":"BTC 65k"}'
curl -H "$AUTH" -H 'content-type: application/json' -X POST $S/api/alerts \
  -d '{"symbol":"TWELVEDATA:EUR/USD","condition":{"Below":1.05},
       "frequency":{"EveryTime":{"cooldown_ms":3600000}},"message":"","expires_ms":1798761600000}'

# Pause / resume (200 + the alert)
curl -H "$AUTH" -H 'content-type: application/json' -X PATCH $S/api/alerts/1 -d '{"active":false}'

# Delete (204)
curl -H "$AUTH" -X DELETE $S/api/alerts/1

# Live stream of fired alerts (Server-Sent Events)
curl -N -H "$AUTH" $S/api/events
```

Create returns 400 for an empty or malformed symbol, a provider other than
`BINANCE`/`TWELVEDATA`, a negative cooldown or an `expires_ms` in the past.
Unknown ids return 404.

`GET /api/events` sends one default (`message`) event per fired alert, with a
`Fired` object as `data`, plus keep-alive comments:

```
data: {"alert_id":1,"symbol":"BINANCE:BTCUSDT","price":65000.1,"time_ms":1795773210549,"message":"BTC 65k"}
```

In a browser: `new EventSource(S + "/api/events?token=...").onmessage = e => JSON.parse(e.data)`.

## Webhooks

Each URL in `PRIMEVIEW_WEBHOOKS` gets one `POST` per fired alert with
`Content-Type: application/json` and the same `Fired` object:

```json
{"alert_id":1,"symbol":"BINANCE:BTCUSDT","price":65000.1,"time_ms":1795773210549,"message":"BTC 65k"}
```

`time_ms` is the trade time for Binance and the poll time for Twelve Data.
Delivery is fire-and-forget: 15 s timeout, no retries, failures and non-2xx
responses are logged as warnings. Webhooks never slow down the price feed.

## Deployment

The server is one self-contained binary (TLS via rustls, no OpenSSL; it only
needs libc and the system CA certificates). Run it behind a reverse proxy
(Caddy, nginx) for HTTPS and set `PRIMEVIEW_TOKEN` whenever it listens on
anything other than localhost. If the proxy buffers responses, turn buffering
off for `/api/events`. It shuts down cleanly on Ctrl-C or SIGTERM, ending open
SSE streams and saving the alerts.

systemd (`/etc/systemd/system/primeview.service`):

```ini
[Unit]
Description=PrimeView alerts server
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/primeview-server
Environment=PRIMEVIEW_BIND=127.0.0.1:8787
Environment=PRIMEVIEW_DATA=/var/lib/primeview/alerts.json
EnvironmentFile=-/etc/primeview/env
DynamicUser=yes
StateDirectory=primeview
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

Put secrets (`PRIMEVIEW_TOKEN`, `PRIMEVIEW_TWELVEDATA_KEY`,
`PRIMEVIEW_WEBHOOKS`) in `/etc/primeview/env`.

Docker (build from the repository root, since the server is part of the
workspace; a `.dockerignore` excluding `target/` keeps the build context small):

```sh
docker build -f server/Dockerfile -t primeview-server .
docker run -d -p 8787:8787 -v primeview-data:/data \
  -e PRIMEVIEW_TOKEN=change-me -e PRIMEVIEW_TWELVEDATA_KEY=your-key primeview-server
```

## Development

```sh
cargo test -p primeview-alerts -p primeview-server
cargo clippy -p primeview-alerts -p primeview-server --all-targets -- -D warnings
```

The server tests drive the router in-process and never start the price feeds
or touch the internet (the webhook test posts to a local listener).
