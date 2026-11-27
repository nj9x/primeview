//! Shared server state: the alert engine, persistence and fire dispatch.

use primeview_alerts::{Engine, Fired};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Notify, broadcast, watch};

/// Runtime settings that handlers and feeds need.
#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub data_path: PathBuf,
    pub webhooks: Vec<String>,
    pub token: Option<String>,
    pub twelvedata_key: String,
    pub poll_secs: u64,
}

pub type AppState = Arc<State>;

pub struct State {
    pub settings: Settings,
    engine: Mutex<Engine>,
    /// Serializes writes to the data file so renames never interleave.
    save_lock: tokio::sync::Mutex<()>,
    pub fired_tx: broadcast::Sender<Fired>,
    /// Signalled whenever the set of alerts may have changed.
    pub alerts_changed: Notify,
    pub http: reqwest::Client,
    pub shutdown: watch::Receiver<bool>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl State {
    pub fn new(settings: Settings, engine: Engine, shutdown: watch::Receiver<bool>) -> AppState {
        let (fired_tx, _) = broadcast::channel(1024);
        let http = reqwest::Client::builder()
            .user_agent(concat!("primeview-server/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(15))
            .build()
            .expect("building the HTTP client");
        Arc::new(State {
            settings,
            engine: Mutex::new(engine),
            save_lock: tokio::sync::Mutex::new(()),
            fired_tx,
            alerts_changed: Notify::new(),
            http,
            shutdown,
        })
    }

    /// Lock the engine. Never hold the guard across an `.await`.
    pub fn engine(&self) -> MutexGuard<'_, Engine> {
        // A panic while holding the lock leaves the engine in a consistent
        // state (all mutations are single steps), so recover from poisoning.
        self.engine.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Write the engine to the data file atomically (temp file + rename).
    pub async fn persist(&self) -> std::io::Result<()> {
        let _guard = self.save_lock.lock().await;
        let json = serde_json::to_vec_pretty(&*self.engine()).map_err(std::io::Error::other)?;
        write_atomic(&self.settings.data_path, &json).await
    }

    /// Persist and log on failure (for background callers).
    pub async fn persist_logged(&self) {
        if let Err(e) = self.persist().await {
            tracing::error!(path = %self.settings.data_path.display(), "persisting alerts failed: {e}");
        }
    }

    /// Feed one price into the engine and dispatch whatever fires.
    /// Never blocks on network or disk: those run in spawned tasks.
    pub fn on_price(self: &Arc<Self>, symbol: &str, price: f64, time_ms: i64) -> Vec<Fired> {
        let fired = self.engine().on_price(symbol, price, time_ms);
        if !fired.is_empty() {
            self.dispatch(&fired);
        }
        fired
    }

    fn dispatch(self: &Arc<Self>, fired: &[Fired]) {
        for f in fired {
            tracing::info!(
                alert_id = f.alert_id,
                symbol = %f.symbol,
                price = f.price,
                time_ms = f.time_ms,
                "alert fired: {}",
                f.message
            );
            // No subscribers is fine.
            let _ = self.fired_tx.send(f.clone());
            for url in &self.settings.webhooks {
                let (http, url, f) = (self.http.clone(), url.clone(), f.clone());
                tokio::spawn(async move {
                    match http.post(&url).json(&f).send().await {
                        Ok(r) if r.status().is_success() => {
                            tracing::debug!(%url, alert_id = f.alert_id, "webhook delivered")
                        }
                        Ok(r) => tracing::warn!(%url, status = %r.status(), "webhook rejected"),
                        Err(e) => tracing::warn!(%url, "webhook failed: {e}"),
                    }
                });
            }
        }
        let state = self.clone();
        tokio::spawn(async move { state.persist_logged().await });
        // Once-alerts deactivated: the feed set may shrink.
        self.alerts_changed.notify_one();
    }
}

async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(dir).await?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await
}

/// Load the engine from `path`, or start empty when the file does not exist.
/// A file that exists but cannot be parsed is an error (never overwrite it).
pub fn load_engine(path: &Path) -> Result<Engine, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("cannot parse {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Engine::new()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}
