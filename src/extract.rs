//! `sift extract` — line-accurate regions from ONE file, no snippet() involved.
//!
//! snippet() is a sampler, not a minimiser: measured 2026-10-08, a document with
//! 9 hits yielded a 205-char snippet containing 1 hit (11%). extract asks a
//! different question — "which LINES match" — and answers it completely: every
//! line becomes one FTS5 row, a MATCH returns every matching rowid, and each
//! rowid expands to a `±context` region. The no-lost-hits property is
//! structural, not a ranking side effect.
//!
//! Fail-closed: when the regions would keep most of the file (`MAX_KEPT_RATIO`)
//! the verdict is `too_broad` and the caller is told to read the whole file
//! instead. A silently truncated picture is worse than no extraction.
//!
//! This module never touches the sidecar: no `sift_seen`, no weights, no scope,
//! no file handle beyond the file being read. It works with `SIFT_DB` pointing
//! at nothing (tested).

use crate::sql;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::Path;

/// Below this many characters, reading the file beats the detour of an
/// extraction round-trip. HEURISTIC, not a measured threshold — the number is a
/// cost intuition and would need its own sweep to become a measurement.
pub const SMALL_FILE_CHARS: usize = 2_000;

/// Hard ceiling on the raw file we are willing to load: 25 MiB = 26_214_400
/// bytes. HEURISTIC: one allocation, and more than any source file an agent
/// should be quoting from.
pub const MAX_FILE_BYTES: usize = 25 * 1024 * 1024;

/// Context lines left and right of every matching line. HEURISTIC: usually
/// enough to see what a bare match line refers to, cheap enough to stay small.
pub const DEFAULT_CONTEXT: i64 = 6;

/// CLI ceiling for `--context`. Above this a "region" is the file and the tool
/// should have said so; the caller can raise this constant if a measurement ever
/// shows otherwise.
pub const MAX_CONTEXT: i64 = 200;

/// At or above this fraction of kept characters, extraction saves too little to
/// justify dropping context. HEURISTIC, not measured.
pub const MAX_KEPT_RATIO: f64 = 0.6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    SmallFile,
    Binary,
    TooLarge,
    NoHits,
    TooBroad,
    Extracted,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Region {
    /// 1-based first line, inclusive.
    pub start: i64,
    /// 1-based last line, inclusive.
    pub end: i64,
    /// Raw region text, lines joined with '\n'.
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub path: String,
    pub query: String,
    pub verdict: Verdict,
    pub total_lines: i64,
    pub matched_lines: i64,
    /// Lines actually present in `regions`; 0 whenever regions are withheld.
    pub kept_lines: i64,
    /// Characters in the would-be regions / characters in the file. This is the
    /// metric the `too_broad` decision is made on, so it is populated even when
    /// the regions themselves are withheld.
    pub kept_ratio: f64,
    pub regions: Vec<Region>,
    /// Character count of the text, for the `small_file` message. Not part of
    /// the JSON contract.
    #[serde(skip)]
    chars: usize,
    /// Raw byte count, for the `too_large` message. Not part of the JSON contract.
    #[serde(skip)]
    bytes: usize,
}

impl Outcome {
    /// Text form. Ends the verdicts' one-liners exactly as specified; the
    /// `extracted` form carries header plus regions. No trailing newline — the
    /// caller prints with println!.
    pub fn render(&self) -> String {
        match self.verdict {
            Verdict::SmallFile => format!(
                "{} chars — too small to extract; read the whole file",
                self.chars
            ),
            Verdict::Binary => "binary file — not text, not extracted".to_string(),
            Verdict::TooLarge => format!(
                "{:.1} MB — too large to extract safely",
                self.bytes as f64 / 1_048_576.0
            ),
            Verdict::NoHits => format!(
                "no hits for \"{}\" — say so, do not fill the gap from model memory",
                self.query
            ),
            Verdict::TooBroad => format!(
                "extraction would keep {:.0}% — read the whole file instead",
                self.kept_ratio * 100.0
            ),
            Verdict::Extracted => {
                let mut out = format!(
                    "💡 [sift extract] {} · {}/{} lines · {} region(s) · kept {:.0}% · for: \"{}\"",
                    self.path,
                    self.matched_lines,
                    self.total_lines,
                    self.regions.len(),
                    self.kept_ratio * 100.0,
                    self.query
                );
                for r in &self.regions {
                    out.push_str(&format!("\n── lines {}-{} ──", r.start, r.end));
                    for (i, line) in r.text.lines().enumerate() {
                        out.push_str(&format!("\n{}: {}", r.start + i as i64, line));
                    }
                }
                out
            }
        }
    }
}

