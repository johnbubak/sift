use crate::db;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn repos_dir() -> PathBuf {
    db::sidecar_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("repos")
}

fn open_sidecar() -> Result<Connection> {
    db::open_sidecar(&db::sidecar_path())
}

fn sha_short(s: &str) -> String {
    // Stable, dependency-free hash for repo directory names.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

fn repo_name_from_url(url: &str) -> String {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("repo")
        .trim_end_matches(".git")
        .to_string()
}

fn slug_from_url(url: &str) -> String {
    // owner/repo for GitHub/Gitea APIs.
    let path = url.split("://").nth(1).unwrap_or(url);
    let parts: Vec<&str> = path.trim_end_matches('/').trim_end_matches(".git")
        .split('/').collect();
    if parts.len() >= 3 {
        format!("{}/{}", parts[parts.len()-2], parts[parts.len()-1])
    } else {
        repo_name_from_url(url)
    }
}

fn host_from_url(url: &str) -> String {
    // crude: github.com / gitea host
    url.split("://")
        .nth(1)
        .and_then(|s| s.split('/').next())
        .unwrap_or("")
        .to_string()
}

pub fn cmd_git(args: &[String]) -> Result<()> {
    let Some(sub) = args.first() else {
        bail!("usage: sift git <add|sync|search|list|status> ...\n");
    };
    match sub.as_str() {
        "add" => cmd_add(&args[1..]),
        "sync" => cmd_sync(&args[1..]),
        "search" => cmd_search(&args[1..]),
        "list" => cmd_list(),
        "status" => cmd_status(),
        other => bail!("unknown git subcommand '{other}'"),
    }
}

fn cmd_add(args: &[String]) -> Result<()> {
    let url = args.first().context("missing repo url")?;
    let dir = repos_dir().join(sha_short(url));
    std::fs::create_dir_all(&dir).context("creating repos dir")?;
    if !dir.join(".git").exists() {
        let status = Command::new("git")
            .args(["clone", "--depth", "1", url, dir.to_str().unwrap()])
            .status()
            .context("running git clone")?;
        if !status.success() {
            bail!("git clone failed");
        }
    }
    let conn = open_sidecar()?;
    conn.execute(
        "INSERT OR IGNORE INTO git_repo (url, name, path, host) VALUES (?1,?2,?3,?4)",
        params![url, repo_name_from_url(url), dir.display().to_string(), host_from_url(url)],
    )?;
    println!("added repo {} -> {}", repo_name_from_url(url), dir.display());
    Ok(())
}

fn cmd_list() -> Result<()> {
    let conn = open_sidecar()?;
    let mut stmt = conn.prepare("SELECT id,name,url,last_sync FROM git_repo ORDER BY id")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?))
    })?;
    for r in rows {
        let (id, name, url, ls) = r?;
        println!("{id} {name} {url} last_sync={ls}");
    }
    Ok(())
}

