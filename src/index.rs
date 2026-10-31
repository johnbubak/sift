use crate::db;
use crate::sql;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    pub docs: i64,
    pub fts_rows: i64,
    pub body_bytes: i64,
    pub watermark_ms: i64,
    pub last_sync: i64,
    /// Rows added by the build that produced these stats. Meaningless for `status`.
    pub added: i64,
}

/// Incremental by default: `sql/extract.sql` resumes from `watermark_ms` and is
/// idempotent through `INSERT OR IGNORE` on `part_id`. `full` wipes first.
pub fn build(source: &Path, sidecar_path: &Path, full: bool) -> Result<IndexStats> {
    if !source.exists() {
        bail!("source database not found: {}", source.display());
    }
    let conn = db::open_sidecar(sidecar_path)?;

    if full {
        conn.execute_batch(
            "DELETE FROM sift_doc;
             DELETE FROM sift_seen;
             DELETE FROM sift_meta WHERE key = 'watermark_ms';",
        )
        .context("clearing sidecar for full rebuild")?;
    }

    let before: i64 = conn.query_row("SELECT count(*) FROM sift_doc", [], |r| r.get(0))?;

    db::attach_source(&conn, source)?;
    let extract = conn.execute_batch(sql::EXTRACT).context("running extraction (v1)");
    let extract_v2 = conn.execute_batch(sql::EXTRACT_V2).context("running extraction (v2)");
    // Detach even when extraction failed: otherwise the error message loses the
    // real cause and the source stays attached to the connection.
    let detach = db::detach_source(&conn);
    extract?;
    extract_v2?;
    detach?;

    let mut stats = status_from(&conn)?;
    stats.added = stats.docs - before;
    Ok(stats)
}

pub fn status(sidecar_path: &Path) -> Result<IndexStats> {
    let conn = db::open_sidecar(sidecar_path)?;
    status_from(&conn)
}

fn status_from(conn: &Connection) -> Result<IndexStats> {
    Ok(IndexStats {
        docs: conn.query_row("SELECT count(*) FROM sift_doc", [], |r| r.get(0))?,
        fts_rows: conn.query_row("SELECT count(*) FROM sift_fts", [], |r| r.get(0))?,
        body_bytes: conn.query_row(
            "SELECT COALESCE(SUM(length(body)), 0) FROM sift_doc",
            [],
            |r| r.get(0),
        )?,
        watermark_ms: meta_i64(conn, "watermark_ms")?,
        last_sync: meta_i64(conn, "last_sync")?,
        added: 0,
    })
}

fn meta_i64(conn: &Connection, key: &str) -> Result<i64> {
    Ok(conn
        .query_row(
            "SELECT COALESCE(CAST(value AS INTEGER), 0) FROM sift_meta WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .unwrap_or(0))
}
