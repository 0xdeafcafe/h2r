//! Client for private sim batches on Live-managed dedicated hosts.

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use live_proto::hosts::{EndReason, HostLease, HostState};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sim::batch::{Admission, CloudProvider, Schedule, dollars, estimate_microusd};
use sim::config::{Batch, MatchConfig};

use super::RunDir;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const START_TIMEOUT: Duration = Duration::from_secs(75);
const REPORT_GRACE: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Deserialize)]
struct OpenedBatch {
    token: String,
    provider: CloudProvider,
    max_parallel: NonZeroUsize,
}

#[derive(Deserialize)]
struct RunStatus {
    state: HostState,
    reason: Option<EndReason>,
    report: Option<Value>,
}

pub(super) struct Client {
    live: String,
    agent: ureq::Agent,
    batch_id: String,
    credentials: OpenedBatch,
}

impl Client {
    pub(super) fn open(live: Option<String>, token: Option<String>, tag: Option<&str>) -> Result<Self> {
        let live = live
            .or_else(|| std::env::var("H2_LIVE_SERVER").ok())
            .unwrap_or_else(|| "http://127.0.0.1:8074".into());
        let agent = agent();
        let live = live_base(&agent, &live)?;
        let admin = match token.or_else(|| std::env::var("H2_LIVE_TOKEN").ok()) {
            Some(token) if !token.is_empty() && token.bytes().all(|b| b.is_ascii_graphic()) => token,
            Some(_) => bail!("invalid Live admin token"),
            None => admin_token(&agent, &live, tag)?,
        };
        let batch_id = live_proto::sign::random_id();
        let url = format!("{live}/v1/admin/sim-runs/batches/{batch_id}");
        let response = authorized(agent.post(&url), &admin).call();
        let credentials = decode(response).context("opening a cloud sim batch on Live")?;
        Ok(Self {
            live,
            agent,
            batch_id,
            credentials,
        })
    }

    pub(super) fn batch_id(&self) -> &str {
        &self.batch_id
    }

    pub(super) fn run(&self, batch: &Batch, parallel: usize, budget: Option<u64>) -> usize {
        let parallel = parallel.min(self.credentials.max_parallel.get());
        let mut schedule = Schedule::new(parallel, budget);
        let mut next = batch.runs.iter().enumerate().peekable();
        let mut active = 0;
        let mut failed = 0;
        let mut stop = false;
        let (send, receive) = mpsc::channel();
        std::thread::scope(|scope| {
            loop {
                while !stop && let Some(&(n, run)) = next.peek() {
                    let cost = estimate_microusd(&run.config, self.credentials.provider);
                    match schedule.admit(active, cost) {
                        Admission::AtCapacity => break,
                        Admission::OverBudget => {
                            println!(
                                "Budget limit reached. {} runs left unqueued. Estimated queued cost ${:.6}.",
                                batch.runs.len() - n, dollars(schedule.reserved())
                            );
                            stop = true;
                            break;
                        }
                        Admission::Ready => {}
                    }
                    let _ = next.next();
                    active += 1;
                    let send = send.clone();
                    scope.spawn(move || {
                        let dir = RunDir::new(&batch.out, &run.id);
                        let result = self.run_match(n, &run.config, &dir);
                        let _ = send.send((n, result));
                    });
                }
                if active == 0 {
                    break;
                }
                let (n, result) = receive.recv().expect("cloud run workers send their result");
                active -= 1;
                let run = &batch.runs[n];
                let line = match result {
                    Ok(lease) => format!("report  {lease}"),
                    Err(error) => {
                        failed += 1;
                        if error.host_may_be_running && !stop {
                            eprintln!("Stopping new launches because Live could not confirm a host exited.");
                            stop = true;
                        }
                        format!("NO REPORT  {:#}", error.error)
                    }
                };
                println!("[{}/{}] {:<28} {line}", n + 1, batch.runs.len(), run.id);
                if let Err(e) = std::fs::write(
                    RunDir::new(&batch.out, &run.id).log(),
                    format!("{line}\n"),
                ) {
                    eprintln!("{}: could not write run log ({e})", run.id);
                }
            }
        });
        failed
    }

    fn run_match(&self, index: usize, config: &MatchConfig, dir: &RunDir) -> Result<String, Failure> {
        let run_id = format!("{}-{index}", self.batch_id);
        let url = format!("{}/v1/admin/sim-runs/{run_id}/hosts", self.live);
        let body = serde_json::to_vec(&json!({ "batch_id": self.batch_id(), "match": config }))
            .map_err(|e| Failure::new(e.into(), false))?;
        let lease = self.start(&url, &body)?;
        let report = self.wait(&url, &lease)?;
        save_report(dir, &report).map_err(|e| Failure::new(e, false))?;
        Ok(lease.id)
    }

    fn start(&self, url: &str, body: &[u8]) -> Result<HostLease, Failure> {
        let started = Instant::now();
        loop {
            let response = authorized(self.agent.post(url), &self.credentials.token)
                .timeout(START_TIMEOUT)
                .set("Content-Type", "application/json")
                .send_bytes(body);
            let status = status(&response);
            if status == Some(429) && started.elapsed() < START_TIMEOUT {
                std::thread::sleep(POLL_INTERVAL);
                continue;
            }
            let uncertain = status.is_none_or(|n| n < 400 || n == 408 || n >= 500);
            return decode(response)
                .with_context(|| format!("starting {url}"))
                .map_err(|e| Failure::new(e, uncertain));
        }
    }

