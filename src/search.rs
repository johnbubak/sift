use crate::sql;
use crate::weights::Weights;
use anyhow::{bail, Result};
use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub doc_id: i64,
    pub session_id: String,
    pub session_title: String,
    pub directory: String,
    pub project_id: String,
    pub kind: String,
    pub tool: Option<String>,
    pub time_created: i64,
    pub snippet: String,
    pub bm25: f64,
    pub rank_bm25: i64,
    pub rank_recency: i64,
    pub rrf: f64,
}

impl Hit {
    /// One hit, two lines, provenance plus content. This is the token budget:
    /// `snippet_tokens` of content and no more, never the full body.
    pub fn render(&self) -> String {
        let origin = match &self.tool {
            Some(t) if !t.is_empty() => format!("{}/{}", self.kind, t),
            _ => self.kind.clone(),
        };
        format!(
            "💡 [sift] {} · {} · {} · {}\n   {}",
            self.directory,
            self.session_title,
            origin,
            format_ms(self.time_created),
            self.snippet
        )
    }
}

pub fn format_ms(ms: i64) -> String {
    // Minimal civil-from-days conversion; avoids pulling in chrono for one line.
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        tod / 3600,
        (tod % 3600) / 60
    )
}

/// Howard Hinnant's civil_from_days.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Turn arbitrary user text into something FTS5 will accept.
///
/// Free text is not a query: `"foo-bar"`, `NEAR`, `*`, `:`, `(`, `AND` and a
/// stray quote all produce syntax errors, and `" AND ".join(terms)` crashes on
/// quotes. Every term is stripped down to code-safe characters, double-quoted,
/// and gets a `*` prefix when it is at least 3 characters long.
///
/// Terms are OR-ed by default. Measured 2026-10-08 on the real corpus:
///
/// | query                          | AND | OR |
/// |---|---|---|
/// | jwt cache invalidierung        |   1 | 500+ |
/// | fts5 trigram tokenizer         |  26 | 500+ |
/// | surrealdab async integration   | 147 | 500+ |
///
/// Conjunction returns "no hits" for perfectly reasonable recall queries, because
/// a single remembered detail rarely sits in the same part as all the others.
/// OR is safe here: bm25 already ranks documents matching more terms higher, so
/// precision comes from the ranking rather than from the boolean glue. `--and`
/// restores strict conjunction for callers who need it.
///
/// Known limitation: `"a b"` is not a phrase — quotes are stripped and it becomes
/// two terms. Acceptable, and documented rather than silently surprising.
pub fn sanitize_fts_query(input: &str, and: bool) -> String {
    let sep = if and { " " } else { " OR " };
    let mut terms: Vec<String> = Vec::new();
    for raw in input.split_whitespace() {
        let cleaned: String = raw
            .chars()
            .filter(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | ':' | '#' | '+' | '@'))
            .collect();
        if cleaned.is_empty() || !cleaned.chars().any(|c| c.is_alphanumeric()) {
            // A token made only of allowed punctuation ("...", "->", "../..")
            // would survive the filter and become a nonsense quoted term.
            continue;
        }
        if cleaned.chars().count() >= 3 {
            terms.push(format!("\"{cleaned}\"*"));
        } else {
            terms.push(format!("\"{cleaned}\""));
        }
    }
    terms.join(sep)
}

