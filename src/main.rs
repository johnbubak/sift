//! sift — session recall for coding agents.
//!
//! Phase 1: CLI. The MCP server (rmcp) is Phase 2 on purpose — rmcp 3.5.1 was
//! three days old when this was written and unverified macro glue is worse than
//! a working CLI. See docs/03-phasen.md.

mod db;
mod extract;
mod mcp;
mod index;
mod search;
mod sql;
mod trigger;
mod weights;

use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use search::Hit;
use std::collections::HashMap;
use weights::Weights;

/// Write to stdout, treating a closed pipe as a normal end of output.
///
/// Rust sets SIGPIPE to SIG_IGN before `main`, so `sift … | head` makes
/// `println!` panic with `Broken pipe` and exit 101. A consumer that stopped
/// reading is not an error — it is the everyday `| head` case — so on EPIPE the
/// process exits 141 (128 + 13, the status a SIGPIPE death would report) and
/// prints nothing to stderr. No libc and no `unsafe`: `std::io::ErrorKind`
/// names the condition, so the repo stays at zero unsafe blocks.
///
/// Any other write error is real and stays loud: stderr message, exit 1.
fn out(args: std::fmt::Arguments, newline: bool) {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    let result = if newline {
        writeln!(stdout, "{args}")
    } else {
        write!(stdout, "{args}")
    };
    match result {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(141),
        Err(e) => {
            eprintln!("sift: writing to stdout: {e}");
            std::process::exit(1);
        }
    }
}

/// `println!` through [`out`]. Every stdout write goes through here so a broken
/// pipe can never reach the panicking print macros.
macro_rules! outln {
    ($($arg:tt)*) => {
        crate::out(format_args!($($arg)*), true)
    };
}

/// `print!` through [`out`] — for text that already ends in a newline.
macro_rules! out {
    ($($arg:tt)*) => {
        crate::out(format_args!($($arg)*), false)
    };
}

const USAGE: &str = "\
sift — local session recall over opencode's SQLite history

USAGE
  sift build [--full]                 extract new content into the sidecar index
  sift status                         index size, watermark, last sync
  sift search <query...> [OPTIONS]    BM25 + recency, snippet-budgeted
  sift extract <file> --for <query>   every matching line of one file, with context
  sift files <dir> --for <query>      same, recursively over a project directory  sift mark-seen <scope> <id...>      record doc ids as delivered
  sift should-search <message...>     is this a recall request? (fail-open)
  sift spur [--limit N] [--min-repeat N]  self-audit: truncation, bad endings, loops
  sift mcp                               run as MCP stdio server

OPTIONS (search)
  --limit N          hits to return                (default from weights.toml)
  --scope S          dedup scope; repeat the same S to avoid repeats
  --recency W        override recency weight        (default from weights.toml)
  --directory D      keep only hits from exactly this directory
  --directory-contains S   keep only hits whose directory contains S
  --after YYYY-MM-DD       keep hits from local 00:00:00 on that day onwards
  --before YYYY-MM-DD      keep hits up to local 23:59:59 on that day
  --and              require ALL terms in one document (default: OR, ranked by bm25)
  --json             machine-readable output

OPTIONS (extract)
  --for Q            search query (required); every matching line is kept
  --context N        context lines left/right per hit  (default 6, max 200)
  --json             machine-readable output

OPTIONS (should-search)
  --json             machine-readable output

ENV
  SIFT_SOURCE   path to opencode.db   (default ~/.local/share/opencode/opencode.db)
  SIFT_DB       path to the sidecar   (default ~/.local/share/sift/sift.db)
  SIFT_WEIGHTS  path to weights.toml  (default: next to the binary, then ./weights.toml)
";

