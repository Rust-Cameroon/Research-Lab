use anyhow::Result;
use async_trait::async_trait;
use pocketflow_rs::{Context, Node, ProcessResult};
use serde_json::{json, Value};
use tokio::io::{self, AsyncBufReadExt};

use crate::state::MyState;
use crate::utils::{call_llm, call_role_llm, detect_inter_agent_request, tavily_search, generate_pdf_report, publish_transcript};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct GetQuestionNode;

#[async_trait]
impl Node for GetQuestionNode {
    type State = MyState;

    async fn execute(&self, _context: &Context) -> Result<serde_json::Value> {
        println!("Enter your question: ");
        let mut reader = io::BufReader::new(tokio::io::stdin());
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let question = line.trim().to_string();
        Ok(json!(question))
    }

    async fn post_process(
        &self,
        context: &mut Context,
        result: &Result<serde_json::Value>,
    ) -> Result<ProcessResult<MyState>> {
        if let Ok(val) = result {
            context.set("question", val.clone());
            append_transcript(context, "User", "question");
            Ok(ProcessResult::new(MyState::Success, "success".to_string()))
        } else {
            Ok(ProcessResult::new(MyState::Failure, "failure".to_string()))
        }
    }
}

pub struct FinalReportNode;

#[async_trait]
impl Node for FinalReportNode {
    type State = MyState;

    async fn execute(&self, context: &Context) -> Result<serde_json::Value> {
        // Assemble simple HTML from shared state
        let goal = context.get("goal").cloned().unwrap_or(json!({}));
        let claims = context.get("claims").cloned().unwrap_or(json!({}));
        let critic = context.get("critic_review").cloned().unwrap_or(json!({}));
        let plan = context.get("plan").cloned().unwrap_or(json!({}));
        let stat = context.get("statistical_approval").cloned().unwrap_or(json!({}));
        let ethics = context.get("ethics_approval").cloned().unwrap_or(json!({}));
        let analysis = context.get("analysis_result").cloned().unwrap_or(json!({}));
        let synth = context.get("pi_synthesis").cloned().unwrap_or(json!({}));
        let report = context.get("report_draft").cloned().unwrap_or(json!({}));
        let transcript = context.get("transcript").cloned().unwrap_or(json!([]));

        let html = format!(r#"<!DOCTYPE html>
<html><head><meta charset='utf-8'><title>Final Survey Report</title>
<style>body{{font-family:sans-serif;}} h1,h2{{margin-top:1em;}} pre{{white-space:pre-wrap;}}</style>
</head><body>
<h1>Final Survey Report</h1>
<h2>Goal</h2><pre>{}</pre>
<h2>Claims</h2><pre>{}</pre>
<h2>Critic Review</h2><pre>{}</pre>
<h2>Plan</h2><pre>{}</pre>
<h2>Approvals</h2><pre>Statistical: {}\nEthics: {}</pre>
<h2>Analysis</h2><pre>{}</pre>
<h2>PI Synthesis</h2><pre>{}</pre>
<h2>Postdoc Draft</h2><pre>{}</pre>
<h2>Transcript</h2><pre>{}</pre>
</body></html>"#,
            goal, claims, critic, plan, stat, ethics, analysis, synth, report, transcript
        );

        // Output path
        let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let out = PathBuf::from("data").join("artifacts").join("reports").join(format!("final_{}.pdf", ts));
        let artifact = generate_pdf_report(&html, out)?;
        Ok(artifact)
    }

    async fn post_process(
        &self,
        context: &mut Context,
        result: &Result<serde_json::Value>,
    ) -> Result<ProcessResult<MyState>> {
        if let Ok(val) = result {
            context.set("final_report", val.clone());
            append_transcript(context, "PI", "final_report");
            Ok(ProcessResult::new(MyState::Success, "success".to_string()))
        } else {
            Ok(ProcessResult::new(MyState::Failure, "failure".to_string()))
        }
    }
}

pub struct AnswerNode;

#[async_trait]
impl Node for AnswerNode {
    type State = MyState;

    async fn execute(&self, context: &Context) -> Result<serde_json::Value> {
        let question = context
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let answer = call_llm(&question).await?;
        Ok(json!(answer))
    }