/// One search. The parameter list mirrors the `?1..?13` binding order of
/// sql/search.sql exactly — that is why it is a flat argument list and not a
/// struct: a mistake here is not a compile error, it is silently wrong results.
/// Keep both sides in lockstep (and re-run scripts/verify-sql.sh).
///
/// `directory` / `directory_contains` are inactive when empty, `after_ms` /
/// `before_ms` when 0 — the same "empty means off" rule the SQL uses, kept on
/// the Rust side so the two can never disagree about what "no filter" means.
#[allow(clippy::too_many_arguments)]
pub fn search(
    conn: &Connection,
    query: &str,
    limit: i64,
    scope: &str,
    w: &Weights,
    recency_override: Option<f64>,
    and: bool,
    directory: &str,
    directory_contains: &str,
    after_ms: i64,
    before_ms: i64,
) -> Result<Vec<Hit>> {
    let match_expr = sanitize_fts_query(query, and);
    if match_expr.is_empty() {
        bail!("query contains no searchable terms: {query:?}");
    }
    let recency = recency_override.unwrap_or(w.recency_weight);

    let mut stmt = conn.prepare(sql::SEARCH)?;
    let hits = stmt.query_map(
        params![
            match_expr,         // ?1
            limit,              // ?2
            recency,            // ?3
            scope,              // ?4
            w.title_weight,     // ?5
            w.max_per_session,  // ?6
            w.snippet_tokens,   // ?7
            w.body_weight,      // ?8
            w.rrf_k,            // ?9
            directory,          // ?10
            directory_contains, // ?11
            after_ms,           // ?12
            before_ms,          // ?13
        ],
        |row| {
            Ok(Hit {
                doc_id: row.get(0)?,
                session_id: row.get(1)?,
                session_title: row.get(2)?,
                directory: row.get(3)?,
                project_id: row.get(4)?,
                kind: row.get(5)?,
                tool: row.get(6)?,
                time_created: row.get(7)?,
                snippet: row.get(8)?,
                bm25: row.get(9)?,
                rank_bm25: row.get(10)?,
                rank_recency: row.get(11)?,
                rrf: row.get(12)?,
            })
        },
    )?;

    let mut out = Vec::new();
    for h in hits {
        out.push(h?);
    }
    Ok(out)
}