fn cmd_sync(args: &[String]) -> Result<()> {
    let filter = args.first().cloned();
    let conn = open_sidecar()?;
    let repos: Vec<(i64, String, String, String, String)> = {
        let mut stmt = conn.prepare("SELECT id,name,path,host,url FROM git_repo")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (id, name, path, host, url) in repos {
        if let Some(f) = &filter {
            if name != *f && id.to_string() != *f {
                continue;
            }
        }
        // fetch
        let _ = Command::new("git").args(["-C", &path, "fetch", "--depth", "100"]).status();
        index_commits(&conn, id, &path)?;
        if host.contains("github.com") {
            index_github(&conn, id, &name, &url)?;
        } else {
            index_gitea(&conn, id, &host, &slug_from_url(&url));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
        conn.execute("UPDATE git_repo SET last_sync=?1 WHERE id=?2", params![now, id])?;
        println!("synced {name}");
    }
    Ok(())
}

fn index_commits(conn: &Connection, repo_id: i64, path: &str) -> Result<()> {
    let out = Command::new("git")
        .args(["-C", path, "log", "--all", "--format=%H%x1f%an%x1f%aI%x1f%s%x1f%b%x1e"])
        .output()
        .context("git log")?;
    let text = String::from_utf8_lossy(&out.stdout);
    for rec in text.split('\x1e') {
        let rec = rec.trim();
        if rec.is_empty() { continue; }
        let parts: Vec<&str> = rec.splitn(5, '\x1f').collect();
        if parts.len() < 4 { continue; }
        let sha = parts[0].trim();
        let author = parts[1].trim();
        let date = parts[2].trim();
        let subject = parts[3].trim();
        let body = parts.get(4).copied().unwrap_or("").trim();
        let ts = parse_iso_ms(date);
        let full_body = if body.is_empty() { subject.to_string() } else { format!("{subject}\n\n{body}") };
        let fp = format!("{}:{}", full_body.len(), full_body.chars().take(64).collect::<String>());
        conn.execute(
            "INSERT OR IGNORE INTO git_doc (repo_id,kind,ref,author,ts,title,body,body_fp) VALUES (?1,'commit',?2,?3,?4,?5,?6,?7)",
            params![repo_id, sha, author, ts, subject, full_body, fp],
        )?;
    }
    Ok(())
}

fn github_slug(url: &str, name: &str) -> String {
    // GitHub API needs owner/repo; derive from URL, fall back to name.
    let path = url.split("://").nth(1).unwrap_or("").split('/').skip(1).collect::<Vec<_>>().join("/");
    let path = path.trim_end_matches(".git").trim_end_matches('/');
    if path.contains('/') { path.to_string() } else { name.to_string() }
}

fn index_github(conn: &Connection, repo_id: i64, name: &str, url: &str) -> Result<()> {
    let slug = github_slug(url, name);
    // issues
    let out = Command::new("gh")
        .args(["issue", "list", "--repo", &slug, "--state", "all", "--limit", "200", "--json", "number,title,body,author,createdAt"])
        .output();
    if let Ok(out) = out {
        if out.status.success() {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                if let Some(arr) = v.as_array() {
                    for it in arr {
                        let num = it.get("number").and_then(|x| x.as_i64()).unwrap_or(0);
                        let title = it.get("title").and_then(|x| x.as_str()).unwrap_or("");
                        let body = it.get("body").and_then(|x| x.as_str()).unwrap_or("");
                        let author = it.get("author").and_then(|x| x.get("login")).and_then(|x| x.as_str()).unwrap_or("");
                        let created = it.get("createdAt").and_then(|x| x.as_str()).unwrap_or("");
                        let ts = parse_iso_ms(created);
                        let fp = format!("{}:{}", body.len(), body.chars().take(64).collect::<String>());
                        let _ = conn.execute(
                            "INSERT OR IGNORE INTO git_doc (repo_id,kind,ref,author,ts,title,body,body_fp) VALUES (?1,'issue',?2,?3,?4,?5,?6,?7)",
                            params![repo_id, num.to_string(), author, ts, title, body, fp],
                        );
                    }
                }
            }
        }
    }
    // prs
    let out = Command::new("gh")
        .args(["pr", "list", "--repo", &slug, "--state", "all", "--limit", "200", "--json", "number,title,body,author,createdAt"])
        .output();
    if let Ok(out) = out {
        if out.status.success() {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                if let Some(arr) = v.as_array() {
                    for it in arr {
                        let num = it.get("number").and_then(|x| x.as_i64()).unwrap_or(0);
                        let title = it.get("title").and_then(|x| x.as_str()).unwrap_or("");
                        let body = it.get("body").and_then(|x| x.as_str()).unwrap_or("");
                        let author = it.get("author").and_then(|x| x.get("login")).and_then(|x| x.as_str()).unwrap_or("");
                        let created = it.get("createdAt").and_then(|x| x.as_str()).unwrap_or("");
                        let ts = parse_iso_ms(created);
                        let fp = format!("{}:{}", body.len(), body.chars().take(64).collect::<String>());
                        let _ = conn.execute(
                            "INSERT OR IGNORE INTO git_doc (repo_id,kind,ref,author,ts,title,body,body_fp) VALUES (?1,'pr',?2,?3,?4,?5,?6,?7)",
                            params![repo_id, num.to_string(), author, ts, title, body, fp],
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn index_gitea(conn: &Connection, repo_id: i64, host: &str, slug: &str) {
    let token = match std::env::var("GITEA_TOKEN").or_else(|_| std::env::var("GITEA_API_TOKEN")) {
        Ok(t) if !t.is_empty() => t,
        _ => return, // fail-open: commits only
    };
    let base = format!("https://{}/api/v1/repos/{}", host, slug);
    for (kind, endpoint) in [("issue", "issues?state=all&limit=50"), ("pr", "pulls?state=all&limit=50")] {
        let url = format!("{}/{}", base, endpoint);
        let out = std::process::Command::new("curl")
            .args(["-fsS", "-H", &format!("Authorization: token {}", token), &url])
            .output();
        if let Ok(out) = out {
            if !out.status.success() { continue; }
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                if let Some(arr) = v.as_array() {
                    for it in arr {
                        let num = it.get("number").and_then(|x| x.as_i64()).unwrap_or(0);
                        let title = it.get("title").and_then(|x| x.as_str()).unwrap_or("");
                        let body = it.get("body").and_then(|x| x.as_str()).unwrap_or("");
                        let author = it.get("user").and_then(|x| x.get("login")).and_then(|x| x.as_str()).unwrap_or("");
                        let created = it.get("created_at").and_then(|x| x.as_str()).unwrap_or("");
                        let ts = parse_iso_ms(created);
                        let fp = format!("{}:{}", body.len(), body.chars().take(64).collect::<String>());
                        let _ = conn.execute(
                            "INSERT OR IGNORE INTO git_doc (repo_id,kind,ref,author,ts,title,body,body_fp) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                            rusqlite::params![repo_id, kind, num.to_string(), author, ts, title, body, fp],
                        );
                    }
                }
            }
        }
    }
}

pub fn cmd_search(args: &[String]) -> Result<()> {
    let mut term = String::new();
    let mut repo: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--repo" => { repo = args.get(i+1).cloned(); i += 2; continue; }
            _ => { if !term.is_empty() { term.push(' '); } term.push_str(&args[i]); i += 1; }
        }
    }
    if term.is_empty() { bail!("missing search term"); }
    let conn = open_sidecar()?;
    let fts_query = format!("{}*", term.replace('"', ""));
    let sql = if repo.is_some() {
        "SELECT g.kind, g.ref, g.author, g.ts, g.title, snippet(git_fts,1,'[',']','...',12) \
         FROM git_fts JOIN git_doc g ON g.id=git_fts.rowid JOIN git_repo r ON r.id=g.repo_id \
         WHERE git_fts MATCH ?1 AND r.name=?2 ORDER BY rank LIMIT 20"
    } else {
        "SELECT g.kind, g.ref, g.author, g.ts, g.title, snippet(git_fts,1,'[',']','...',12) \
         FROM git_fts JOIN git_doc g ON g.id=git_fts.rowid \
         WHERE git_fts MATCH ?1 ORDER BY rank LIMIT 20"
    };
    let mut stmt = conn.prepare(sql)?;
    let map_row = |r: &rusqlite::Row| -> rusqlite::Result<String> {
        Ok(format!("{} {} · {} · {} · {}",
            r.get::<_, String>(0)?, r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            r.get::<_, i64>(3)?, r.get::<_, String>(5)?))
    };
    if let Some(repo) = repo {
        let rows = stmt.query_map(params![fts_query, repo], map_row)?;
        for r in rows { println!("{}", r?); }
    } else {
        let rows = stmt.query_map(params![fts_query], map_row)?;
        for r in rows { println!("{}", r?); }
    }
    Ok(())
}

fn cmd_status() -> Result<()> {
    let conn = open_sidecar()?;
    let repos: i64 = conn.query_row("SELECT count(*) FROM git_repo", [], |r| r.get(0))?;
    let docs: i64 = conn.query_row("SELECT count(*) FROM git_doc", [], |r| r.get(0))?;
    println!("git repos={repos} docs={docs}");
    Ok(())
}

fn parse_iso_ms(s: &str) -> i64 {
    // Best-effort: use sqlite to parse ISO8601.
    let conn = Connection::open_in_memory().unwrap();
    conn.query_row("SELECT COALESCE(CAST(strftime('%s', ?1) AS INTEGER),0)*1000", [s], |r| r.get(0)).unwrap_or(0)
}