fn main() {
    if let Err(e) = run() {
        eprintln!("sift: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        out!("{USAGE}");
        return Ok(());
    };
    match cmd.as_str() {
        "build" => cmd_build(&args[1..]),
        "status" => cmd_status(),
        "search" => cmd_search(&args[1..]),
        "extract" => cmd_extract(&args[1..]),
        "files" => cmd_files(&args[1..]),        "mark-seen" => cmd_mark_seen(&args[1..]),
        "should-search" => cmd_should_search(&args[1..]),
        "spur" => cmd_spur(&args[1..]),
        "mcp" => cmd_mcp(),
        "-h" | "--help" | "help" => {
            out!("{USAGE}");
            Ok(())
        }
        other => bail!("unknown command '{other}'\n\n{USAGE}"),
    }
}

fn cmd_mcp() -> Result<()> {
    let rt = tokio::runtime::Runtime::new().context("creating tokio runtime")?;
    rt.block_on(crate::mcp::run())
}

fn cmd_build(args: &[String]) -> Result<()> {
    let full = args.iter().any(|a| a == "--full");
    let source = db::source_path();
    let sidecar = db::sidecar_path();
    outln!("source  {}", source.display());
    outln!("sidecar {}", sidecar.display());

    let stats = index::build(&source, &sidecar, full)?;
    print_stats(&stats, full);
    if stats.fts_rows != stats.docs {
        bail!(
            "FTS out of sync: {} docs vs {} index rows — run 'sift build --full'",
            stats.docs,
            stats.fts_rows
        );
    }
    Ok(())
}

fn cmd_status() -> Result<()> {
    let sidecar = db::sidecar_path();
    if !sidecar.exists() {
        outln!("no sidecar at {} — run 'sift build'", sidecar.display());
        return Ok(());
    }
    let stats = index::status(&sidecar)?;
    print_stats(&stats, false);
    Ok(())
}

fn print_stats(stats: &index::IndexStats, full: bool) {
    let verb = if full { "rebuilt" } else { "added" };
    let watermark = if stats.watermark_ms == 0 {
        "unset".to_string()
    } else {
        search::format_ms(stats.watermark_ms)
    };
    outln!(
        "{verb} {} · {} docs · {} index rows · {} MB body · watermark {watermark} · last sync {}",
        stats.added,
        stats.docs,
        stats.fts_rows,
        stats.body_bytes / 1_048_576,
        if stats.last_sync == 0 {
            "never".to_string()
        } else {
            search::format_ms(stats.last_sync * 1000)
        },
    );
}

/// `--after` / `--before` value `YYYY-MM-DD` as ms epoch, local time.
///
/// `end_of_day = false` -> local 00:00:00.000 of that day (`>=` bound),
/// `end_of_day = true`  -> local 23:59:59.999 of that day (`<=` bound).
/// The `before` bound deliberately ends the day instead of starting it: a
/// midnight bound would drop the entire day the caller named. It ends at
/// 23:59:59.**999**, not .000: `time_created` is ms-precision, so a
/// second-resolution bound would silently lose the last 999 ms of the day.
///
/// Local, not UTC: the corpus is human history, "since June" means the user's
/// June, not 00:00 UTC. The conversion is delegated to SQLite, which is already
/// linked and already knows the system timezone (DST included) — the same
/// reasoning as format_ms() in search.rs, which avoids pulling in chrono for
/// one line. No hand-rolled timezone table, no subprocess.
///
/// A malformed date is an error, never a silent 0 (= "filter off"): a typo in
/// `--after` must not look like "no filter given" and hand back the whole corpus.
fn day_bound_ms(raw: &str, end_of_day: bool) -> Result<i64> {
    let (y, m, d) = parse_day(raw)?;
    let clock = if end_of_day { "23:59:59" } else { "00:00:00" };
    // 'utc' modifier: the literal is LOCAL, the result is a UTC epoch.
    let conn = Connection::open_in_memory().context("opening in-memory calendar")?;
    let secs: Option<i64> = conn.query_row(
        "SELECT CAST(strftime('%s', ?1, 'utc') AS INTEGER)",
        [format!("{y:04}-{m:02}-{d:02} {clock}")],
        |row| row.get(0),
    )?;
    match secs {
        // SQLite works in whole seconds; the last ms of the day is added here.
        Some(s) if end_of_day => Ok(s * 1000 + 999),
        Some(s) => Ok(s * 1000),
        None => bail!("no epoch for {raw:?} — outside the representable calendar"),
    }
}

/// Strict `YYYY-MM-DD` with a real calendar check, returned as (year, month, day).
///
/// The calendar check is here and not delegated to SQLite because SQLite
/// normalises impossible dates instead of refusing them: measured 2026-10-08,
/// `date('2026-02-30')` = `2026-03-02` and `strftime('%s', '2026-02-30', 'utc')`
/// returns a perfectly usable epoch. A typo in `--after` would then silently move
/// the window by two days. The shape check is kept as well, because the SQLite
/// answer to '2026-6-1' is a bare NULL and this way the message can name the
/// expected format.
fn parse_day(raw: &str) -> Result<(i64, u32, u32)> {
    let bytes = raw.as_bytes();
    let shape_ok = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    if !shape_ok {
        bail!("expected a date as YYYY-MM-DD, got {raw:?}");
    }
    let num = |range: std::ops::Range<usize>| -> Result<u32> {
        raw[range]
            .parse::<u32>()
            .with_context(|| format!("expected a date as YYYY-MM-DD, got {raw:?}"))
    };
    let y = num(0..4)? as i64;
    let m = num(5..7)?;
    let d = num(8..10)?;
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days_in_month = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => bail!("month {m} out of range in {raw:?} (expected YYYY-MM-DD)"),
    };
    if y < 1 {
        bail!("year {y} out of range in {raw:?} (expected YYYY-MM-DD)");
    }
    if d == 0 || d > days_in_month {
        bail!("{raw:?} is not a real date — {m:02} has {days_in_month} days");
    }
    Ok((y, m, d))
}

