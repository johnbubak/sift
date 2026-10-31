-- `sift extract` line index: one FTS5 row per LINE, not per document.
--
-- Why line rows and not another document index: snippet() is a sampler, not a
-- minimiser. Measured 2026-10-08: a document with 9 hits produced a 205-char
-- snippet that contained 1 of those 9 hits (11%). A caller reasoning from that
-- snapshot concludes the other 8 hits do not exist. Here every line is its own
-- row, rowid = 1-based line number, and a MATCH returns EVERY matching line.
-- The no-lost-hits property is structural: it is how the table is built, not a
-- ranking trick.
--
-- Runs on a throwaway Connection::open_in_memory() per extract call and is
-- never attached to the sidecar (no sift_seen, no weights, no scope).
-- Tokenizer mirrors sift_fts in migrations/001_sidecar.sql so a term behaves
-- the same in both indexes.
CREATE VIRTUAL TABLE line_fts USING fts5(
  body,
  tokenize = 'unicode61 remove_diacritics 2'
);
