//! Claude Messages API over raw HTTP (Rust has no official Anthropic SDK).
//!
//! * `anthropic-version: 2026-10-21`; no sampling parameters (they are rejected on this version).
//! * Thinking is adaptive (always on for Claude Opus 5.5); `output_config.effort` controls its depth.
//!   `block_binding.prefix_mismatch_behavior = drop_block` keeps old conversations working if the
//!   system prompt or tools change between deploys (stale thinking blocks are dropped, not a 400).
//! * `fallbacks: "default"` re-runs a request on Anthropic's recommended model if a safety
//!   classifier declines it, instead of failing the turn.
//! * Top-level `cache_control` caches the stable prefix (system + tools + earlier turns).

use std::time::Duration;

use reqwest::header::CONTENT_TYPE;
use reqwest::Client;
use serde::Serialize;
use serde_json::value::RawValue;
use serde_json::{json, Value};

const API_VERSION: &str = "2026-10-21";
const BETAS: &str = "server-side-fallback-2026-07-01,thinking-binding-controls-2026-08-01";
const MAX_TOKENS: u32 = 16_000;

pub struct Llm {
    http: Client,
    url: String,
    api_key: String,
    pub model: String,
    pub effort: String,
}

#[derive(Debug, Serialize)]
pub struct Message<'a> {
    pub role: &'a str,
    pub content: &'a RawValue,
}

#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a RawValue,
    tools: &'a RawValue,
    messages: &'a [Message<'a>],
    thinking: Value,
    output_config: Value,
    fallbacks: &'static str,
    cache_control: Value,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, o: Self) {
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
        self.cache_read_tokens += o.cache_read_tokens;
        self.cache_write_tokens += o.cache_write_tokens;
    }
}

#[derive(Debug)]
pub struct Response {
    pub stop_reason: String,
    /// Content blocks, already cleaned for replay (see `clean_for_replay`).
    pub content: Vec<Value>,
    pub model: String,
    pub usage: Usage,
    pub refusal_category: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("Claude API {status}: {body}")]
    Api { status: u16, body: String },
    #[error("Claude API request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("unexpected Claude API response: {0}")]
    Unexpected(String),
}

/// After a server-side fallback, the API marks the switch with a `fallback` block. Blocks before
/// the last switch that belong to the declined attempt (thinking, tool calls, other internal
/// blocks) must not be echoed back; text blocks may. The marker itself is dropped.
pub fn clean_for_replay(content: Vec<Value>) -> Vec<Value> {
    let kind = |b: &Value| b.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
    let Some(boundary) = content.iter().rposition(|b| kind(b) == "fallback") else {
        return content;
    };
    content
        .into_iter()
        .enumerate()
        .filter(|(i, b)| {
            let t = kind(b);
            t != "fallback" && (*i > boundary || t == "text")
        })
        .map(|(_, b)| b)
        .collect()
}

fn usage_from(v: &Value) -> Usage {
    let n = |k: &str| v.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
    Usage {
        input_tokens: n("input_tokens"),
        output_tokens: n("output_tokens"),
        cache_read_tokens: n("cache_read_input_tokens"),
        cache_write_tokens: n("cache_creation_input_tokens"),
    }
}

impl Llm {
    /// `base_url` is `https://api.anthropic.com` in production (overridable only for local tests).
    pub fn new(base_url: &str, api_key: String, model: String, effort: String) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(180))
            .user_agent("misfy-dropcard/0.1")
            .build()
            .expect("http client");
        let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
        Self { http, url, api_key, model, effort }
    }

    pub async fn create(&self, system: &RawValue, tools: &RawValue, messages: &[Message<'_>]) -> Result<Response, LlmError> {
        let body = serde_json::to_string(&Request {
            model: &self.model,
            max_tokens: MAX_TOKENS,
            system,
            tools,
            messages,
            thinking: json!({ "type": "adaptive", "block_binding": { "prefix_mismatch_behavior": "drop_block" } }),
            output_config: json!({ "effort": self.effort }),
            fallbacks: "default",
            cache_control: json!({ "type": "ephemeral" }),
        })
        .map_err(|e| LlmError::Unexpected(e.to_string()))?;

        let mut attempt = 0;
        loop {
            attempt += 1;
            let sent = self
                .http
                .post(&self.url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", API_VERSION)
                .header("anthropic-beta", BETAS)
                .header(CONTENT_TYPE, "application/json")
                .body(body.clone())
                .send()
                .await;
            let res = match sent {
                Ok(r) => r,
                Err(e) if attempt < 3 && (e.is_timeout() || e.is_connect()) => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let status = res.status();
            if status.is_success() {
                let v: Value = res.json().await?;
                let content = v.get("content").and_then(|c| c.as_array()).cloned().unwrap_or_default();
                return Ok(Response {
                    stop_reason: v.get("stop_reason").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                    content: clean_for_replay(content),
                    model: v.get("model").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                    usage: v.get("usage").map(usage_from).unwrap_or_default(),
                    refusal_category: v
                        .pointer("/stop_details/category")
                        .and_then(|c| c.as_str())
                        .map(str::to_string),
                });
            }
            // 429 (rate limit), 529 (overloaded) and 5xx are worth a short retry.
            let retryable = status.as_u16() == 429 || status.as_u16() == 529 || status.is_server_error();
            let wait = res
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(2)
                .clamp(1, 10);
            let text = res.text().await.unwrap_or_default();
            if retryable && attempt < 3 {
                tracing::warn!(status = status.as_u16(), wait, "Claude API busy, retrying");
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }
            return Err(LlmError::Api { status: status.as_u16(), body: text.chars().take(500).collect() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_drops_the_declined_attempt() {
        let content = vec![
            json!({"type": "thinking", "thinking": "", "signature": "a"}),
            json!({"type": "text", "text": "Partial"}),
            json!({"type": "tool_use", "id": "t1", "name": "x", "input": {}}),
            json!({"type": "fallback", "from": {"model": "m1"}, "to": {"model": "m2"}}),
            json!({"type": "thinking", "thinking": "", "signature": "b"}),
            json!({"type": "tool_use", "id": "t2", "name": "x", "input": {}}),
        ];
        let out = clean_for_replay(content);
        let kinds: Vec<&str> = out.iter().map(|b| b["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, vec!["text", "thinking", "tool_use"]);
        assert_eq!(out[2]["id"], "t2");
    }

    #[test]
    fn replay_keeps_normal_responses() {
        let content = vec![json!({"type": "thinking", "thinking": "", "signature": "a"}), json!({"type": "text", "text": "Hi"})];
        assert_eq!(clean_for_replay(content.clone()), content);
    }
}