fn cmd_search(args: &[String]) -> Result<()> {
    let (positional, flags) = parse_args(
        args,
        &[
            "--limit",
            "--scope",
            "--recency",
            "--directory",
            "--directory-contains",
            "--after",
            "--before",
        ],
    )?;
    if positional.is_empty() {
        bail!("search needs a query\n\n{USAGE}");
    }
    let query = positional.join(" ");

    let w = load_weights();
    let limit = flags
        .get("--limit")
        .map(|v| v.parse::<i64>())
        .transpose()?
        .unwrap_or(w.limit_default);
    let scope = flags.get("--scope").map(String::as_str).unwrap_or("");
    let recency = match flags.get("--recency") {
        Some(v) => Some(v.parse::<f64>().context("--recency must be a number")?),
        None => None,
    };
    // Empty string = filter off. Passing "" through unchanged keeps the SQL
    // contract ("empty means inactive") in one place instead of two.
    let directory = flags.get("--directory").map(String::as_str).unwrap_or("");
    let directory_contains = flags
        .get("--directory-contains")
        .map(String::as_str)
        .unwrap_or("");
    let after_ms = match flags.get("--after") {
        Some(v) => day_bound_ms(v, false).with_context(|| format!("--after {v:?}"))?,
        None => 0,
    };
    let before_ms = match flags.get("--before") {
        Some(v) => day_bound_ms(v, true).with_context(|| format!("--before {v:?}"))?,
        None => 0,
    };
    if after_ms != 0 && before_ms != 0 && after_ms > before_ms {
        bail!(
            "--after {} is later than --before {}",
            search::format_ms(after_ms),
            search::format_ms(before_ms)
        );
    }
    let as_json = flags.contains_key("--json");
    let and = flags.contains_key("--and");

    let conn = db::open_sidecar(&db::sidecar_path())?;
    let hits: Vec<Hit> = search::search(
        &conn,
        &query,
        limit,
        scope,
        &w,
        recency,
        and,
        directory,
        directory_contains,
        after_ms,
        before_ms,
    )?;

    if !scope.is_empty() {
        let ids: Vec<i64> = hits.iter().map(|h| h.doc_id).collect();
        let n = search::mark_seen(&conn, scope, &ids)?;
        eprintln!("# marked {n} doc(s) as seen in scope '{scope}'");
    }

    if as_json {
        outln!("{}", serde_json::to_string_pretty(&hits)?);
    } else if hits.is_empty() {
        outln!("no hits — say so, do not fill the gap from model memory");
    } else {
        for h in &hits {
            outln!("{}", h.render());
        }
    }
    Ok(())
}

