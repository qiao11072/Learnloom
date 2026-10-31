use crate::{
    budget::Budget,
    config::{read_credential, Snapshot},
    Result,
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub struct ModelReply {
    pub content: String,
    pub usage: Value,
    pub model: String,
    pub config_revision: u64,
    pub elapsed_ms: u128,
}

static JOURNAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn deepseek(endpoint: &str) -> bool {
    reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .as_deref()
        == Some("api.deepseek.com")
}
fn journal_start(snapshot: &Snapshot) -> Result<Option<(String, String)>> {
    let Ok(path) = std::env::var("M0_REAL_JOURNAL") else {
        return Ok(None);
    };
    if !deepseek(&snapshot.endpoint) {
        return Ok(None);
    }
    let _lock = JOURNAL_LOCK.lock().map_err(|_| "journal lock")?;
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut starts = std::collections::BTreeMap::new();
    let mut spent = 0u64;
    for line in text.lines() {
        let v: Value = serde_json::from_str(line).map_err(|_| "invalid budget journal")?;
        match v["kind"].as_str() {
            Some("legacy") => spent += v["tokens"].as_u64().unwrap_or(0),
            Some("start") => {
                starts.insert(
                    v["id"].as_str().unwrap().to_string(),
                    v["reserved"].as_u64().unwrap(),
                );
            }
            Some("finish") => {
                starts.remove(v["id"].as_str().unwrap());
                spent += v["charged_tokens"].as_u64().unwrap();
            }
            _ => {}
        }
    }
    spent += starts.values().sum::<u64>();
    if spent + snapshot.context as u64 > 1_000_000 {
        return Err("TOKEN_BUDGET_EXCEEDED".into());
    }
    let id = crate::id();
    journal_append(
        &path,
        &json!({"kind":"start","id":id,"reserved":snapshot.context,"model":snapshot.model,"config_revision":snapshot.config_revision}),
    )?;
    Ok(Some((path, id)))
}
fn journal_append(path: &str, value: &Value) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{value}").map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}
fn journal_finish(
    journal: Option<(String, String)>,
    result: &Result<ModelReply>,
    observed: Option<Value>,
    reserved: usize,
) -> Result<()> {
    if let Some((path, id)) = journal {
        let _lock = JOURNAL_LOCK.lock().map_err(|_| "journal lock")?;
        let usage = result.as_ref().ok().map(|r| r.usage.clone()).or(observed);
        let charged = usage
            .as_ref()
            .and_then(|v| v["total_tokens"].as_u64())
            .unwrap_or(reserved as u64);
        journal_append(
            &path,
            &json!({"kind":"finish","id":id,"charged_tokens":charged,"actual_usage":usage,"status":if result.is_ok(){"response_received"}else{"failed"},"output":result.as_ref().ok().map(|r|&r.content),"error":result.as_ref().err()}),
        )?;
    }
    Ok(())
}

pub async fn generate(
    snapshot: &Snapshot,
    system: &str,
    task: &Value,
    timeout: Duration,
    cancel: CancellationToken,
) -> Result<ModelReply> {
    let budget = Budget::new(snapshot.context)?;
    budget.check(system, &task.to_string(), "", "")?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())?;
    let mut body = json!({"model":snapshot.model,"messages":[{"role":"system","content":system},{"role":"user","content":task.to_string()}],"max_tokens":budget.output,"response_format":{"type":"json_object"},"stream":false,"temperature":0});
    if deepseek(&snapshot.endpoint) {
        body["thinking"] = json!({"type":"disabled"});
    }
    let mut request = client
        .post(format!(
            "{}/chat/completions",
            snapshot.endpoint.trim_end_matches('/')
        ))
        .json(&body);
    if let Some(reference) = &snapshot.credential_ref {
        request = request.bearer_auth(read_credential(reference)?);
    }
    let journal = journal_start(snapshot)?;
    let mut observed_usage = None;
    let started = Instant::now();
    let work = async {
        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                "TIMEOUT".to_string()
            } else {
                "NETWORK_ERROR".to_string()
            }
        })?;
        if !response.status().is_success() {
            return Err(match response.status().as_u16() {
                401 | 403 => "AUTH_FAILED",
                404 => "MODEL_UNAVAILABLE",
                429 => "RATE_LIMITED",
                400 => "INVALID_REQUEST",
                _ => "PROVIDER_ERROR",
            }
            .into());
        }
        let value: Value = response
            .json()
            .await
            .map_err(|_| "INVALID_OUTPUT".to_string())?;
        observed_usage = Some(value["usage"].clone());
        if value["choices"][0]["finish_reason"].as_str() == Some("length") {
            return Err("INVALID_OUTPUT: truncated".into());
        }
        let content = value["choices"][0]["message"]["content"]
            .as_str()
            .ok_or("INVALID_OUTPUT")?
            .to_string();
        serde_json::from_str::<Value>(&content).map_err(|_| "INVALID_OUTPUT".to_string())?;
        Ok(ModelReply {
            content,
            usage: value["usage"].clone(),
            model: value["model"].as_str().unwrap_or(&snapshot.model).into(),
            config_revision: snapshot.config_revision,
            elapsed_ms: started.elapsed().as_millis(),
        })
    };
    let result =
        tokio::select! {biased; _=cancel.cancelled()=>Err("CANCELLED".into()),result=work=>result};
    journal_finish(journal, &result, observed_usage, snapshot.context)?;
    result
}

