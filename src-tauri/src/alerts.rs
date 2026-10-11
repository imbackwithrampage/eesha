// The pure half of the developer alerts (GitHub and Vercel notices): the
// "new since start, once only" rule, the watch list, the back-off. No network
// and no Tauri here, so src-tauri/examples/alerts-probe.rs compiles this very
// file and runs it over made-up poll results.
//
// The idea of the alerts (a Vercel deploy turning Ready or Failed, never for the
// deploys on screen at launch; a watch list of up to 20 names) comes from
// upstream pull requests 141 and 281; the code is written here.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::Value;

/// What the island turns into a notice. `kind` is a code the island words in
/// the interface language; `key` is what makes it once only.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Alert {
    pub key: String,
    pub kind: &'static str,
    pub bad: bool,
    pub subject: String,
    pub detail: String,
    pub url: String,
}

/// Keys already alerted. Cleared past 500 entries (a restart forgets them too:
/// an alert is only ever raised on a change, never on what was there).
#[derive(Default)]
pub struct Dedupe(HashSet<String>);

impl Dedupe {
    pub fn first(&mut self, key: &str) -> bool {
        if self.0.len() > 500 {
            self.0.clear();
        }
        self.0.insert(key.to_string())
    }
}

// ── Watch list ────────────────────────────────────────────────────────────────

pub const MAX_WATCH: usize = 20;

fn name_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && !s.starts_with('.') && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// A GitHub repository as "owner/name" (each part checked, so it can go into a
/// GraphQL query as it is).
pub fn repo_ok(s: &str) -> bool {
    s.split_once('/').is_some_and(|(owner, name)| name_ok(owner) && name_ok(name))
}

/// A watch list as Settings stored it: strings that pass `ok`, no duplicate
/// (any case), the first 20.
pub fn parse_watch(raw: Option<&Value>, ok: fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in raw.and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        let Some(s) = item.as_str().map(str::trim) else { continue };
        if ok(s) && !out.iter().any(|x| x.eq_ignore_ascii_case(s)) && out.len() < MAX_WATCH {
            out.push(s.to_string());
        }
    }
    out
}

/// An empty list watches everything of the user's own.
pub fn watched(list: &[String], name: &str) -> bool {
    list.is_empty() || list.iter().any(|x| x.eq_ignore_ascii_case(name))
}

// ── Back-off ──────────────────────────────────────────────────────────────────

/// When a service may be asked again. Errors double the wait (30 min at most);
/// a rate limit waits for the time the service names.
#[derive(Default)]
pub struct Backoff {
    fails: u32,
    until_ms: u64,
}

impl Backoff {
    pub fn blocked(&self, now_ms: u64) -> bool {
        now_ms < self.until_ms
    }

    pub fn ok(&mut self) {
        self.fails = 0;
    }

    pub fn fail(&mut self, now_ms: u64, base_secs: u64) {
        self.fails = (self.fails + 1).min(8);
        let wait = (base_secs << (self.fails - 1)).min(1800);
        self.until_ms = self.until_ms.max(now_ms + wait * 1000);
    }

    /// `reset_at_secs`: the Unix time of `X-RateLimit-Reset`; `retry_after`: seconds.
    pub fn limited(&mut self, now_ms: u64, reset_at_secs: Option<u64>, retry_after: Option<u64>) {
        let wait = retry_after
            .or(reset_at_secs.map(|r| r.saturating_sub(now_ms / 1000) + 1))
            .unwrap_or(60)
            .clamp(1, 3600);
        self.until_ms = self.until_ms.max(now_ms + wait * 1000);
    }
}

// ── Vercel ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Dep {
    pub id: String,
    pub project: String,
    /// BUILDING, QUEUED, INITIALIZING, READY, ERROR, CANCELED
    pub state: String,
    pub created_ms: u64,
    pub host: String,
    pub detail: String,
}

fn terminal(state: &str) -> bool {
    matches!(state, "READY" | "ERROR" | "CANCELED")
}

/// The deployments by id and state as of the last poll; None before the first.
pub type VercelSeen = Option<HashMap<String, String>>;

/// Alerts for the deployments that turned Ready or Failed since the last poll.
/// The first poll only fills `seen`. A deployment that is new since the app
/// started and already done at its first sight (it built between two polls)
/// counts; one that was there before the start never does.
pub fn vercel_alerts(
    seen: &mut VercelSeen,
    deps: &[Dep],
    watch: &[String],
    started_ms: u64,
    dedupe: &mut Dedupe,
) -> Vec<Alert> {
    let deps: Vec<&Dep> = deps.iter().filter(|d| watched(watch, &d.project)).collect();
    let now: HashMap<String, String> = deps.iter().map(|d| (d.id.clone(), d.state.clone())).collect();
    let Some(before) = seen.replace(now) else { return Vec::new() };

    let mut out = Vec::new();
    for d in deps {
        let turned = match before.get(&d.id) {
            Some(prev) => !terminal(prev) && prev != &d.state,
            None => d.created_ms >= started_ms,
        };
        let ready = d.state == "READY";
        if !turned || !(ready || d.state == "ERROR") {
            continue;
        }
        let key = format!("vercel:{}:{}", d.id, d.state);
        if dedupe.first(&key) {
            out.push(Alert {
                key,
                kind: if ready { "deployReady" } else { "deployFailed" },
                bad: !ready,
                subject: d.project.clone(),
                detail: d.detail.clone(),
                url: if name_ok(&d.host.replace('.', "-")) { format!("https://{}", d.host) } else { String::new() },
            });
        }
    }
    out
}