/// `sift extract` — every matching line of one file, plus context. Reads the
/// file only; the sidecar is never opened (no `sift_seen`, no weights, no scope).
/// All content verdicts exit 0 — the tool advises, it does not fail on content.
fn cmd_extract(args: &[String]) -> Result<()> {
    let (positional, flags) = parse_args(args, &["--for", "--context"])?;
    if positional.len() != 1 {
        bail!("extract needs exactly one file\n\n{USAGE}");
    }
    let Some(query) = flags.get("--for") else {
        bail!("extract needs --for <query>\n\n{USAGE}");
    };
    if query.trim().is_empty() {
        bail!("extract needs a non-empty --for <query>\n\n{USAGE}");
    }
    let context = match flags.get("--context") {
        Some(v) => {
            let n: i64 = v
                .parse()
                .with_context(|| format!("--context must be a whole number of lines, got {v:?}"))?;
            if !(0..=extract::MAX_CONTEXT).contains(&n) {
                bail!(
                    "--context must be between 0 and {}, got {n}",
                    extract::MAX_CONTEXT
                );
            }
            n
        }
        None => extract::DEFAULT_CONTEXT,
    };

    let outcome = extract::extract_file(std::path::Path::new(&positional[0]), query, context)?;
    if flags.contains_key("--json") {
        outln!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        outln!("{}", outcome.render());
    }
    Ok(())
}

/// `sift files` — recursive project-directory extraction. Reads files only;
/// no sidecar, no weights, no scope. Skips VCS/build dirs.
fn cmd_files(args: &[String]) -> Result<()> {
    let (positional, flags) = parse_args(args, &["--for", "--context"])?;
    if positional.len() != 1 {
        bail!("files needs exactly one directory\n\n{USAGE}");
    }
    let Some(query) = flags.get("--for") else {
        bail!("files needs --for <query>\n\n{USAGE}");
    };
    if query.trim().is_empty() {
        bail!("files needs a non-empty --for <query>\n\n{USAGE}");
    }
    let context = match flags.get("--context") {
        Some(v) => {
            let n: i64 = v
                .parse()
                .with_context(|| format!("--context must be a whole number of lines, got {v:?}"))?;
            if !(0..=extract::MAX_CONTEXT).contains(&n) {
                bail!(
                    "--context must be between 0 and {}, got {n}",
                    extract::MAX_CONTEXT
                );
            }
            n
        }
        None => extract::DEFAULT_CONTEXT,
    };

    let root = std::path::Path::new(&positional[0]);
    if !root.is_dir() {
        bail!("not a directory: {}", root.display());
    }
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if matches!(name.as_ref(), ".git" | "target" | "node_modules" | ".tokensave") {
                    continue;
                }
                stack.push(path);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut matched = 0usize;
    for path in &files {
        match extract::extract_file(path, query, context) {
            Ok(outcome) => {
                if matches!(outcome.verdict, extract::Verdict::Extracted) {
                    matched += 1;
                    outln!("{}", outcome.render());
                }
            }
            Err(_) => continue,
        }
    }
    outln!("💡 [sift files] {} file(s) with hits", matched);
    Ok(())
}

