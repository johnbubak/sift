//! The validated SQL stays in files.
//!
//! These three files are the verified artefact of Phase 0 — the whole pipeline
//! was proven end to end against the live 5.46 GB source with
//! `scripts/verify-sql.sh` before any of it was ported to Rust. Do not inline or
//! "improve" them here; change the file, re-run the script.

pub const MIGRATION: &str = include_str!("../migrations/001_sidecar.sql");
pub const EXTRACT: &str = include_str!("../sql/extract.sql");
pub const EXTRACT_V2: &str = include_str!("../sql/extract_v2.sql");
pub const SEARCH: &str = include_str!("../sql/search.sql");

// `sift extract` — in-memory line index, never part of the sidecar pipeline.
pub const EXTRACT_LINES: &str = include_str!("../sql/extract_lines.sql");
pub const EXTRACT_MATCHES: &str = include_str!("../sql/extract_matches.sql");