    async fn post_process(
        &self,
        context: &mut Context,
        result: &Result<serde_json::Value>,
    ) -> Result<ProcessResult<MyState>> {
        if let Ok(val) = result {
            context.set("answer", val.clone());
            append_transcript(context, "Answer", "answer");
            Ok(ProcessResult::new(MyState::Success, "success".to_string()))
        } else {
            Ok(ProcessResult::new(MyState::Failure, "failure".to_string()))
        }
    }
}

// ------------------------------------------------------------
// Agent Role Nodes (MVP stub implementations)
// ------------------------------------------------------------

pub struct GoalNode;

#[async_trait]
impl Node for GoalNode {
    type State = MyState;

    async fn execute(&self, context: &Context) -> Result<serde_json::Value> {
        let ctx = context_snapshot(context);
        let v = call_role_llm(
            "PI",
            "Define or refine the current research goal. Return JSON: { title, rationale }.",
            &ctx,
        )
        .await?;
        Ok(v)
    }

    async fn post_process(
        &self,
        context: &mut Context,
        result: &Result<serde_json::Value>,
    ) -> Result<ProcessResult<MyState>> {
        if let Ok(val) = result {
            context.set("goal", val.clone());
            append_transcript(context, "PI", "goal");
            Ok(ProcessResult::new(MyState::Success, "success".to_string()))
        } else {
            Ok(ProcessResult::new(MyState::Failure, "failure".to_string()))
        }
    }
}

pub struct ReaderNode;

#[async_trait]
impl Node for ReaderNode {
    type State = MyState;

    async fn execute(&self, context: &Context) -> Result<serde_json::Value> {
        // Build base context and enhance with Tavily search results using goal/topic
        let mut ctx = context_snapshot(context);
        let goal_title = context
            .get("goal")
            .and_then(|v| v.get("title"))
            .and_then(|v| v.as_str())
            .unwrap_or("current research topic");
        let tav = tavily_search(goal_title, 5).await.unwrap_or_else(|_| json!({"results": []}));
        if let Some(obj) = ctx.as_object_mut() {
            obj.insert("tavily".to_string(), tav);
        }

        let v = call_role_llm(
            "Reader",
            "Using the provided Tavily search results, survey literature and extract 2 concise claims. Each claim must include evidence with url and snippet. Return strict JSON: { claims: [{ id, text, evidence: { url, snippet } }] }.",
            &ctx,
        )
        .await?;
        // Simple inter-agent dispatch if a plain string ask is returned
        if let Some(s) = v.as_str() {
            if let Some((role, msg)) = detect_inter_agent_request(s) {
                let v2 = call_role_llm(&role, &msg, &ctx).await?;
                return Ok(v2);
            }
        }
        Ok(v)
    }

    async fn post_process(
        &self,
        context: &mut Context,
        result: &Result<serde_json::Value>,
    ) -> Result<ProcessResult<MyState>> {
        if let Ok(val) = result {
            let mut normalized = val.clone();
            if let Some(obj) = normalized.as_object_mut() {
                if let Some(claims) = obj.get_mut("claims") {
                    if let Some(arr) = claims.as_array_mut() {
                        for c in arr.iter_mut() {
                            if let Some(cobj) = c.as_object_mut() {
                                let ev = cobj.entry("evidence").or_insert(json!({}));
                                if let Some(eobj) = ev.as_object_mut() {
                                    if !eobj.contains_key("url") { eobj.insert("url".to_string(), json!("")); }
                                    if !eobj.contains_key("snippet") { eobj.insert("snippet".to_string(), json!("")); }
                                } else {
                                    *ev = json!({"url": "", "snippet": ""});
                                }
                            }
                        }
                    }
                }
            }
            context.set("claims", normalized);
            append_transcript(context, "Reader", "claims");
            Ok(ProcessResult::new(MyState::Success, "success".to_string()))
        } else {
            Ok(ProcessResult::new(MyState::Failure, "failure".to_string()))
        }
    }
}

pub struct CriticNode;

#[async_trait]
impl Node for CriticNode {
    type State = MyState;

    async fn execute(&self, context: &Context) -> Result<serde_json::Value> {
        let ctx = context_snapshot(context);
        let v = call_role_llm(
            "Critic",
            "Evaluate current claims. Flag confounders and request verifications. Return JSON: { review, actions?: [...] }.",
            &ctx,
        )
        .await?;
        if let Some(s) = v.as_str() {
            if let Some((role, msg)) = detect_inter_agent_request(s) {
                let v2 = call_role_llm(&role, &msg, &ctx).await?;
                return Ok(v2);
            }
        }
        Ok(v)
    }

