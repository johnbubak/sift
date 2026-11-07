-- sift sidecar schema — runs against OUR OWN file (~/.local/share/sift/sift.db)
-- NEVER against ~/.local/share/opencode/opencode.db (5.46 GB, WAL, owned by opencode).
--
-- Design: external-content FTS5 over sift_doc(title, body).
--   - no duplicated body storage (source of truth is sift_doc)
--   - rebuildable via: INSERT INTO sift_fts(sift_fts) VALUES('rebuild')
--   - triggers keep both in sync automatically, so extraction can use
--     INSERT OR IGNORE and stay idempotent across overlapping time windows.
--
-- Triggers here are CORRECT and required. Triggers in opencode.db would be
-- forbidden — that DB is read-only for us.

PRAGMA journal_mode = WAL;
PRAGMA synchronous  = NORMAL;

CREATE TABLE IF NOT EXISTS sift_meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

-- One searchable unit == one source part.
CREATE TABLE IF NOT EXISTS sift_doc (
  id           INTEGER PRIMARY KEY,
  part_id      TEXT    NOT NULL UNIQUE,
  message_id   TEXT    NOT NULL,
  session_id   TEXT    NOT NULL,
  kind         TEXT    NOT NULL,          -- text | reasoning | tool
  tool         TEXT,                      -- tool name, only for kind=tool
  title        TEXT    NOT NULL,          -- session.title, denormalized for boost
  directory    TEXT    NOT NULL,          -- session.directory, 42 distinct
  project_id   TEXT    NOT NULL,          -- session.project_id, 34 distinct
  time_created INTEGER NOT NULL,          -- ms epoch, watermark source
  body         TEXT    NOT NULL,
  -- Fingerprint for result-level dedup. Measured 2026-10-08: 8.8% of bodies are
  -- byte-identical across distinct part_ids (repeated "Edit applied successfully"
  -- / "LSP errors" blocks). sift_seen cannot catch this - it fires across queries,
  -- this duplicates WITHIN one result set.
  -- Prototype approximation: length + first 64 chars. A real 64-bit hash in Rust
  -- replaces this once the indexer is in Rust (see docs/03-phasen.md Phase 1).
  body_fp      TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS sift_doc_session_idx ON sift_doc(session_id);
CREATE INDEX IF NOT EXISTS sift_doc_time_idx    ON sift_doc(time_created);
CREATE INDEX IF NOT EXISTS sift_doc_kind_idx    ON sift_doc(kind);
CREATE INDEX IF NOT EXISTS sift_doc_dir_idx     ON sift_doc(directory);

-- unicode61 + remove_diacritics 2. Trigram is deliberately NOT the default:
-- it indexes every 3-char window (~3-5x index size) and cannot match terms
-- shorter than 3 chars. Prefix queries (term*) buy most of the recall we need.
-- Revisit only with a measurement, see docs/03-phasen.md Phase 5.
CREATE VIRTUAL TABLE IF NOT EXISTS sift_fts USING fts5(
  title,
  body,
  content    = 'sift_doc',
  content_rowid = 'id',
  tokenize   = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS sift_doc_ai AFTER INSERT ON sift_doc BEGIN
  INSERT INTO sift_fts(rowid, title, body) VALUES (new.id, new.title, new.body);
END;

CREATE TRIGGER IF NOT EXISTS sift_doc_ad AFTER DELETE ON sift_doc BEGIN
  INSERT INTO sift_fts(sift_fts, rowid, title, body)
  VALUES ('delete', old.id, old.title, old.body);
END;

CREATE TRIGGER IF NOT EXISTS sift_doc_au AFTER UPDATE ON sift_doc BEGIN
  INSERT INTO sift_fts(sift_fts, rowid, title, body)
  VALUES ('delete', old.id, old.title, old.body);
  INSERT INTO sift_fts(rowid, title, body) VALUES (new.id, new.title, new.body);
END;

-- Anti-redundanz. The agent passes ONE scope id per conversation and never has
-- to ship id lists around; the filter lives here, server-side, at zero token cost.
CREATE TABLE IF NOT EXISTS sift_seen (
  scope   TEXT    NOT NULL,
  doc_id  INTEGER NOT NULL,
  seen_at INTEGER NOT NULL,
  PRIMARY KEY (scope, doc_id)
) WITHOUT ROWID;

-- Phase A — Git history (own table + watermark per repo).
CREATE TABLE IF NOT EXISTS git_repo (
  id        INTEGER PRIMARY KEY,
  url       TEXT    NOT NULL UNIQUE,
  name      TEXT    NOT NULL,
  path      TEXT    NOT NULL,
  host      TEXT,
  last_sync INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS git_doc (
  id       INTEGER PRIMARY KEY,
  repo_id  INTEGER NOT NULL REFERENCES git_repo(id) ON DELETE CASCADE,
  kind     TEXT    NOT NULL,   -- commit | issue | pr
  ref      TEXT    NOT NULL,
  author   TEXT,
  ts       INTEGER NOT NULL,
  title    TEXT    NOT NULL,
  body     TEXT    NOT NULL,
  body_fp  TEXT    NOT NULL,
  UNIQUE(repo_id, kind, ref)
);

CREATE VIRTUAL TABLE IF NOT EXISTS git_fts USING fts5(
  title, body, content='git_doc', content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS git_doc_ai AFTER INSERT ON git_doc BEGIN
  INSERT INTO git_fts(rowid, title, body) VALUES (new.id, new.title, new.body);
END;
CREATE TRIGGER IF NOT EXISTS git_doc_ad AFTER DELETE ON git_doc BEGIN
  INSERT INTO git_fts(git_fts, rowid, title, body)
  VALUES ('delete', old.id, old.title, old.body);
END;
CREATE TRIGGER IF NOT EXISTS git_doc_au AFTER UPDATE ON git_doc BEGIN
  INSERT INTO git_fts(git_fts, rowid, title, body)
  VALUES ('delete', old.id, old.title, old.body);
  INSERT INTO git_fts(rowid, title, body) VALUES (new.id, new.title, new.body);
END;

-- Phase B — Feedback audit log (rate limit + no content).
CREATE TABLE IF NOT EXISTS feedback_log (
  id      INTEGER PRIMARY KEY,
  ts      INTEGER NOT NULL,
  payload TEXT    NOT NULL
);