    fn wait(&self, url: &str, lease: &HostLease) -> Result<Value, Failure> {
        let deadline = Instant::now()
            + START_TIMEOUT
            + Duration::from_secs(u64::from(lease.max_match_s))
            + REPORT_GRACE;
        let mut terminal_since = None;
        loop {
            let response = authorized(self.agent.get(url), &self.credentials.token).call();
            match status(&response) {
                None | Some(408 | 429 | 500..=599) => {} // retry transient poll failures
                _ => {
                    let status: RunStatus = decode(response)
                        .with_context(|| format!("polling host {}", lease.id))
                        .map_err(|e| Failure::new(e, terminal_since.is_none()))?;
                    if matches!(status.state, HostState::Ended | HostState::Failed) {
                        if let Some(report) = status.report {
                            return Ok(report);
                        }
                        let first = terminal_since.get_or_insert_with(Instant::now);
                        if status.state == HostState::Failed || first.elapsed() >= REPORT_GRACE {
                            return Err(Failure::new(
                                anyhow!(
                                    "host {} {:?} without a report ({:?})",
                                    lease.id,
                                    status.state,
                                    status.reason
                                ),
                                false,
                            ));
                        }
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(Failure::new(
                    anyhow!("timed out waiting for the report from host {}", lease.id),
                    terminal_since.is_none(),
                ));
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

struct Failure {
    error: anyhow::Error,
    host_may_be_running: bool,
}

impl Failure {
    fn new(error: anyhow::Error, host_may_be_running: bool) -> Self {
        Self {
            error,
            host_may_be_running,
        }
    }
}

pub(super) fn save_report(dir: &RunDir, report: &Value) -> Result<()> {
    if report["format"].as_str() != Some(sim::metrics::FORMAT) {
        bail!("Live returned an unsupported sim report");
    }
    serde_json::from_value::<sim::metrics::Report>(report.clone())
        .context("reading the host's sim report")?;
    let path = dir.report();
    let staging = path.with_extension("json.part");
    std::fs::write(&staging, serde_json::to_vec_pretty(report)?)?;
    std::fs::rename(staging, path)?;
    Ok(())
}

#[derive(Deserialize)]
pub(super) struct HostMatch {
    pub run_id: String,
    pub batch_id: String,
    #[serde(rename = "match")]
    pub matched: MatchConfig,
}

pub(super) fn host_match(live: &str, token: &str, lease: &HostLease) -> Result<HostMatch> {
    if lease.bots.is_empty() || !live_proto::game::valid_id(&lease.id) {
        bail!("this host lease has no simulation roster");
    }
    let agent = agent();
    let live = live_base(&agent, live)?;
    let url = format!("{live}/v1/hosts/{}/sim", lease.id);
    let matched: HostMatch = decode(authorized(agent.get(&url), token).call())
        .context("fetching the sim match from Live")?;
    if live_proto::game::sim_game_id(&matched.run_id) != lease.assignment_id
        || !live_proto::game::valid_id(&matched.batch_id)
    {
        bail!("Live returned a match for a different lease");
    }
    Ok(matched)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
}

fn live_base(agent: &ureq::Agent, live: &str) -> Result<String> {
    let parsed = agent.get(live).request_url().ok().context("invalid Live URL")?;
    let url = parsed.as_url();
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !live_proto::bearer_allowed(url.as_str())
    {
        bail!("Live needs an HTTPS or loopback HTTP URL without credentials, query or fragment");
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn admin_token(agent: &ureq::Agent, live: &str, tag: Option<&str>) -> Result<String> {
    let discovery: live_proto::Discovery =
        decode(agent.get(&format!("{live}/.well-known/h2live.json")).call())
            .context("discovering the Live server")?;
    let accounts = crate::live_accounts::load(&crate::live_accounts::accounts_path()?)?;
    let mut choices = accounts.iter().filter(|account| {
        account.is_on(live, &discovery.id)
            && tag.is_none_or(|tag| account.gamertag.eq_ignore_ascii_case(tag))
    });
    let account = choices
        .next()
        .context("no stored Live account matches; pass --token or select an admin with --as")?;
    if choices.next().is_some() {
        bail!("multiple stored Live accounts match; use --as GAMERTAG or --token");
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    Ok(live_proto::sign::issue_token(
        &account.key_pair()?,
        &crate::live_accounts::author_id(account.xuid, &discovery.id),
        &discovery.id,
        live_proto::lxm::GAMES_WRITE,
        now,
    ))
}

fn authorized(req: ureq::Request, token: &str) -> ureq::Request {
    req.set("Authorization", &format!("Bearer {token}"))
}

fn status(response: &Result<ureq::Response, ureq::Error>) -> Option<u16> {
    match response {
        Ok(response) => Some(response.status()),
        Err(ureq::Error::Status(code, _)) => Some(*code),
        Err(ureq::Error::Transport(_)) => None,
    }
}

fn decode<T: DeserializeOwned>(response: Result<ureq::Response, ureq::Error>) -> Result<T> {
    let response = match response {
        Ok(response) if (200..300).contains(&response.status()) => response,
        Ok(response) => bail!("unexpected HTTP {} from Live", response.status()),
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            match serde_json::from_str::<live_proto::ErrorBody>(&body) {
                Ok(body) => bail!("HTTP {code} from Live: {}", body.message),
                Err(_) => bail!("HTTP {code} from Live"),
            }
        }
        Err(ureq::Error::Transport(error)) => return Err(error).context("reaching Live"),
    };
    serde_json::from_reader(response.into_reader()).context("decoding Live's reply")
}
