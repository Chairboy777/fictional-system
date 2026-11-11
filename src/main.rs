//! PHUSE EU Connect 2026 itinerary planner.
//!
//!   phuse-planner            serve the web app (default)
//!   phuse-planner fetch      download the live agenda into DATA_PATH and exit
//!
//! Environment:
//!   PORT                (default 8080)
//!   DATA_PATH           (default data/sessions.json)
//!   REFRESH_ON_START=1  fetch the live agenda at start-up, falling back to DATA_PATH
//!   ANTHROPIC_API_KEY   enables chat with Claude
//!   CLAUDE_MODEL        (default claude-opus-5-5)
//!   CLAUDE_EFFORT       (default medium)

mod chat;
mod fetch;
mod model;
mod recommend;
mod text;

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use model::Catalog;
use recommend::{Index, Profile};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct AppState {
    pub catalog: Catalog,
    pub index: Index,
    pub catalogue_text: String,
    pub claude: Option<chat::ClaudeConfig>,
    pub http: reqwest::Client,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| default.to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let data_path = env_or("DATA_PATH", "data/sessions.json");
    let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(300)).build()?;

    if std::env::args().nth(1).as_deref() == Some("fetch") {
        let cat = fetch::fetch_catalog(&http).await?;
        cat.save(&data_path)?;
        println!("Saved {} sessions to {data_path}", cat.sessions.len());
        return Ok(());
    }

    let refresh = env_or("REFRESH_ON_START", "0") == "1";
    let catalog = if refresh || !std::path::Path::new(&data_path).exists() {
        match fetch::fetch_catalog(&http).await {
            Ok(c) => {
                if let Err(e) = c.save(&data_path) {
                    eprintln!("warning: could not save agenda to {data_path}: {e}");
                }
                c
            }
            Err(e) => {
                eprintln!("warning: live agenda fetch failed ({e:#}); using {data_path}");
                Catalog::load(&data_path)?
            }
        }
    } else {
        Catalog::load(&data_path)?
    };
    println!("Loaded {} sessions (fetched {})", catalog.sessions.len(), catalog.fetched_at);

    let set = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let claude = if let Some(api_key) = set("ANTHROPIC_API_KEY") {
        Some(chat::ClaudeConfig {
            provider: chat::Provider::Anthropic,
            url: chat::API_URL.into(),
            api_key,
            model: env_or("CLAUDE_MODEL", "claude-opus-5-5"),
            effort: env_or("CLAUDE_EFFORT", "medium"),
        })
    } else {
        set("OLLAMA_MODEL").map(|model| chat::ClaudeConfig {
            provider: chat::Provider::Ollama,
            url: format!("{}/v1/messages", env_or("OLLAMA_URL", "http://localhost:11434").trim_end_matches('/')),
            api_key: String::new(),
            model,
            effort: String::new(),
        })
    };
    match &claude {
        Some(c) if c.provider == chat::Provider::Ollama => println!("Chat: Ollama ({}) at {}", c.model, c.url),
        Some(c) => println!("Chat: Claude ({}, effort {})", c.model, c.effort),
        None => println!("Chat: no ANTHROPIC_API_KEY or OLLAMA_MODEL — using the built-in command parser"),
    }

    let state = Arc::new(AppState {
        index: Index::new(&catalog),
        catalogue_text: chat::catalogue_text(&catalog),
        catalog,
        claude,
        http,
    });

    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("../static/index.html")) }))
        .route("/app.js", get(|| async { asset("application/javascript; charset=utf-8", include_str!("../static/app.js")) }))
        .route("/style.css", get(|| async { asset("text/css; charset=utf-8", include_str!("../static/style.css")) }))
        .route("/api/meta", get(meta))
        .route("/api/sessions", get(sessions))
        .route("/api/recommend", post(recommend))
        .route("/api/chat", post(chat_handler))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state);

    let port = env_or("PORT", "8080");
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    println!("Listening on http://localhost:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, "no-cache")], body).into_response()
}

#[derive(Serialize)]
struct StreamInfo {
    name: String,
    code: String,
    count: usize,
}

async fn meta(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let mut streams: BTreeMap<String, StreamInfo> = BTreeMap::new();
    for s in st.catalog.sessions.iter().filter(|s| !s.stream_code.is_empty()) {
        streams
            .entry(s.stream_code.clone())
            .or_insert_with(|| StreamInfo { name: s.stream.clone(), code: s.stream_code.clone(), count: 0 })
            .count += 1;
    }
    Json(json!({
        "event": st.catalog.event_name,
        "source_url": st.catalog.source_url,
        "fetched_at": st.catalog.fetched_at,
        "days": st.catalog.days().iter().map(|d| json!({"date": d, "label": model::day_label(d)})).collect::<Vec<_>>(),
        "streams": streams.into_values().collect::<Vec<_>>(),
        "chat": st.claude.as_ref().map(|c| json!({"model": c.model, "provider": c.label()})),
    }))
}

async fn sessions(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    Json(&st.catalog.sessions).into_response()
}

#[derive(Deserialize)]
struct RecommendRequest {
    profile: Profile,
    #[serde(default)]
    pinned: Vec<String>,
}

async fn recommend(State(st): State<Arc<AppState>>, Json(req): Json<RecommendRequest>) -> Response {
    let built = recommend::build(&st.catalog, &st.index, &req.profile, &req.pinned);
    // Scores keyed by session id, only for sessions with some relevance.
    let scores: BTreeMap<&str, &recommend::Scored> = st
        .catalog
        .sessions
        .iter()
        .zip(built.scores.iter())
        .filter(|(_, sc)| sc.score > 0.0)
        .map(|(s, sc)| (s.id.as_str(), sc))
        .collect();
    Json(json!({ "items": built.items, "scores": scores })).into_response()
}

async fn chat_handler(State(st): State<Arc<AppState>>, Json(req): Json<chat::ChatRequest>) -> Response {
    if req.message.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "empty message").into_response();
    }
    Json(chat::handle(&st, req).await).into_response()
}
