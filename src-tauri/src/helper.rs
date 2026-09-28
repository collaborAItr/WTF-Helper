//! The helper's behaviour, apart from the window and tray: pairing, the stream,
//! handling each check, Pause, Disconnect and the activity log.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;

use crate::checks::queries::{self, Platform};
use crate::checks::{field, run_batch, Machine};
use crate::flavor::{FLAVOR, HELPER_VERSION};
use crate::gate::{decide, Decision, Refusal};
use crate::protocol::{
    clip_chars, parse_stream_data, Approval, CheckName, CheckResult, ClaimRequest, RawCheckEvent,
    ResultBody, StreamEvent,
};
use crate::relay::{Api, ApiError, SseParser};
use crate::settings::{Pairing, Settings, SettingsFile};
use crate::timeutil::{now_ms, parse_http_date_ms, parse_iso_utc_ms};
use crate::tokens::TokenStore;

/// Server heartbeats come every 10 seconds; this long with no bytes means the stream is gone.
pub const STREAM_SILENCE: Duration = Duration::from_secs(30);
const BACKOFF_MAX_SECS: u64 = 30;
const SERVICE_OFF_RETRY: Duration = Duration::from_secs(60);
const ACTIVITY_MAX: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Connection {
    NotPaired,
    Connecting,
    Connected,
    Reconnecting,
    ServiceOff,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub at_ms: i64,
    pub checks: Vec<String>,
    pub approval: Option<String>,
    pub outcome: String,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingView {
    pub account_hint: String,
    pub device_name: String,
    pub paired_at_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusView {
    pub product_name: &'static str,
    pub service_name: &'static str,
    pub host: &'static str,
    pub is_test: bool,
    pub version: &'static str,
    pub paired: Option<PairingView>,
    pub paused: bool,
    pub connection: Connection,
    pub notice: Option<String>,
    pub activity: Vec<Activity>,
}

struct State {
    settings: Settings,
    connection: Connection,
    notice: Option<String>,
    activity: VecDeque<Activity>,
}

pub type Notify = Box<dyn Fn(&StatusView) + Send + Sync>;

pub struct Helper<M: Machine + Send + 'static> {
    api: Api,
    machine: M,
    tokens: Box<dyn TokenStore>,
    settings_file: SettingsFile,
    state: Mutex<State>,
    /// Bumped on pair and disconnect, so work from an older pairing is dropped.
    generation: AtomicU64,
    /// Service clock minus this computer's clock, from the latest heartbeat.
    clock_offset_ms: AtomicI64,
    stream_task: Mutex<Option<tokio::task::AbortHandle>>,
    notify: Notify,
}

fn account_hint(user_id: &str) -> String {
    let tail: String = user_id
        .chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

impl<M: Machine + Send + 'static> Helper<M> {
    pub fn new(
        api: Api,
        machine: M,
        tokens: Box<dyn TokenStore>,
        settings_file: SettingsFile,
        notify: Notify,
    ) -> Arc<Self> {
        let mut settings = settings_file.load();
        if settings.pairing.is_some() && tokens.get().is_none() {
            settings.pairing = None;
            let _ = settings_file.save(&settings);
        }
        let connection = if settings.pairing.is_some() {
            Connection::Connecting
        } else {
            Connection::NotPaired
        };
        Arc::new(Helper {
            api,
            machine,
            tokens,
            settings_file,
            state: Mutex::new(State {
                settings,
                connection,
                notice: None,
                activity: VecDeque::new(),
            }),
            generation: AtomicU64::new(0),
            clock_offset_ms: AtomicI64::new(0),
            stream_task: Mutex::new(None),
            notify,
        })
    }

    pub fn status(&self) -> StatusView {
        let state = self.state.lock().unwrap();
        StatusView {
            product_name: FLAVOR.product_name,
            service_name: FLAVOR.service_name,
            host: FLAVOR.host,
            is_test: FLAVOR.is_test,
            version: HELPER_VERSION,
            paired: state.settings.pairing.as_ref().map(|p| PairingView {
                account_hint: account_hint(&p.user_id),
                device_name: p.device_name.clone(),
                paired_at_ms: p.paired_at_ms,
            }),
            paused: state.settings.paused,
            connection: state.connection,
            notice: state.notice.clone(),
            activity: state.activity.iter().cloned().collect(),
        }
    }

    fn changed(&self) {
        (self.notify)(&self.status());
    }

    fn set_connection(&self, connection: Connection) {
        let mut state = self.state.lock().unwrap();
        if state.connection == connection {
            return;
        }
        state.connection = connection;
        drop(state);
        self.changed();
    }

    pub fn is_paired(&self) -> bool {
        self.state.lock().unwrap().settings.pairing.is_some()
    }

    pub fn is_paused(&self) -> bool {
        self.state.lock().unwrap().settings.paused
    }

    fn log(&self, entry: Activity) {
        let mut state = self.state.lock().unwrap();
        state.activity.push_front(entry);
        state.activity.truncate(ACTIVITY_MAX);
        drop(state);
        self.changed();
    }

    pub fn set_paused(&self, paused: bool) -> Result<StatusView, String> {
        let mut state = self.state.lock().unwrap();
        state.settings.paused = paused;
        let saved = self.settings_file.save(&state.settings);
        drop(state);
        self.changed();
        saved.map(|_| self.status())
    }

    async fn device_details(&self) -> (String, Option<String>) {
        let platform = self.machine.platform();
        let system = self
            .machine
            .query(&queries::SYSTEM_INFO)
            .await
            .unwrap_or_default();
        let os = self
            .machine
            .query(&queries::OS_VERSION)
            .await
            .unwrap_or_default();
        let name = system
            .first()
            .map(|r| field(r, "computer_name").to_string())
            .filter(|n| !n.is_empty());
        let fallback = match platform {
            Platform::Mac => "Mac",
            Platform::Windows => "Windows PC",
        };
        let version = os.first().map(|r| match platform {
            Platform::Mac => field(r, "version").to_string(),
            Platform::Windows => format!("{} ({})", field(r, "name"), field(r, "version")),
        });
        (
            clip_chars(name.as_deref().unwrap_or(fallback), 80),
            version
                .filter(|v| !v.trim().is_empty())
                .map(|v| clip_chars(v.trim(), 80)),
        )
    }

    /// Claims a six-digit code, keeps the token in the keychain, and starts the stream.
    pub async fn pair(self: &Arc<Self>, code: &str) -> Result<StatusView, String> {
        let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Type the 6-digit code from collaborAItr.".into());
        }
        let (device_name, os_version) = self.device_details().await;
        let request = ClaimRequest {
            code,
            device_name: device_name.clone(),
            os: self.machine.platform().os_name().to_string(),
            os_version,
            helper_version: HELPER_VERSION.to_string(),
        };
        let claimed = self.api.claim(&request).await.map_err(|e| e.plain())?;
        self.tokens.set(&claimed.device_token).map_err(|_| {
            "Could not save the connection in this computer's keychain.".to_string()
        })?;
        self.generation.fetch_add(1, Ordering::SeqCst);
        {
            let mut state = self.state.lock().unwrap();
            state.settings.pairing = Some(Pairing {
                host: FLAVOR.host.to_string(),
                device_id: claimed.device_id,
                user_id: claimed.user_id,
                device_name,
                paired_at_ms: now_ms(),
            });
            state.notice = None;
            state.connection = Connection::Connecting;
            self.settings_file.save(&state.settings)?;
        }
        self.changed();
        self.start_stream();
        Ok(self.status())
    }

    /// Forgets the token on this computer and stops the stream.
    pub fn disconnect(&self, notice: Option<String>) -> StatusView {
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(task) = self.stream_task.lock().unwrap().take() {
            task.abort();
        }
        let _ = self.tokens.delete();
        {
            let mut state = self.state.lock().unwrap();
            state.settings.pairing = None;
            state.connection = Connection::NotPaired;
            state.notice = notice;
            let _ = self.settings_file.save(&state.settings);
        }
        self.changed();
        self.status()
    }

    /// Must be called from inside the async runtime.
    pub fn start_stream(self: &Arc<Self>) {
        let mut slot = self.stream_task.lock().unwrap();
        if let Some(task) = slot.take() {
            task.abort();
        }
        if !self.is_paired() {
            return;
        }
        let helper = self.clone();
        *slot = Some(tokio::spawn(async move { helper.run_stream().await }).abort_handle());
    }

    /// Holds the stream, reconnecting after drops, until the pairing ends.
    pub async fn run_stream(self: Arc<Self>) {
        let generation = self.generation.load(Ordering::SeqCst);
        let mut backoff = 1u64;
        let mut first = true;
        loop {
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            let Some(token) = self.tokens.get() else {
                self.disconnect(Some("The connection was missing from this computer's keychain. Connect again with a new code.".into()));
                return;
            };
            self.set_connection(if first {
                Connection::Connecting
            } else {
                Connection::Reconnecting
            });
            first = false;
            match self.api.open_stream(&token).await {
                Ok(response) => {
                    let server_date = response
                        .headers()
                        .get(reqwest::header::DATE)
                        .and_then(|v| v.to_str().ok())
                        .and_then(parse_http_date_ms);
                    if let Some(server_ms) = server_date {
                        self.clock_offset_ms
                            .store(server_ms - now_ms(), Ordering::SeqCst);
                    }
                    self.set_connection(Connection::Connected);
                    backoff = 1;
                    let mut body = response.bytes_stream();
                    let mut parser = SseParser::default();
                    while let Ok(Some(Ok(chunk))) =
                        tokio::time::timeout(STREAM_SILENCE, body.next()).await
                    {
                        for data in parser.push(&chunk) {
                            match parse_stream_data(&data) {
                                Some(StreamEvent::Check(event)) => {
                                    let helper = self.clone();
                                    let token = token.clone();
                                    tokio::spawn(async move {
                                        helper.handle_check(&token, event, generation).await
                                    });
                                }
                                Some(StreamEvent::Heartbeat { at: Some(at) }) => {
                                    if let Some(server_ms) = parse_iso_utc_ms(&at) {
                                        self.clock_offset_ms
                                            .store(server_ms - now_ms(), Ordering::SeqCst);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Err(ApiError::Unauthorized { .. }) => {
                    if self.generation.load(Ordering::SeqCst) == generation {
                        self.disconnect(Some(
                            "This computer was disconnected from your collaborAItr account. To use WTF Helper again, connect it with a new code.".into(),
                        ));
                    }
                    return;
                }
                Err(ApiError::NotEnabled) => {
                    self.set_connection(Connection::ServiceOff);
                    tokio::time::sleep(SERVICE_OFF_RETRY).await;
                    continue;
                }
                Err(_) => {}
            }
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            self.set_connection(Connection::Reconnecting);
            let jitter = (now_ms().rem_euclid(1000)) as u64;
            tokio::time::sleep(Duration::from_millis(backoff * 1000 + jitter)).await;
            backoff = (backoff * 2).min(BACKOFF_MAX_SECS);
        }
    }

    fn labels(names: &[String]) -> Vec<String> {
        names
            .iter()
            .map(|n| {
                CheckName::parse(n)
                    .map(|c| c.label().to_string())
                    .unwrap_or_else(|| n.clone())
            })
            .collect()
    }

    async fn post(
        &self,
        token: &str,
        request_id: &str,
        receipt: &str,
        approval: Approval,
        results: &[CheckResult],
    ) -> Result<(), String> {
        self.api
            .post_result(
                token,
                request_id,
                &ResultBody {
                    receipt,
                    approval,
                    results,
                },
            )
            .await
            .map_err(|e| e.plain())
    }

    /// Runs or refuses one check request and records it in the activity log.
    pub async fn handle_check(&self, token: &str, event: RawCheckEvent, generation: u64) {
        let offset = self.clock_offset_ms.load(Ordering::SeqCst);
        let decision = decide(&event, self.is_paused(), now_ms() + offset);
        match decision {
            Decision::Drop { names, reason } => self.log(Activity {
                at_ms: now_ms(),
                checks: Self::labels(&names),
                approval: event
                    .approval
                    .as_ref()
                    .and_then(Approval::parse)
                    .map(|a| a.activity_label().to_string()),
                outcome: reason.log_label().to_string(),
                ok: false,
            }),
            Decision::Refuse {
                request_id,
                receipt,
                approval,
                names,
                reason,
            } => {
                self.send_refusal(
                    token,
                    &request_id,
                    &receipt,
                    approval,
                    &names,
                    reason,
                    generation,
                )
                .await
            }
            Decision::Run {
                request_id,
                receipt,
                approval,
                checks,
                deadline_ms,
            } => {
                let names: Vec<String> = checks
                    .iter()
                    .map(|c| c.check.as_str().to_string())
                    .collect();
                let results = run_batch(&self.machine, &checks, deadline_ms - offset).await;
                if self.generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                // Paused while the checks ran: send nothing from them.
                if self.is_paused() {
                    self.send_refusal(
                        token,
                        &request_id,
                        &receipt,
                        approval,
                        &names,
                        Refusal::Paused,
                        generation,
                    )
                    .await;
                    return;
                }
                let failed = results.iter().filter(|r| !r.ok).count();
                let sent = self
                    .post(token, &request_id, &receipt, approval, &results)
                    .await;
                drop(results);
                let (outcome, ok) = match sent {
                    Ok(()) if failed == 0 => ("Sent".to_string(), true),
                    Ok(()) if failed == 1 => ("Sent. 1 check could not run.".to_string(), true),
                    Ok(()) => (format!("Sent. {failed} checks could not run."), true),
                    Err(error) => (format!("Could not send: {error}"), false),
                };
                self.log(Activity {
                    at_ms: now_ms(),
                    checks: Self::labels(&names),
                    approval: Some(approval.activity_label().to_string()),
                    outcome,
                    ok,
                });
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_refusal(
        &self,
        token: &str,
        request_id: &str,
        receipt: &str,
        approval: Approval,
        names: &[String],
        reason: Refusal,
        generation: u64,
    ) {
        if self.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let results: Vec<CheckResult> = names
            .iter()
            .map(|n| CheckResult::failure(n, reason.message()))
            .collect();
        let sent = self
            .post(token, request_id, receipt, approval, &results)
            .await;
        self.log(Activity {
            at_ms: now_ms(),
            checks: Self::labels(names),
            approval: Some(approval.activity_label().to_string()),
            outcome: match sent {
                Ok(()) => reason.log_label().to_string(),
                Err(_) => format!("{} (the service did not hear back)", reason.log_label()),
            },
            ok: false,
        });
    }
}

#[cfg(test)]
mod tests;
