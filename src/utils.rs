use anyhow::Result;
use serde_json::{json, Value};
use async_openai::{Client, types::{
    CreateChatCompletionRequestArgs,
    ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestUserMessageArgs,
}};
use std::env;
use tavily::{Tavily, SearchRequest};
use std::path::PathBuf;
use std::fs;
use std::process::Command;
use tokio::sync::broadcast;
use once_cell::sync::Lazy;

pub async fn call_llm(prompt: &str) -> Result<String> {
    // Backward-compatible simple wrapper
    let ctx = json!({"prompt_only": true});
    let v = call_role_llm("general", prompt, &ctx).await?;
    let s = v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string());
    Ok(s)
}

// -----------------------------
// Realtime transcript broadcast
// -----------------------------

static BROADCAST: Lazy<broadcast::Sender<Value>> = Lazy::new(|| {
    let (tx, _rx) = broadcast::channel(512);
    tx
});

pub fn subscribe_transcript() -> broadcast::Receiver<Value> {
    BROADCAST.subscribe()
}

pub fn publish_transcript(event: Value) {
    let _ = BROADCAST.send(event);
}

pub async fn tavily_search(query: &str, max_results: usize) -> Result<Value> {
    if let Ok(api_key) = env::var("TAVILY_API_KEY") {
        let tav = Tavily::new(&api_key);
        let mut req = SearchRequest::new(&api_key, query);
        req.search_depth("advanced");
        req.max_results(max_results as i32);
        // Some versions expose call(&request); fallback to search(query)
        let resp = match tav.call(&req).await {
            Ok(r) => Some(r),
            Err(_) => None,
        };
        // As a minimal bridge, return only query metadata; downstream LLM will fetch details if needed
        if resp.is_some() {
            return Ok(json!({
                "query": query,
                "note": "tavily response available (not serialized)",
                "max_results": max_results
            }));
        }
    }
    Ok(json!({
        "note": "tavily stubbed (set TAVILY_API_KEY)",
        "query": query,
        "results": []
    }))
}

pub async fn call_role_llm(role: &str, instruction: &str, context: &Value) -> Result<Value> {
    // async-openai reads OPENAI_API_KEY from env automatically.
    let client = Client::new();

    let system_prompt = format!(
        "You are the {} agent in a multi-agent research lab. Follow your role strictly. Use and update shared state when appropriate.",
        role
    );
    let user_prompt = format!(
        "Instruction:\n{}\n\nShared State (JSON):\n{}\n\nRespond with concise JSON representing your output.",
        instruction,
        context
    );

    let system_msg = ChatCompletionRequestMessage::System(
        ChatCompletionRequestSystemMessageArgs::default()
            .content(system_prompt)
            .build()?
    );
    let user_msg = ChatCompletionRequestMessage::User(
        ChatCompletionRequestUserMessageArgs::default()
            .content(user_prompt)
            .build()?
    );

    let req = CreateChatCompletionRequestArgs::default()
        .model("gpt-4")
        .messages([system_msg, user_msg])
        .temperature(0.2)
        .build()?;

    // If no API key set, async-openai will error; provide a graceful stub.
    let resp = match client.chat().create(req).await {
        Ok(r) => r,
        Err(_) => {
            return Ok(json!({
                "role": role,
                "note": "stubbed response (set OPENAI_API_KEY to enable real calls)",
                "result": instruction
            }))
        }
    };

    let content = resp
        .choices
        .get(0)
        .and_then(|c| c.message.content.as_ref())
        .map(|s| s.as_str())
        .unwrap_or("");

    // Try to parse JSON; if not JSON, return as string value
    let parsed: Result<Value, _> = serde_json::from_str(content);
    Ok(parsed.unwrap_or_else(|_| Value::String(content.to_string())))
}

pub fn detect_inter_agent_request(text: &str) -> Option<(String, String)> {
    // Very simple pattern: "role: <Role>, msg: <Message>"
    // This can be improved later.
    let lower = text.to_lowercase();
    if let Some(role_idx) = lower.find("role:") {
        if let Some(msg_idx) = lower.find(", msg:") {
            let role_part = text[role_idx + 5..msg_idx].trim();
            let msg_part = text[msg_idx + 6..].trim();
            if !role_part.is_empty() && !msg_part.is_empty() {
                return Some((role_part.to_string(), msg_part.to_string()));
            }
        }
    }
    None
}

pub fn ensure_dir(path: &PathBuf) -> std::io::Result<()> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    Ok(())
}

pub fn generate_pdf_report(html: &str, out_pdf: PathBuf) -> anyhow::Result<Value> {
    // Try wkhtmltopdf; if not present, save HTML alongside and return html path
    ensure_dir(&out_pdf).ok();
    // Write temp html
    let mut out_html = out_pdf.clone();
    out_html.set_extension("html");
    fs::write(&out_html, html)?;

    // Attempt wkhtmltopdf
    let status = Command::new("wkhtmltopdf")
        .arg(out_html.to_string_lossy().to_string())
        .arg(out_pdf.to_string_lossy().to_string())
        .status();
    if let Ok(st) = status {
        if st.success() {
            return Ok(json!({
                "artifact": out_pdf.to_string_lossy(),
                "kind": "pdf",
            }));
        }
    }
    Ok(json!({
        "artifact": out_html.to_string_lossy(),
        "kind": "html",
        "note": "wkhtmltopdf not available; saved HTML instead"
    }))
}
