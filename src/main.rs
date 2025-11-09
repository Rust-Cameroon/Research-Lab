mod state;
mod utils;
mod nodes;
mod cli;

use anyhow::Result;
use pocketflow_rs::{build_flow, Context};
use state::MyState;
use nodes::{
    GoalNode,
    ReaderNode,
    CriticNode,
    PlannerNode,
    StatisticianApprovalNode,
    EthicsApprovalNode,
    AnalystNode,
    PISynthesisNode,
    PostdocReportNode,
    FinalReportNode,
};

use clap::Parser;
use cli::{Cli, Commands, ServeCmd};
use std::io::{self, Write};
use dotenvy::dotenv;
use std::env;
use axum::{Router, routing::{get, post}, Json, extract::{ws::{WebSocketUpgrade, Message, WebSocket}, Query}};
use axum::http::StatusCode;
use axum::response::{IntoResponse, sse::{Sse, Event, KeepAlive}};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use uuid::Uuid;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
use std::convert::Infallible;
use utoipa::{OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;
use tower_http::cors::{CorsLayer, AllowMethods, AllowHeaders};
use axum::http::{Method, header};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenv(); // load .env if present
    // Optional: print key presence for troubleshooting
    let has_openai = env::var("OPENAI_API_KEY").is_ok();
    let has_tavily = env::var("TAVILY_API_KEY").is_ok();
    eprintln!("[env] OPENAI_API_KEY: {} | TAVILY_API_KEY: {}", has_openai, has_tavily);
    let cli = Cli::parse();

    // Instantiate agent role nodes
    let goal = GoalNode;
    let reader = ReaderNode;
    let critic = CriticNode;
    let planner = PlannerNode;
    let stat_approval = StatisticianApprovalNode;
    let ethics_approval = EthicsApprovalNode;
    let analyst = AnalystNode;
    let pi_synth = PISynthesisNode;
    let postdoc = PostdocReportNode;
    let final_report = FinalReportNode;

    // Build research lab flow (linear MVP)
    let flow = build_flow!(
        start: ("goal", goal),
        nodes: [
            ("reader", reader),
            ("critic", critic),
            ("planner", planner),
            ("stat", stat_approval),
            ("ethics", ethics_approval),
            ("analyst", analyst),
            ("pi", pi_synth),
            ("postdoc", postdoc),
            ("final_report", final_report)
        ],
        edges: [
            ("goal", "reader", MyState::Success),
            ("reader", "critic", MyState::Success),
            ("critic", "planner", MyState::Success),
            ("planner", "stat", MyState::Success),
            ("stat", "ethics", MyState::Success),
            ("ethics", "analyst", MyState::Success),
            ("analyst", "pi", MyState::Success),
            ("pi", "postdoc", MyState::Success),
            ("postdoc", "final_report", MyState::Success)
        ]
    );

    // Shared context
    let mut context = Context::new();

    // If CLI provided a goal (run) or chat message, seed it
    match &cli.command {
        Some(Commands::Run(run)) => {
            if let Some(title) = &run.goal {
                context.set("goal", serde_json::json!({"title": title}));
            }
        }
        Some(Commands::Chat(chat)) => {
            let message = if let Some(m) = &chat.message { m.clone() } else {
                print!("You> ");
                let _ = io::stdout().flush();
                let mut buf = String::new();
                io::stdin().read_line(&mut buf).expect("read line");
                buf.trim().to_string()
            };
            context.set("goal", serde_json::json!({"title": message}));
        }
        Some(Commands::Serve(cfg)) => {
            return serve_http(cfg).await;
        }
        _ => {}
    }

    // Run flow
    // Seed run_id for local runs too (helps logging consistency)
    let run_id = Uuid::new_v4().to_string();
    context.set("run_id", serde_json::json!(run_id));
    let ctx = flow.run(context).await?;

    // Print role-tagged outputs
    println!("\n=== Transcript ===");
    if let Some(v) = ctx.get("goal") { println!("[PI] Goal -> {}", v); }
    if let Some(v) = ctx.get("claims") { println!("[Reader] Claims -> {}", v); }
    if let Some(v) = ctx.get("critic_review") { println!("[Critic] Review -> {}", v); }
    if let Some(v) = ctx.get("plan") { println!("[Planner] Plan -> {}", v); }
    if let Some(v) = ctx.get("statistical_approval") { println!("[Statistician] Approval -> {}", v); }
    if let Some(v) = ctx.get("ethics_approval") { println!("[Ethics] Approval -> {}", v); }
    if let Some(v) = ctx.get("analysis_result") { println!("[Analyst] Analysis -> {}", v); }
    if let Some(v) = ctx.get("pi_synthesis") { println!("[PI] Synthesis -> {}", v); }
    if let Some(v) = ctx.get("report_draft") { println!("[Postdoc] Report Draft -> {}", v); }
    if let Some(v) = ctx.get("final_report") { println!("[PI] Final Report -> {}", v); }

    if let Some(t) = ctx.get("transcript") {
        if let Some(arr) = t.as_array() {
            for e in arr {
                let agent = e.get("agent").and_then(|v| v.as_str()).unwrap_or("?");
                let field = e.get("field").and_then(|v| v.as_str()).unwrap_or("");
                let value = e.get("value").cloned().unwrap_or(serde_json::json!(null));
                println!("[{}] {} -> {}", agent, field, value);
            }
        }
    }

    Ok(())
}

