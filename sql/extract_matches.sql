-- Every matching line, in file order.
--
-- rowid is the 1-based line number assigned at insert time, so the caller maps
-- hits straight to regions. Complete by construction: no LIMIT, no threshold,
-- no snippet() sampling — that is the whole point of this index.
-- ?1 = sanitized MATCH expression, see search::sanitize_fts_query().
SELECT rowid FROM line_fts WHERE line_fts MATCH ?1 ORDER BY rowid;