/// Read and classify one file. Real errors (missing file, directory, unreadable,
/// unsearchable query) are `Err`; every content verdict is `Ok`.
pub fn extract_file(path: &Path, query: &str, context: i64) -> Result<Outcome> {
    // fs::read on a directory also fails here, which is the wanted "path is a
    // directory" error.
    let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    extract_bytes(&path.display().to_string(), &raw, query, context)
}

/// The testable core: bytes in, verdict out.
fn extract_bytes(path: &str, raw: &[u8], query: &str, context: i64) -> Result<Outcome> {
    // An unsearchable query is a usage error, not a verdict: it must fail before
    // any guard can mask it as "binary" or "small".
    let match_expr = crate::search::sanitize_fts_query(query, false);
    if match_expr.is_empty() {
        bail!("query contains no searchable terms: {query:?}");
    }

    let mut out = Outcome {
        path: path.to_string(),
        query: query.to_string(),
        verdict: Verdict::Binary,
        total_lines: 0,
        matched_lines: 0,
        kept_lines: 0,
        kept_ratio: 0.0,
        regions: Vec::new(),
        chars: 0,
        bytes: raw.len(),
    };

    // Guard order matters and is specified: NUL/invalid UTF-8 first, then size,
    // then smallness. A NUL byte is what `file` calls binary; invalid UTF-8 is
    // the same statement in Rust's terms.
    if raw.contains(&0) {
        return Ok(out);
    }
    let Ok(text) = std::str::from_utf8(raw) else {
        return Ok(out);
    };
    out.chars = text.chars().count();
    if raw.len() > MAX_FILE_BYTES {
        out.verdict = Verdict::TooLarge;
        return Ok(out);
    }
    out.total_lines = text.lines().count() as i64;
    if out.chars < SMALL_FILE_CHARS {
        out.verdict = Verdict::SmallFile;
        return Ok(out);
    }

    // One in-memory index per call. The sidecar is never opened.
    let lines: Vec<&str> = text.lines().collect();
    let mut conn = Connection::open_in_memory().context("opening in-memory line index")?;
    conn.execute_batch(sql::EXTRACT_LINES)
        .context("creating line_fts")?;
    {
        let tx = conn.transaction()?;
        {
            let mut ins = tx.prepare("INSERT INTO line_fts(rowid, body) VALUES (?1, ?2)")?;
            for (i, line) in lines.iter().enumerate() {
                ins.execute(params![i as i64 + 1, line])?;
            }
        }
        tx.commit()?;
    }

    let hits: Vec<i64> = {
        let mut stmt = conn.prepare(sql::EXTRACT_MATCHES)?;
        let rows = stmt.query_map([&match_expr], |row| row.get(0))?;
        let mut hits = Vec::new();
        for h in rows {
            hits.push(h?);
        }
        hits
    };

    out.matched_lines = hits.len() as i64;
    if hits.is_empty() {
        out.verdict = Verdict::NoHits;
        return Ok(out);
    }

    let spans = merge_regions(&hits, context, out.total_lines);
    let mut regions = Vec::with_capacity(spans.len());
    let mut kept_chars = 0usize;
    let mut kept_lines = 0i64;
    for (start, end) in &spans {
        let region_text = lines[(*start - 1) as usize..*end as usize].join("\n");
        kept_chars += region_text.chars().count();
        kept_lines += end - start + 1;
        regions.push(Region {
            start: *start,
            end: *end,
            text: region_text,
        });
    }
    // Compare emitted characters (regions joined by '\n') to the whole file.
    // Deliberately includes the newlines, so "everything" measures just under
    // 100% instead of just over.
    out.kept_ratio = kept_chars as f64 / out.chars as f64;
    out.kept_lines = kept_lines;

    if out.kept_ratio >= MAX_KEPT_RATIO {
        // Fail-closed: the regions are measured but withheld. Handing the caller
        // 60%+ of a file while pretending it was narrowed is how a truncated
        // picture becomes a wrong answer.
        out.verdict = Verdict::TooBroad;
        out.kept_lines = 0;
        return Ok(out);
    }

    out.verdict = Verdict::Extracted;
    out.regions = regions;
    Ok(out)
}