fn cmd_mark_seen(args: &[String]) -> Result<()> {
    if args.len() < 2 {
        bail!("usage: sift mark-seen <scope> <doc_id...>");
    }
    let scope = &args[0];
    let ids: Result<Vec<i64>> = args[1..]
        .iter()
        .map(|s| s.parse::<i64>().with_context(|| format!("bad doc id {s:?}")))
        .collect();
    let conn = db::open_sidecar(&db::sidecar_path())?;
    let n = search::mark_seen(&conn, scope, &ids?)?;
    outln!("marked {n} doc(s) as seen in scope '{scope}'");
    Ok(())
}

/// Advisory only. `hard = false` never blocks a search — absence of a signal
/// means "not recommended", not "forbidden". See src/trigger.rs.
fn cmd_should_search(args: &[String]) -> Result<()> {
    let (positional, flags) = parse_args(args, &[])?;
    let message = positional.join(" ");
    if message.is_empty() {
        bail!("should-search needs a message\n\n{USAGE}");
    }
    let v = trigger::should_search(&message);

    if flags.contains_key("--json") {
        outln!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        outln!("state: {}", v.state.as_str());
        outln!(
            "hard:  {}",
            if v.hard {
                "yes — positive evidence against a search"
            } else {
                "no — a search is still permitted"
            }
        );
        outln!(
            "terms: {}",
            if v.terms.is_empty() {
                "-".into()
            } else {
                v.terms.join(" ")
            }
        );
        for w in &v.why {
            outln!("  · {w}");
        }
    }
    Ok(())
}

fn load_weights() -> Weights {
    match Weights::discover() {
        Some(p) => match Weights::load(&p) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("sift: {e:#} — using built-in defaults");
                Weights::default()
            }
        },
        None => Weights::default(),
    }
}

/// Split args into positionals and `--flag value` pairs. Bare `--json` style
/// flags end up in the map with an empty value.
fn parse_args(args: &[String], with_value: &[&str]) -> Result<(Vec<String>, HashMap<String, String>)> {
    let mut positional = Vec::new();
    let mut flags = HashMap::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a.starts_with("--") {
            if with_value.contains(&a.as_str()) {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| anyhow::anyhow!("{a} needs a value"))?;
                flags.insert(a.clone(), v.clone());
                i += 2;
            } else {
                flags.insert(a.clone(), String::new());
                i += 1;
            }
        } else {
            positional.push(a.clone());
            i += 1;
        }
    }
    Ok((positional, flags))
}