/// Record what was served, so the same content is not paid for twice.
/// This is what makes the scope worth passing instead of id lists.
pub fn mark_seen(conn: &Connection, scope: &str, ids: &[i64]) -> Result<usize> {
    if scope.is_empty() {
        bail!("scope must not be empty");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut n = 0;
    for id in ids {
        n += conn.execute(
            "INSERT OR IGNORE INTO sift_seen(scope, doc_id, seen_at) VALUES (?1, ?2, ?3)",
            params![scope, id, now],
        )?;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizer_neutralises_fts5_operators() {
        // Quoting is the point: an operator the user typed becomes a literal
        // search term rather than a syntax error. It does not have to vanish.
        let q = sanitize_fts_query("foo-bar NEAR \"x\" (y) a* :b", false);
        assert!(q.contains("\"foo-bar\"*"), "{q}");
        assert!(q.contains("\"NEAR\"*"), "{q}");
        assert!(!q.contains(" NEAR "), "bare operator would be parsed: {q}");
        assert!(!q.contains('('), "{q}");
        assert!(q.contains("\"x\""), "{q}");
    }

    #[test]
    fn punctuation_only_tokens_are_dropped() {
        // "." and "-" are in the allowed set, so "..." and "../.." would
        // otherwise become literal search terms.
        assert!(sanitize_fts_query("--- ...", false).is_empty());
        assert!(sanitize_fts_query("../.. ->", false).is_empty());
        assert_eq!(sanitize_fts_query("a ... b", false), "\"a\" OR \"b\"");
    }

    #[test]
    fn short_terms_get_no_prefix_star() {
        let q = sanitize_fts_query("ab abc", false);
        assert_eq!(q, "\"ab\" OR \"abc\"*", "{q}");
    }

    #[test]
    fn or_is_the_default_and_and_is_opt_in() {
        assert_eq!(
            sanitize_fts_query("jwt cache invalidierung", false),
            "\"jwt\"* OR \"cache\"* OR \"invalidierung\"*"
        );
        assert_eq!(
            sanitize_fts_query("jwt cache invalidierung", true),
            "\"jwt\"* \"cache\"* \"invalidierung\"*"
        );
    }

    #[test]
    fn punctuation_only_query_yields_empty() {
        assert!(sanitize_fts_query("--- ... ***", false).is_empty());
    }

    #[test]
    fn civil_from_days_matches_known_epochs() {
        assert_eq!(format_ms(0), "1970-01-01 00:00");
        assert_eq!(format_ms(1_000_000_000_000), "2001-09-09 01:46");
    }

    /// Throwaway sidecar with three docs: two in /repo/a (one a week later than
    /// the other), one in /repo/b. Distinct sessions and distinct bodies, so the
    /// diversity filters cannot be mistaken for a filter under test.
    fn filter_fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::sql::MIGRATION).unwrap();
        let day = 86_400_000;
        let rows = [
            ("p1", "s1", "/repo/a", "fts5 alpha", 1_800_000_000_000i64),
            ("p2", "s2", "/repo/b", "fts5 beta", 1_800_000_000_000),
            (
                "p3",
                "s3",
                "/repo/a",
                "fts5 gamma",
                1_800_000_000_000 + 7 * day,
            ),
        ];
        for (part, session, dir, body, tc) in rows {
            conn.execute(
                "INSERT INTO sift_doc(part_id, message_id, session_id, kind, title, directory,
                                      project_id, time_created, body, body_fp)
                 VALUES (?1, ?1, ?2, 'text', 't', ?3, 'p', ?4, ?5, ?5)",
                rusqlite::params![part, session, dir, tc, body],
            )
            .unwrap();
        }
        conn
    }

    fn filtered(
        conn: &Connection,
        directory: &str,
        directory_contains: &str,
        after_ms: i64,
        before_ms: i64,
    ) -> Vec<Hit> {
        search(
            conn,
            "fts5",
            10,
            "",
            &Weights::default(),
            None,
            false,
            directory,
            directory_contains,
            after_ms,
            before_ms,
        )
        .unwrap()
    }

    #[test]
    fn empty_filters_are_inactive() {
        // The default path must stay byte-identical to the pre-filter behaviour:
        // empty string / 0 means "no filter", never "match nothing".
        let conn = filter_fixture();
        let hits = filtered(&conn, "", "", 0, 0);
        assert_eq!(hits.len(), 3, "{hits:?}");
        let mut dirs: Vec<&str> = hits.iter().map(|h| h.directory.as_str()).collect();
        dirs.sort_unstable();
        dirs.dedup();
        assert_eq!(dirs, ["/repo/a", "/repo/b"], "{dirs:?}");
    }

    #[test]
    fn directory_filters_narrow_the_candidate_set() {
        let conn = filter_fixture();
        let a = filtered(&conn, "/repo/a", "", 0, 0);
        assert_eq!(a.len(), 2, "{a:?}");
        assert!(a.iter().all(|h| h.directory == "/repo/a"));

        let b = filtered(&conn, "", "repo/b", 0, 0);
        assert_eq!(b.len(), 1, "{b:?}");
        assert_eq!(b[0].directory, "/repo/b");

        // No overlap: an exact match does not also match a substring.
        assert!(filtered(&conn, "/repo", "", 0, 0).is_empty());
    }

    #[test]
    fn time_filters_are_inclusive_bounds() {
        let conn = filter_fixture();
        let day = 86_400_000;
        let t0 = 1_800_000_000_000;

        // after = start of a day INSIDE the corpus: only the newer doc survives.
        let after = filtered(&conn, "", "", t0 + day, 0);
        assert_eq!(after.len(), 1, "{after:?}");
        assert_eq!(after[0].time_created, t0 + 7 * day);

        // before = end of the SAME window: the two older docs survive, the newer
        // one does not. before=0 and after=0 stay inactive.
        let before = filtered(&conn, "", "", 0, t0 + day - 1);
        assert_eq!(before.len(), 2, "{before:?}");

        // Both bounds together = an empty window, which is an empty result, not
        // an ignored filter.
        assert!(filtered(&conn, "", "", t0 + 2 * day, t0 + day).is_empty());
    }
}