    async fn post_process(
        &self,
        context: &mut Context,
        result: &Result<serde_json::Value>,
    ) -> Result<ProcessResult<MyState>> {
        if let Ok(val) = result {
            context.set("critic_review", val.clone());
            append_transcript(context, "Critic", "critic_review");
            Ok(ProcessResult::new(MyState::Success, "success".to_string()))
        } else {
            Ok(ProcessResult::new(MyState::Failure, "failure".to_string()))
        }
    }
}

pub struct PlannerNode;

#[async_trait]
impl Node for PlannerNode {
    type State = MyState;

    async fn execute(&self, context: &Context) -> Result<serde_json::Value> {
        let ctx = context_snapshot(context);
        let v = call_role_llm(
            "Planner",
            "Draft an experiment plan with sample size/power. Return JSON: { plan_id, summary, params }.",
            &ctx,
        )
        .await?;
        Ok(v)
    }

    async fn post_process(
        &self,
        context: &mut Context,
        result: &Result<serde_json::Value>,
    ) -> Result<ProcessResult<MyState>> {
        if let Ok(val) = result {
            context.set("plan", val.clone());
            append_transcript(context, "Planner", "plan");
            Ok(ProcessResult::new(MyState::Success, "success".to_string()))
        } else {
            Ok(ProcessResult::new(MyState::Failure, "failure".to_string()))
        }
    }
}

pub struct StatisticianApprovalNode;

#[async_trait]
impl Node for StatisticianApprovalNode {
    type State = MyState;

    async fn execute(&self, context: &Context) -> Result<serde_json::Value> {
        let ctx = context_snapshot(context);
        let v = call_role_llm(
            "Statistician",
            "Assess plan adequacy/power. Return JSON: { statistical_approval: true|false, notes }.",
            &ctx,
        )
        .await?;
        Ok(v)
    }

    async fn post_process(
        &self,
        context: &mut Context,
        result: &Result<serde_json::Value>,
    ) -> Result<ProcessResult<MyState>> {
        if let Ok(val) = result {
            context.set("statistical_approval", val.clone());
            append_transcript(context, "Statistician", "statistical_approval");
            Ok(ProcessResult::new(MyState::Success, "success".to_string()))
        } else {
            Ok(ProcessResult::new(MyState::Failure, "failure".to_string()))
        }
    }
}

pub struct EthicsApprovalNode;

#[async_trait]
impl Node for EthicsApprovalNode {
    type State = MyState;

    async fn execute(&self, context: &Context) -> Result<serde_json::Value> {
        let ctx = context_snapshot(context);
        let v = call_role_llm(
            "Ethics",
            "Review compliance and risks. Return JSON: { ethics_approval: true|false, restrictions?: [...] }.",
            &ctx,
        )
        .await?;
        Ok(v)
    }

    async fn post_process(
        &self,
        context: &mut Context,
        result: &Result<serde_json::Value>,
    ) -> Result<ProcessResult<MyState>> {
        if let Ok(val) = result {
            context.set("ethics_approval", val.clone());
            append_transcript(context, "Ethics", "ethics_approval");
            Ok(ProcessResult::new(MyState::Success, "success".to_string()))
        } else {
            Ok(ProcessResult::new(MyState::Failure, "failure".to_string()))
        }
    }
}

pub struct AnalystNode;

#[async_trait]
impl Node for AnalystNode {
    type State = MyState;

    async fn execute(&self, context: &Context) -> Result<serde_json::Value> {
        // Gate on approvals
        let stat_ok = context.get("statistical_approval").is_some();
        let eth_ok = context.get("ethics_approval").is_some();
        if !(stat_ok && eth_ok) {
            return Ok(json!({"blocked": "approvals_missing"}));
        }
        let ctx = context_snapshot(context);
        let v = call_role_llm(
            "Analyst",
            "Run the planned analysis. Return JSON: { analysis, artifacts: [paths] }.",
            &ctx,
        )
        .await?;
        Ok(v)
    }