/// `spur` — self-audit of the agent's own session trace.
///
/// Reads opencode.db read-only. Reports three structural signals of where the
/// agent lost the thread: steps that started but never finished, sessions that
/// ended in error/abort, and loops (same tool+signature repeated >= N times
/// inside one session). No heuristics on text, no FTS5 — pure structure.
pub fn spur_report(limit: i64, min_repeat: i64) -> Result<String> {
    let mut buf = String::new();
    macro_rules! o {
        ($($arg:tt)*) => { let _ = writeln!(buf, $($arg)*); }
    }
    use std::fmt::Write;

    let source = db::source_path();
    o!("spur — self-audit of the agent's own session trace");
    o!("source  {}", source.display());
    o!("");

    let conn = Connection::open(db::source_uri(&source))
        .context("opening source database read-only")?;

    // --- 1. Unpaired step-starts (structural truncation) ---
    o!("=== 1. UNPAIRED STEP-STARTS (structural truncation) ===");
    let (total_starts, total_finishes, unpaired, affected): (i64, i64, i64, i64) = conn.query_row(
        "WITH per_msg AS (
           SELECT message_id,
                  sum(json_extract(data,'$.type')='step-start')  AS s,
                  sum(json_extract(data,'$.type')='step-finish') AS f
           FROM part GROUP BY message_id)
         SELECT COALESCE(sum(s),0), COALESCE(sum(f),0),
                COALESCE(sum(CASE WHEN s>f THEN s-f ELSE 0 END),0),
                count(DISTINCT CASE WHEN s>f THEN message_id END)
         FROM per_msg",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    o!("{total_starts} starts, {total_finishes} finishes -> {unpaired} unpaired, across {affected} messages");
    o!("");
    let mut stmt = conn.prepare(
        "WITH per_msg AS (
           SELECT session_id, message_id,
                  sum(json_extract(data,'$.type')='step-start')  AS s,
                  sum(json_extract(data,'$.type')='step-finish') AS f
           FROM part GROUP BY session_id, message_id)
         SELECT session_id, message_id, s, f, s-f AS gap
         FROM per_msg WHERE s > f
         ORDER BY gap DESC, session_id
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?))
    })?;
    for r in rows {
        let (sid, mid, s, f, gap) = r?;
        o!("  {sid}  {mid}  starts={s} finishes={f} gap={gap}");
    }
    o!("");

    // --- 2. Bad session endings ---
    o!("=== 2. BAD SESSION ENDINGS (error / abort / non-clean) ===");
    let bad_count: i64 = conn.query_row(
        "WITH lastmsg AS (
           SELECT session_id, json_extract(data,'$.role') role,
                  json_extract(data,'$.finish') finish,
                  json_extract(data,'$.error.name') err,
                  ROW_NUMBER() OVER (PARTITION BY session_id ORDER BY time_created DESC) rn
           FROM message)
         SELECT count(*) FROM lastmsg
         WHERE rn=1 AND NOT (role='assistant' AND finish='stop')",
        [], |r| r.get(0),
    )?;
    o!("{bad_count} sessions ended non-clean");
    o!("");
    let mut stmt = conn.prepare(
        "WITH lastmsg AS (
           SELECT session_id, json_extract(data,'$.role') role,
                  json_extract(data,'$.finish') finish,
                  json_extract(data,'$.error.name') err,
                  ROW_NUMBER() OVER (PARTITION BY session_id ORDER BY time_created DESC) rn
           FROM message)
         SELECT session_id, role, COALESCE(finish,'-'), COALESCE(err,'-')
         FROM lastmsg WHERE rn=1 AND NOT (role='assistant' AND finish='stop')
         ORDER BY session_id LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?,
            r.get::<_, String>(2)?, r.get::<_, String>(3)?))
    })?;
    for r in rows {
        let (sid, role, finish, err) = r?;
        o!("  {sid}  role={role} finish={finish} error={err}");
    }
    o!("");

    // --- 3. Loops ---
    o!("=== 3. LOOPS (same tool+signature >= {min_repeat}x in one session) ===");
    let loop_sessions: i64 = conn.query_row(
        "WITH c AS (
           SELECT session_id, json_extract(data,'$.tool') tool,
                  substr(json_extract(data,'$.state.input'),1,120) sig
           FROM part WHERE json_extract(data,'$.type')='tool'),
         g AS (SELECT session_id FROM c GROUP BY session_id, tool, sig HAVING count(*)>=?1)
         SELECT count(DISTINCT session_id) FROM g",
        [min_repeat], |r| r.get(0),
    )?;
    o!("{loop_sessions} sessions contain at least one loop (min-repeat={min_repeat})");
    o!("");
    let mut stmt = conn.prepare(
        "WITH c AS (
           SELECT session_id, json_extract(data,'$.tool') tool,
                  substr(json_extract(data,'$.state.input'),1,120) sig
           FROM part WHERE json_extract(data,'$.type')='tool')
         SELECT session_id, tool, count(*) AS n, sig
         FROM c GROUP BY session_id, tool, sig
         HAVING count(*) >= ?1
         ORDER BY n DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![min_repeat, limit], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?, r.get::<_, String>(3)?))
    })?;
    for r in rows {
        let (sid, tool, n, sig) = r?;
        let short: String = sig.chars().take(80).collect();
        o!("  {sid}  {tool} x{n}  {short}");
    }
    o!("");
    o!("note: repetition in agent traces is not contiguous (adjacent-repeat rate ~6%).");
    o!("      detection is by density within a session, not by run boundaries.");
    Ok(buf)
}


