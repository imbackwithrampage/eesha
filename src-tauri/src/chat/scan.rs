// Finding the model servers already running on this computer, so the chat can
// offer them with nothing to type and no key: Ollama, LM Studio, llama.cpp,
// vLLM and Jan, each on the port it listens on unless told otherwise.
//
// Only 127.0.0.1 is asked, one `GET /v1/models` per port, with no key, no
// redirect followed and a second and a half to answer. It runs when the user
// opens the model picker or Settings → Chat, never by itself.
//
// The list of servers and ports is upstream pull request 400's
// (`CustomProviders.swift`).

use std::time::Duration;

use notch_core::net;
use reqwest::Url;
use serde::Serialize;
use serde_json::Value;

/// (the provider it connects as, its name, its usual port). Ollama and LM
/// Studio have their own entry in Settings; the others share the one for any
/// OpenAI-compatible server.
const USUAL: &[(&str, &str, u16)] = &[
    ("ollama", "Ollama", 11434),
    ("lmstudio", "LM Studio", 1234),
    ("custom", "llama.cpp", 8080),
    ("custom", "vLLM", 8000),
    ("custom", "Jan", 1337),
];

const PATIENCE: Duration = Duration::from_millis(1500);
/// A model list is small; a port that answers with more is something else.
const MAX_LIST: usize = 1024 * 1024;
/// Models that embed or rank rather than chat are left out of the list.
const NOT_CHAT: &[&str] = &["embed", "bge-", "all-minilm", "clip", "rerank"];

#[derive(Debug, Clone, Serialize)]
pub struct Found {
    /// "ollama", "lmstudio" or "custom": the provider this server connects as.
    pub id: &'static str,
    pub name: &'static str,
    pub url: String,
    /// Its chat models; none yet when nothing was downloaded.
    pub models: Vec<String>,
}

/// The servers answering right now, in the order of `USUAL`.
pub async fn scan() -> Vec<Found> {
    let probes: Vec<_> = USUAL.iter().map(|&(id, name, port)| tokio::spawn(probe(id, name, port))).collect();
    let mut found = Vec::new();
    for probe in probes {
        if let Ok(Some(server)) = probe.await {
            found.push(server);
        }
    }
    found
}

async fn probe(id: &'static str, name: &'static str, port: u16) -> Option<Found> {
    let url = Url::parse(&format!("http://127.0.0.1:{port}")).ok()?;
    // The client made for keys, though none is sent: it is the one that only
    // talks to this machine over http and follows no redirect elsewhere.
    let response = net::keyed_client(&url, PATIENCE).ok()?.get(net::join(&url, "v1/models")).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = net::read_capped(response, MAX_LIST).await.ok()?;
    let models = model_ids(&serde_json::from_slice(&body).ok()?)?;
    Some(Found { id, name, url: url.as_str().trim_end_matches('/').to_string(), models })
}

/// The chat models of an OpenAI-style `/v1/models` answer; None for anything
/// that is not one (some other program on that port).
pub fn model_ids(body: &Value) -> Option<Vec<String>> {
    let items = body.get("data")?.as_array()?;
    Some(
        items
            .iter()
            .filter_map(|m| m.get("id").and_then(Value::as_str))
            .filter(|id| !NOT_CHAT.iter().any(|n| id.to_lowercase().contains(n)))
            .map(str::to_string)
            .collect(),
    )
}