/// Expand each hit line to `[line - context, line + context]`, clamped to the
/// file, then merge overlapping or directly adjacent spans.
fn merge_regions(hits: &[i64], context: i64, total_lines: i64) -> Vec<(i64, i64)> {
    let mut spans: Vec<(i64, i64)> = hits
        .iter()
        .map(|line| ((line - context).max(1), (line + context).min(total_lines)))
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (start, end) in spans {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 + 1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `total` lines of filler, with the given 1-based line numbers replaced by
    /// a `NEEDLE<line>` marker. Long enough to clear SMALL_FILE_CHARS.
    fn synthetic(total: usize, needle_lines: &[usize]) -> String {
        (1..=total)
            .map(|i| {
                if needle_lines.contains(&i) {
                    format!("NEEDLE{i} {}", "x".repeat(30))
                } else {
                    format!("padding line {i} {}", "y".repeat(30))
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn needles(lines: &[usize]) -> String {
        lines
            .iter()
            .map(|l| format!("NEEDLE{l}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn every_matching_line_survives_into_a_region() {
        // The regression test against the measured snippet() behaviour (1 of 9).
        // 9 hits far apart, query over all of them, every line must be reachable
        // inside some region — not just counted in matched_lines.
        let needle_lines = [20usize, 80, 140, 200, 260, 320, 380, 440, 495];
        let text = synthetic(500, &needle_lines);
        let out = extract_bytes(
            "synthetic.txt",
            text.as_bytes(),
            &needles(&needle_lines),
            DEFAULT_CONTEXT,
        )
        .unwrap();

        assert_eq!(out.verdict, Verdict::Extracted, "{out:?}");
        assert_eq!(out.matched_lines, 9, "{out:?}");
        assert_eq!(out.total_lines, 500, "{out:?}");
        for line in needle_lines {
            let line = line as i64;
            let covered = out.regions.iter().any(|r| {
                r.start <= line
                    && line <= r.end
                    && r.text
                        .lines()
                        .any(|s| s.starts_with(&format!("NEEDLE{line} ")))
            });
            assert!(
                covered,
                "line {line} missing from regions {:?}",
                out.regions
            );
        }
        assert!(out.kept_ratio < MAX_KEPT_RATIO, "{out:?}");
    }

    #[test]
    fn small_file_is_a_verdict_not_an_error() {
        let text = "hello world\n".repeat(10);
        let out = extract_bytes("tiny.txt", text.as_bytes(), "hello", DEFAULT_CONTEXT).unwrap();
        assert_eq!(out.verdict, Verdict::SmallFile, "{out:?}");
        assert!(out.regions.is_empty());
        assert_eq!(
            out.render(),
            format!(
                "{} chars — too small to extract; read the whole file",
                text.chars().count()
            )
        );
    }

    #[test]
    fn binary_is_a_verdict_for_nul_and_for_invalid_utf8() {
        let out = extract_bytes("bin.dat", b"text\0with nul", "text", DEFAULT_CONTEXT).unwrap();
        assert_eq!(out.verdict, Verdict::Binary, "{out:?}");
        assert_eq!(out.render(), "binary file — not text, not extracted");

        let out = extract_bytes("bin.dat", &[0xff, 0xfe, b'a'], "a", DEFAULT_CONTEXT).unwrap();
        assert_eq!(out.verdict, Verdict::Binary, "{out:?}");
    }

    #[test]
    fn oversized_file_is_refused_before_any_search() {
        let raw = vec![b'a'; MAX_FILE_BYTES + 1];
        let out = extract_bytes("huge.txt", &raw, "a", DEFAULT_CONTEXT).unwrap();
        assert_eq!(out.verdict, Verdict::TooLarge, "{out:?}");
        assert!(out.render().contains("25.0 MB"), "{}", out.render());
    }

    #[test]
    fn no_hits_says_so_instead_of_guessing() {
        let text = synthetic(200, &[]);
        let out =
            extract_bytes("plain.txt", text.as_bytes(), "xyzzyquux", DEFAULT_CONTEXT).unwrap();
        assert_eq!(out.verdict, Verdict::NoHits, "{out:?}");
        assert_eq!(out.matched_lines, 0);
        assert!(out.regions.is_empty());
        let rendered = out.render();
        assert!(rendered.contains("\"xyzzyquux\""), "{rendered}");
        assert!(rendered.contains("do not fill the gap"), "{rendered}");
    }

    #[test]
    fn context_is_clamped_at_both_file_edges() {
        let text = synthetic(50, &[1, 50]);
        let out = extract_bytes("edges.txt", text.as_bytes(), &needles(&[1, 50]), 2).unwrap();
        assert_eq!(out.verdict, Verdict::Extracted, "{out:?}");
        assert_eq!(out.regions.len(), 2, "{:?}", out.regions);
        assert_eq!((out.regions[0].start, out.regions[0].end), (1, 3));
        assert_eq!((out.regions[1].start, out.regions[1].end), (48, 50));
    }

    #[test]
    fn overlapping_and_adjacent_hits_merge_into_one_region() {
        let text = synthetic(60, &[10, 12, 15]);
        let out = extract_bytes("merge.txt", text.as_bytes(), &needles(&[10, 12, 15]), 2).unwrap();
        assert_eq!(out.regions.len(), 1, "{:?}", out.regions);
        assert_eq!((out.regions[0].start, out.regions[0].end), (8, 17));

        // context 0 and adjacent hit lines: start == previous end + 1 must merge
        // rather than produce two single-line regions.
        let text = synthetic(60, &[30, 31]);
        let out = extract_bytes("adjacent.txt", text.as_bytes(), &needles(&[30, 31]), 0).unwrap();
        assert_eq!(out.regions.len(), 1, "{:?}", out.regions);
        assert_eq!((out.regions[0].start, out.regions[0].end), (30, 31));
    }

    #[test]
    fn broad_queries_are_refused_rather_than_truncated() {
        // Every line matches: keeping the regions would keep the whole file.
        let text = (1..=200)
            .map(|i| format!("needle every line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.chars().count() > SMALL_FILE_CHARS);
        let out = extract_bytes("broad.txt", text.as_bytes(), "needle", DEFAULT_CONTEXT).unwrap();
        assert_eq!(out.verdict, Verdict::TooBroad, "{out:?}");
        assert!(out.kept_ratio >= MAX_KEPT_RATIO, "{out:?}");
        assert!(out.regions.is_empty(), "regions must be withheld: {out:?}");
        assert!(out.render().contains("read the whole file instead"));
    }

    #[test]
    fn unsearchable_query_is_an_error() {
        let text = synthetic(100, &[]);
        assert!(extract_bytes("x.txt", text.as_bytes(), "... ---", DEFAULT_CONTEXT).is_err());
        assert!(extract_bytes("x.txt", text.as_bytes(), "", DEFAULT_CONTEXT).is_err());
    }

    #[test]
    fn works_without_a_sidecar() {
        // No sidecar may be touched: SIFT_DB points at nothing here, the same
        // environment in which `sift search` would fail to open its database.
        let previous = std::env::var_os("SIFT_DB");
        std::env::set_var("SIFT_DB", "/nonexistent/sift-does-not-exist.db");

        let dir = std::env::temp_dir().join(format!("sift-extract-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.txt");
        std::fs::write(&path, synthetic(100, &[42])).unwrap();

        let out = extract_file(&path, &needles(&[42]), DEFAULT_CONTEXT).unwrap();
        assert_eq!(out.verdict, Verdict::Extracted, "{out:?}");
        assert_eq!(out.matched_lines, 1);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
        match previous {
            Some(v) => std::env::set_var("SIFT_DB", v),
            None => std::env::remove_var("SIFT_DB"),
        }
    }
}
