//! Ranking weights. This file is what `julia/sweep.jl` writes; the defaults below
//! are the unproven starting point.
//!
//! Nothing here may drift out of sync with `sql/search.sql` — every field maps to
//! a `?N` parameter. If you add a weight, wire the parameter and re-run
//! `scripts/verify-sql.sh`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct Weights {
    /// Weight of the recency ranking inside the RRF fusion. 0.0 = pure BM25.
    pub recency_weight: f64,
    /// bm25() weight for the session title column.
    pub title_weight: f64,
    /// bm25() weight for the body column.
    pub body_weight: f64,
    /// snippet() budget, in TOKENS not words.
    pub snippet_tokens: i64,
    /// At most this many hits per session in one result set.
    pub max_per_session: i64,
    /// Default number of hits.
    pub limit_default: i64,
    /// RRF damping constant. 60 is the literature default for k.
    pub rrf_k: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            recency_weight: 0.3,
            title_weight: 1.0,
            body_weight: 3.0,
            snippet_tokens: 30,
            max_per_session: 2,
            limit_default: 3,
            rrf_k: 60.0,
        }
    }
}

impl Weights {
    /// Load from `weights.toml`. A missing file is fine and means defaults; a
    /// malformed file is an error, not a silent fallback — a broken sweep result
    /// must never masquerade as a working config.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Where weights.toml is looked up: next to the binary, then the working dir.
    pub fn discover() -> Option<std::path::PathBuf> {
        if let Ok(explicit) = std::env::var("SIFT_WEIGHTS") {
            return Some(explicit.into());
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let candidate = dir.join("weights.toml");
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
        let cwd = std::path::PathBuf::from("weights.toml");
        if cwd.exists() {
            Some(cwd)
        } else {
            None
        }
    }
}