    async fn post_process(
        &self,
        context: &mut Context,
        result: &Result<serde_json::Value>,
    ) -> Result<ProcessResult<MyState>> {
        if let Ok(val) = result {
            if val.get("blocked").is_some() {
                return Ok(ProcessResult::new(MyState::Failure, "failure".to_string()));
            }
            context.set("analysis_result", val.clone());
            append_transcript(context, "Analyst", "analysis_result");
            Ok(ProcessResult::new(MyState::Success, "success".to_string()))
        } else {
            Ok(ProcessResult::new(MyState::Failure, "failure".to_string()))
        }
    }
}

pub struct PISynthesisNode;

#[async_trait]
impl Node for PISynthesisNode {
    type State = MyState;

    async fn execute(&self, context: &Context) -> Result<serde_json::Value> {
        let ctx = context_snapshot(context);
        let v = call_role_llm(
            "PI",
            "Synthesize results into conclusions and next steps. Return JSON: { synthesis, next_steps }.",
            &ctx,
        )
        .await?;
        Ok(v)
    }

    async fn post_process(
        &self,
        context: &mut Context,
        result: &Result<serde_json::Value>,
    ) -> Result<ProcessResult<MyState>> {
        if let Ok(val) = result {
            context.set("pi_synthesis", val.clone());
            append_transcript(context, "PI", "pi_synthesis");
            Ok(ProcessResult::new(MyState::Success, "success".to_string()))
        } else {
            Ok(ProcessResult::new(MyState::Failure, "failure".to_string()))
        }
    }
}

pub struct PostdocReportNode;

#[async_trait]
impl Node for PostdocReportNode {
    type State = MyState;

    async fn execute(&self, context: &Context) -> Result<serde_json::Value> {
        let ctx = context_snapshot(context);
        let v = call_role_llm(
            "Postdoc",
            "Draft a brief report based on PI synthesis. Return JSON: { report, sections }.",
            &ctx,
        )
        .await?;
        Ok(v)
    }

    async fn post_process(
        &self,
        context: &mut Context,
        result: &Result<serde_json::Value>,
    ) -> Result<ProcessResult<MyState>> {
        if let Ok(val) = result {
            context.set("report_draft", val.clone());
            append_transcript(context, "Postdoc", "report_draft");
            Ok(ProcessResult::new(MyState::Success, "success".to_string()))
        } else {
            Ok(ProcessResult::new(MyState::Failure, "failure".to_string()))
        }
    }
}

fn append_transcript(context: &mut Context, agent: &str, field: &str) {
    let entry = json!({
        "agent": agent,
        "field": field,
        "value": context.get(field).cloned().unwrap_or(json!(null))
    });
    println!("[{}] {} -> {}", agent, field, entry.get("value").cloned().unwrap_or(json!(null)));
    // Get existing transcript or create new array
    if let Some(existing) = context.get("transcript").cloned() {
        if let Some(mut arr) = existing.as_array().cloned() {
            arr.push(entry.clone());
            context.set("transcript", json!(arr));
            // continue to publish event
        }
    }
    if context.get("transcript").is_none() {
        context.set("transcript", json!([entry.clone()]));
    }

    // Publish realtime event
    let run_id = context.get("run_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let event = json!({
        "type": "transcript",
        "run_id": run_id,
        "agent": agent,
        "field": field,
        "value": context.get(field).cloned().unwrap_or(json!(null)),
        "ts_ms": ts,
    });
    publish_transcript(event);
}

// Build a minimal shared state snapshot for LLM context
fn context_snapshot(context: &Context) -> Value {
    let mut m = serde_json::Map::new();
    if let Some(v) = context.get("goal") { m.insert("goal".to_string(), v.clone()); }
    if let Some(v) = context.get("claims") { m.insert("claims".to_string(), v.clone()); }
    if let Some(v) = context.get("critic_review") { m.insert("critic_review".to_string(), v.clone()); }
    if let Some(v) = context.get("plan") { m.insert("plan".to_string(), v.clone()); }
    if let Some(v) = context.get("statistical_approval") { m.insert("statistical_approval".to_string(), v.clone()); }
    if let Some(v) = context.get("ethics_approval") { m.insert("ethics_approval".to_string(), v.clone()); }
    if let Some(v) = context.get("analysis_result") { m.insert("analysis_result".to_string(), v.clone()); }
    if let Some(v) = context.get("pi_synthesis") { m.insert("pi_synthesis".to_string(), v.clone()); }
    if let Some(v) = context.get("report_draft") { m.insert("report_draft".to_string(), v.clone()); }
    Value::Object(m)
}
