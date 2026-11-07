use crate::db;
use anyhow::{bail, Context, Result};
use rusqlite::params;
use std::io::{self, BufRead, Write};

pub fn cmd_feedback(args: &[String]) -> Result<()> {
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let conn = db::open_sidecar(&db::sidecar_path())?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    // Rate limit: max one real send per day.
    let last: i64 = conn
        .query_row(
            "SELECT COALESCE(CAST(value AS INTEGER),0) FROM sift_meta WHERE key='last_feedback_ms'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if !dry_run && now - last < 86_400_000 {
        bail!("feedback rate-limited: last sent {} ms ago (max 1/day)", now - last);
    }

    let docs: i64 = conn
        .query_row("SELECT count(*) FROM sift_doc", [], |r| r.get(0))
        .unwrap_or(0);
    let fts: i64 = conn
        .query_row("SELECT count(*) FROM sift_fts", [], |r| r.get(0))
        .unwrap_or(0);
    let git_repos: i64 = conn
        .query_row("SELECT count(*) FROM git_repo", [], |r| r.get(0))
        .unwrap_or(0);
    let git_docs: i64 = conn
        .query_row("SELECT count(*) FROM git_doc", [], |r| r.get(0))
        .unwrap_or(0);

    let mut flags = Vec::new();
    #[cfg(feature = "mcp")] flags.push("mcp");
    #[cfg(feature = "spur")] flags.push("spur");
    #[cfg(feature = "files")] flags.push("files");
    #[cfg(feature = "trigger")] flags.push("trigger");
    #[cfg(feature = "git")] flags.push("git");
    #[cfg(feature = "web")] flags.push("web");
    #[cfg(feature = "pro")] flags.push("pro");

    let payload = format!(
        "version={}\nos={}\narch={}\nfeatures={}\nsift_docs={}\nfts_rows={}\ngit_repos={}\ngit_docs={}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        flags.join(","),
        docs,
        fts,
        git_repos,
        git_docs,
    );

    println!("--- feedback payload (no DB contents) ---");
    print!("{}", payload);
    println!("----------------------------------------");

    if dry_run {
        return Ok(());
    }

    // Confirm before sending.
    print!("Send this as a GitHub issue to johnbubak/sift? [y/N] ");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    if !line.trim().eq_ignore_ascii_case("y") {
        println!("aborted");
        return Ok(());
    }

    let title = format!("feedback: sift {}", env!("CARGO_PKG_VERSION"));
    let status = std::process::Command::new("gh")
        .args([
            "issue",
            "create",
            "--repo",
            "johnbubak/sift",
            "--title",
            &title,
            "--body",
            &payload,
        ])
        .status()
        .context("running gh")?;
    if !status.success() {
        bail!("gh issue create failed");
    }

    conn.execute(
        "INSERT INTO sift_meta(key,value) VALUES('last_feedback_ms',?1) \
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![now.to_string()],
    )?;
    println!("feedback sent");
    Ok(())
}