#[derive(Deserialize, ToSchema)]
struct RunRequest { goal: Option<String> }

#[derive(Serialize, ToSchema)]
struct RunResponse { transcript: serde_json::Value, final_report: serde_json::Value }

async fn serve_http(cfg: &ServeCmd) -> Result<()> {
    let cors = CorsLayer::new()
        .allow_methods(AllowMethods::list([Method::GET, Method::POST, Method::OPTIONS]))
        .allow_headers(AllowHeaders::list([header::CONTENT_TYPE]))
        .allow_origin([
            "http://localhost:3000".parse().unwrap(),
            "http://127.0.0.1:3000".parse().unwrap(),
        ]);

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/run", post(http_run))
        .route("/run-stream", post(http_run_stream))
        .route("/ws", get(ws_handler))
        .route("/stream", get(sse_stream))
        .merge(SwaggerUi::new("/docs").url("/openapi.json", ApiDoc::openapi()))
        .layer(cors);
    let addr = std::net::SocketAddr::from(([0,0,0,0], cfg.port));
    eprintln!("listening on http://{}", addr);
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await.map_err(|e| anyhow::anyhow!(e))?;
    Ok(())
}

#[utoipa::path(
    post,
    path = "/run",
    request_body = RunRequest,
    responses(
        (status = 200, description = "Run completed", body = RunResponse),
        (status = 500, description = "Internal error")
    ),
    tag = "lab"
)]
async fn http_run(Json(req): Json<RunRequest>) -> Result<Json<RunResponse>, (StatusCode, String)> {
    // Build the same flow as main
    let goal = GoalNode;
    let reader = ReaderNode;
    let critic = CriticNode;
    let planner = PlannerNode;
    let stat_approval = StatisticianApprovalNode;
    let ethics_approval = EthicsApprovalNode;
    let analyst = AnalystNode;
    let pi_synth = PISynthesisNode;
    let postdoc = PostdocReportNode;
    let final_report = FinalReportNode;

    let flow = build_flow!(
        start: ("goal", goal),
        nodes: [
            ("reader", reader), ("critic", critic), ("planner", planner), ("stat", stat_approval),
            ("ethics", ethics_approval), ("analyst", analyst), ("pi", pi_synth), ("postdoc", postdoc), ("final_report", final_report)
        ],
        edges: [
            ("goal", "reader", MyState::Success), ("reader", "critic", MyState::Success), ("critic", "planner", MyState::Success),
            ("planner", "stat", MyState::Success), ("stat", "ethics", MyState::Success), ("ethics", "analyst", MyState::Success),
            ("analyst", "pi", MyState::Success), ("pi", "postdoc", MyState::Success), ("postdoc", "final_report", MyState::Success)
        ]
    );

    let mut context = Context::new();
    if let Some(title) = req.goal { context.set("goal", serde_json::json!({"title": title})); }
    let run_id = Uuid::new_v4().to_string();
    context.set("run_id", serde_json::json!(run_id));

    let ctx = flow.run(context).await.map_err(internal_error)?;
    let transcript = ctx.get("transcript").cloned().unwrap_or(serde_json::json!([]));
    let final_report = ctx.get("final_report").cloned().unwrap_or(serde_json::json!({}));
    Ok(Json(RunResponse{ transcript, final_report }))
}

#[derive(Deserialize)]
struct WsParams { run_id: String, delay_ms: Option<u64> }

async fn ws_handler(ws: WebSocketUpgrade, Query(WsParams{ run_id, delay_ms }): Query<WsParams>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, run_id, delay_ms.unwrap_or(0)))
}