// M0 streaming protocol probe; it returns complete deltas, not a product chat service.
pub async fn stream(
    snapshot: &Snapshot,
    timeout: Duration,
    cancel: CancellationToken,
) -> Result<Vec<String>> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())?;
    let work = async {
        let response=client.post(format!("{}/chat/completions",snapshot.endpoint.trim_end_matches('/'))).json(&json!({"model":snapshot.model,"messages":[{"role":"user","content":"M0 stream"}],"stream":true})).send().await.map_err(|_|"NETWORK_ERROR")?;
        if !response.status().is_success() {
            return Err("PROVIDER_ERROR".into());
        }
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::<u8>::new();
        let mut deltas = vec![];
        let mut done = false;
        while let Some(bytes) = stream.next().await {
            buffer.extend_from_slice(&bytes.map_err(|_| "STREAM_INTERRUPTED")?);
            while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
                let line = std::str::from_utf8(&buffer[..pos])
                    .map_err(|_| "INVALID_UTF8")?
                    .trim()
                    .to_string();
                buffer.drain(..=pos);
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        done = true;
                    } else {
                        let value: Value =
                            serde_json::from_str(data).map_err(|_| "INVALID_OUTPUT")?;
                        if let Some(text) = value["choices"][0]["delta"]["content"].as_str() {
                            deltas.push(text.to_string());
                        }
                    }
                }
            }
        }
        if !done {
            return Err("STREAM_INTERRUPTED".into());
        }
        Ok(deltas)
    };
    tokio::select! {biased; _=cancel.cancelled()=>Err("CANCELLED".into()),result=work=>result}
}

