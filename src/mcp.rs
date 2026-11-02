use rmcp::{
    ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    tool, tool_router,
};
use serde::Deserialize;

use crate::{db, search};

pub struct SiftServer;

#[derive(Deserialize, rmcp::schemars::JsonSchema, Default)]
pub struct SearchParams {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub recency: Option<f64>,
    #[serde(default)]
    pub and: Option<bool>,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default)]
    pub directory_contains: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub before: Option<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema, Default)]
pub struct SpurParams {
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub min_repeat: Option<i64>,
}

fn run_search(p: SearchParams) -> anyhow::Result<String> {
    if p.query.trim().is_empty() {
        anyhow::bail!("search needs a query");
    }
    let w = crate::load_weights();
    let limit = p.limit.unwrap_or(w.limit_default);
    let scope = p.scope.unwrap_or_default();
    let recency = p.recency;
    let and = p.and.unwrap_or(false);
    let directory = p.directory.unwrap_or_default();
    let directory_contains = p.directory_contains.unwrap_or_default();
    let after_ms = match p.after {
        Some(s) if !s.trim().is_empty() => crate::day_bound_ms(&s, false)?,
        _ => 0,
    };
    let before_ms = match p.before {
        Some(s) if !s.trim().is_empty() => crate::day_bound_ms(&s, true)?,
        _ => 0,
    };

    let conn = db::open_sidecar(&db::sidecar_path())?;
    let hits = search::search(
        &conn,
        &p.query,
        limit,
        &scope,
        &w,
        recency,
        and,
        &directory,
        &directory_contains,
        after_ms,
        before_ms,
    )?;

    if !scope.is_empty() {
        let ids: Vec<i64> = hits.iter().map(|h| h.doc_id).collect();
        let _ = search::mark_seen(&conn, &scope, &ids);
    }

    if hits.is_empty() {
        return Ok("no hits".to_string());
    }
    let mut out = String::new();
    for h in &hits {
        out.push_str(&h.render());
        out.push('\n');
    }
    Ok(out)
}

#[tool_router(server_handler)]
impl SiftServer {
    #[tool(name = "sift_search", description = "Search opencode session history (BM25 + recency, snippet-budgeted).")]
    fn sift_search(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match run_search(p) {
            Ok(text) => Ok(CallToolResult::success(vec![ContentBlock::text(text)])),
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(format!("{e:#}"))])),
        }
    }

    #[tool(name = "sift_spur", description = "Self-audit of the agent's own session trace (truncation, bad endings, loops).")]
    fn sift_spur(
        &self,
        Parameters(p): Parameters<SpurParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let limit = p.limit.unwrap_or(20);
        let min_repeat = p.min_repeat.unwrap_or(10);
        match crate::spur_report(limit, min_repeat) {
            Ok(text) => Ok(CallToolResult::success(vec![ContentBlock::text(text)])),
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(format!("{e:#}"))])),
        }
    }
}

pub async fn run() -> anyhow::Result<()> {
    let server = SiftServer;
    let service = server.serve(rmcp::transport::io::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