async fn handle_ws(mut socket: WebSocket, run_id: String, delay_ms: u64) {
    let mut rx = crate::utils::subscribe_transcript();
    while let Ok(event) = rx.recv().await {
        if event.get("run_id").and_then(|v| v.as_str()).unwrap_or("") == run_id {
            let _ = socket.send(Message::Text(event.to_string())).await;
            if delay_ms > 0 { tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await; }
        }
    }
}

fn internal_error<E: std::fmt::Display>(err: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

#[derive(Serialize, ToSchema)]
struct RunStreamResponse { run_id: String }

#[derive(Serialize, ToSchema)]
struct TranscriptEvent {
    r#type: String,
    run_id: String,
    agent: String,
    field: String,
    value: serde_json::Value,
    ts_ms: u64,
}

#[derive(OpenApi)]
#[openapi(
    paths(http_run, http_run_stream, sse_stream),
    components(schemas(RunRequest, RunResponse, RunStreamResponse, TranscriptEvent)),
    tags((name = "lab", description = "Research Lab REST API"))
)]
struct ApiDoc;

#[utoipa::path(
    post,
    path = "/run-stream",
    request_body = RunRequest,
    responses(
        (status = 200, description = "Run started", body = RunStreamResponse),
        (status = 500, description = "Internal error")
    ),
    tag = "lab"
)]
async fn http_run_stream(Json(req): Json<RunRequest>) -> Result<Json<RunStreamResponse>, (StatusCode, String)> {
    // Build flow
    let goal = GoalNode;
    let reader = ReaderNode;
    let critic = CriticNode;
    let planner = PlannerNode;
    let stat_approval = StatisticianApprovalNode;
    let ethics_approval = EthicsApprovalNode;
    let analyst = AnalystNode;
    let pi_synth = PISynthesisNode;
    let postdoc = PostdocReportNode;
    let final_report = FinalReportNode;

    let flow = build_flow!(
        start: ("goal", goal),
        nodes: [
            ("reader", reader), ("critic", critic), ("planner", planner), ("stat", stat_approval),
            ("ethics", ethics_approval), ("analyst", analyst), ("pi", pi_synth), ("postdoc", postdoc), ("final_report", final_report)
        ],
        edges: [
            ("goal", "reader", MyState::Success), ("reader", "critic", MyState::Success), ("critic", "planner", MyState::Success),
            ("planner", "stat", MyState::Success), ("stat", "ethics", MyState::Success), ("ethics", "analyst", MyState::Success),
            ("analyst", "pi", MyState::Success), ("pi", "postdoc", MyState::Success), ("postdoc", "final_report", MyState::Success)
        ]
    );

    // Prepare context
    let mut context = Context::new();
    if let Some(title) = req.goal { context.set("goal", serde_json::json!({"title": title})); }
    let run_id = Uuid::new_v4().to_string();
    context.set("run_id", serde_json::json!(run_id.clone()));

    // Spawn
    tokio::spawn(async move {
        let _ = flow.run(context).await; // transcript events are broadcast by nodes
    });

    Ok(Json(RunStreamResponse{ run_id }))
}

#[derive(Deserialize)]
struct StreamParams { run_id: String, delay_ms: Option<u64> }

#[utoipa::path(
    get,
    path = "/stream",
    params(
        ("run_id" = String, Query, description = "Flow run identifier"),
        ("delay_ms" = u64, Query, description = "Optional pacing delay per event in ms")
    ),
    responses(
        (status = 200, description = "SSE stream of transcript events", content_type = "text/event-stream", body = TranscriptEvent)
    ),
    tag = "lab"
)]
async fn sse_stream(Query(StreamParams{ run_id, delay_ms }): Query<StreamParams>) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = crate::utils::subscribe_transcript();
    let delay = delay_ms.unwrap_or(0);
    let stream = BroadcastStream::new(rx)
        .filter_map(|evt| evt.ok())
        .filter_map(move |v| {
            let ok = v.get("run_id").and_then(|x| x.as_str()).unwrap_or("") == run_id;
            if ok { Some(v) } else { None }
        })
        .then(move |v| {
            let d = delay;
            async move {
                if d > 0 { tokio::time::sleep(std::time::Duration::from_millis(d)).await; }
                v
            }
        })
        .map(|v| {
            let mut ev = Event::default();
            ev = ev.json_data(&v).unwrap_or_else(|_| Event::default().data("{}"));
            Ok(ev)
        });

    Sse::new(stream).keep_alive(KeepAlive::default())
}