fn cmd_spur(args: &[String]) -> Result<()> {
    let mut limit: i64 = 20;
    let mut min_repeat: i64 = 10;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--limit" => {
                i += 1;
                limit = args.get(i).context("--limit needs a number")?.parse()?;
            }
            "--min-repeat" => {
                i += 1;
                min_repeat = args.get(i).context("--min-repeat needs a number")?.parse()?;
            }
            other => bail!("unknown flag {other:?}\n\n{SPUR_USAGE}"),
        }
        i += 1;
    }

    let report = spur_report(limit, min_repeat)?;
    out!("{}", report);
    Ok(())
}

const SPUR_USAGE: &str = "\
spur — self-audit of the agent's own session trace (read-only on opencode.db)

USAGE
  sift spur [--limit N] [--min-repeat N]

  --limit N       rows per section (default 20)
  --min-repeat N  loop threshold (default 10)
";

#[cfg(test)]
mod tests {
    use super::*;

    /// Read an ms epoch back as local wall-clock time. This is the same engine
    /// the conversion uses, so a pass here proves "the produced number, read as
    /// local time, IS the intended wall clock" on any machine, any timezone.
    fn as_local(ms: i64) -> String {
        let conn = Connection::open_in_memory().unwrap();
        conn.query_row(
            "SELECT strftime('%Y-%m-%d %H:%M:%S', ?1 / 1000, 'unixepoch', 'localtime')",
            [ms],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn after_is_local_midnight_of_the_named_day() {
        assert_eq!(
            as_local(day_bound_ms("2026-06-01", false).unwrap()),
            "2026-06-01 00:00:00"
        );
    }

    #[test]
    fn before_ends_the_named_day_instead_of_starting_it() {
        // The whole point of the `before` bound: a midnight bound would drop
        // everything the caller named. No CET/UTC zone changes its offset on
        // 2026-06-01, so the two bounds are exactly one day minus one
        // millisecond (a zone with a transition on that day would show
        // 86_399_999 ± 3_600_000).
        let start = day_bound_ms("2026-06-01", false).unwrap();
        let end = day_bound_ms("2026-06-01", true).unwrap();
        // strftime prints whole seconds only; the .999 is asserted separately.
        assert_eq!(as_local(end), "2026-06-01 23:59:59");
        assert_eq!(end % 1000, 999);
        assert_eq!(end - start, 86_399_999);
        assert!(end > start);
    }

    #[test]
    fn epoch_anchors_the_conversion() {
        // 1970-01-01 in UTC. Local midnight of that day sits within a day of the
        // epoch in either direction (CET 1970-01-01 was UTC+1, so -3_600_000).
        let start = day_bound_ms("1970-01-01", false).unwrap();
        assert!(start > -86_400_000 && start < 86_400_000, "{start}");
        assert_eq!(
            day_bound_ms("1970-01-02", false).unwrap() - start,
            86_400_000
        );
    }

    #[test]
    fn invalid_dates_are_errors_not_silent_zero() {
        // 0 would mean "filter off" downstream, so a typo must never collapse
        // into it.
        for bad in [
            "",
            "not-a-date",
            "2026-6-1",            // unpadded, rejected here by the shape check
            "2026/06/01",          // wrong separator, rejected here by the shape check
            "2026-06-01 12:00:00", // datetime, not a day
            "2026-13-01",          // month out of range
            "2026-02-30",          // SQLite would silently read this as 2026-03-02
            "2026-06-01-extra",
            "20260601",
        ] {
            assert!(day_bound_ms(bad, false).is_err(), "accepted {bad:?}");
            assert!(
                day_bound_ms(bad, true).is_err(),
                "accepted {bad:?} (end of day)"
            );
        }
    }
}