/// Production SSE gateway. Every visible content fragment is accepted by a durable callback.
/// Completion requires both a provider stop marker and [DONE]; transport EOF is never success.
pub async fn stream_chat<F, Fut>(
    snapshot: &Snapshot,
    system: &str,
    task: &Value,
    timeout: Duration,
    cancel: CancellationToken,
    mut on_content: F,
) -> Result<ModelReply>
where
    F: FnMut(String) -> Fut + Send,
    Fut: std::future::Future<Output = Result<()>> + Send,
{
    let budget = Budget::new(snapshot.context)?;
    budget.check(system, &task.to_string(), "", "")?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
        .map_err(|_| "NETWORK_ERROR")?;
    let mut body = json!({"model":snapshot.model,"messages":[{"role":"system","content":system},{"role":"user","content":task.to_string()}],"max_tokens":budget.output,"response_format":{"type":"json_object"},"stream":true,"stream_options":{"include_usage":true},"temperature":0});
    if deepseek(&snapshot.endpoint) {
        body["thinking"] = json!({"type":"disabled"});
    }
    let mut request = client
        .post(format!(
            "{}/chat/completions",
            snapshot.endpoint.trim_end_matches('/')
        ))
        .json(&body);
    if let Some(reference) = &snapshot.credential_ref {
        request = request.bearer_auth(read_credential(reference)?);
    }
    let journal = journal_start(snapshot)?;
    let started = Instant::now();
    let mut observed = None;
    let work = async {
        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                "TIMEOUT"
            } else {
                "NETWORK_ERROR"
            }
        })?;
        if !response.status().is_success() {
            let code = match response.status().as_u16() {
                401 | 403 => "AUTH_FAILED",
                404 => "MODEL_UNAVAILABLE",
                429 => "RATE_LIMITED",
                400 => "INVALID_REQUEST",
                _ => "PROVIDER_ERROR",
            };
            return Err(if code == "RATE_LIMITED" {
                match response
                    .headers()
                    .get("retry-after")
                    .and_then(|s| s.to_str().ok())
                {
                    Some(s) => {
                        if let Ok(n) = s.parse::<u64>() {
                            format!("RATE_LIMITED: wait={n}")
                        } else {
                            "RATE_LIMITED: retry_disabled".into()
                        }
                    }
                    None => "RATE_LIMITED: wait=1".into(),
                }
            } else {
                code.into()
            });
        }
        let mut stream = response.bytes_stream();
        let mut buffer = vec![];
        let mut lines = vec![];
        let mut content = String::new();
        let mut stopped = false;
        let mut model = snapshot.model.clone();
        let mut done = false;
        let mut flush = tokio::time::interval(Duration::from_millis(100));
        'network: loop {
            let bytes = tokio::select! {_=flush.tick()=>{on_content(String::new()).await?;continue;},bytes=stream.next()=>bytes};
            let Some(bytes) = bytes else { break };
            let bytes = bytes.map_err(|e| {
                if e.is_timeout() {
                    "TIMEOUT"
                } else {
                    "STREAM_INTERRUPTED"
                }
            })?;
            buffer.extend_from_slice(&bytes);
            if buffer.len() > 262144 {
                return Err("INVALID_OUTPUT: oversized SSE frame".into());
            }
            while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
                let line = std::str::from_utf8(&buffer[..pos])
                    .map_err(|_| "INVALID_OUTPUT: invalid UTF-8")?
                    .trim_end_matches('\r')
                    .to_owned();
                buffer.drain(..=pos);
                if !line.is_empty() {
                    if let Some(data) = line.strip_prefix("data:") {
                        lines.push(data.trim_start_matches(' ').to_owned());
                    }
                    continue;
                }
                if lines.is_empty() {
                    continue;
                }
                let data = lines.join("\n");
                lines.clear();
                if data == "[DONE]" {
                    done = true;
                    break 'network;
                }
                let value: Value = serde_json::from_str(&data)
                    .map_err(|_| "INVALID_OUTPUT: malformed SSE JSON")?;
                if value.get("error").is_some() {
                    return Err("PROVIDER_ERROR".into());
                }
                let previously_stopped = stopped;
                if value["usage"].is_object() {
                    observed = Some(value["usage"].clone());
                }
                if let Some(name) = value["model"].as_str() {
                    model = name.into();
                }
                if let Some(reason) = value["choices"][0]["finish_reason"].as_str() {
                    if reason != "stop" {
                        return Err(format!("INVALID_OUTPUT: finish_reason {reason}"));
                    }
                    stopped = true;
                }
                if value["choices"][0]["delta"]["tool_calls"].is_array() {
                    return Err("INVALID_OUTPUT: unexpected tool call".into());
                }
                if let Some(delta) = value["choices"][0]["delta"]["content"].as_str() {
                    if previously_stopped && !delta.is_empty() {
                        return Err("INVALID_OUTPUT: content after finish".into());
                    }
                    content.push_str(delta);
                    if content.len() > 131072 {
                        return Err("INVALID_OUTPUT: oversized answer".into());
                    }
                    if !delta.is_empty() {
                        on_content(delta.into()).await?;
                    }
                }
            }
        }
        if !done || !stopped {
            return Err("STREAM_INTERRUPTED".into());
        }
        serde_json::from_str::<Value>(&content).map_err(|_| "INVALID_OUTPUT")?;
        Ok(ModelReply {
            content,
            usage: observed.clone().unwrap_or(Value::Null),
            model,
            config_revision: snapshot.config_revision,
            elapsed_ms: started.elapsed().as_millis(),
        })
    };
    let result = tokio::select! {biased;_=cancel.cancelled()=>Err("CANCELLED".into()),r=work=>r};
    journal_finish(journal, &result, observed, snapshot.context)?;
    result
}
