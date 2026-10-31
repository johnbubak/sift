use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

/// The opencode history database. Read-only, always.
pub fn source_path() -> PathBuf {
    std::env::var("SIFT_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/share/opencode/opencode.db"))
}

/// Our own file. Deliberately outside opencode's directory.
pub fn sidecar_path() -> PathBuf {
    std::env::var("SIFT_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/share/sift/sift.db"))
}

/// `file:...?mode=ro` URI. NOT immutable=1: opencode writes to this database in
/// WAL mode right now, and immutable=1 skips WAL/SHM and silently returns stale
/// rows. See docs/00-archeologie.md.
pub fn source_uri(path: &Path) -> String {
    format!("file:{}?mode=ro", path.display())
}

/// Open the sidecar, creating and migrating it if needed.
pub fn open_sidecar(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating sidecar directory {}", dir.display()))?;
    }
    let conn = Connection::open(path)
        .with_context(|| format!("opening sidecar {}", path.display()))?;
    conn.execute_batch(crate::sql::MIGRATION)
        .context("applying sidecar migration")?;
    Ok(conn)
}

/// ATTACH the source read-only onto an existing connection.
///
/// Separate from `open_source` because extraction needs the source visible as
/// `src` next to the sidecar's own tables, and ATTACH accepts a bound
/// expression while `execute_batch` does not bind — hence the two-step dance.
pub fn attach_source(conn: &Connection, path: &Path) -> Result<()> {
    conn.execute("ATTACH DATABASE ?1 AS src", [source_uri(path)])
        .with_context(|| format!("attaching source {}", path.display()))?;
    Ok(())
}

pub fn detach_source(conn: &Connection) -> Result<()> {
    conn.execute_batch("DETACH DATABASE src")
        .context("detaching source")?;
    Ok(())
}
