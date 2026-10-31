-- Extraction: source -> sidecar.
--
-- Preconditions, the CALLER does this (ATTACH accepts a bound expression, but
-- rusqlite's execute_batch cannot bind parameters, so it cannot live here):
--
--   ATTACH DATABASE 'file:/home/jan/.local/share/opencode/opencode.db?mode=ro' AS src;
--
-- then run this script, then DETACH src.
--
--   sqlite3 ~/.local/share/sift/sift.db
--     "ATTACH DATABASE 'file:$HOME/.local/share/opencode/opencode.db?mode=ro' AS src;" \
--     ".read sql/extract.sql"
--
-- IMPORTANT: no immutable=1. opencode writes to this DB in WAL mode right now;
-- immutable=1 skips WAL/shm and silently returns stale results.
--
-- Verified field paths in opencode's part.data (2026-10-08):
--   type=text       -> $.text          31.552 rows, avg 1721 B, 98% non-empty
--   type=reasoning  -> $.text          50.458 rows, avg 1465 B, 87% non-empty
--   type=tool       -> $.state.output  65.141 rows, avg 2588 B, 99% non-empty
--   type=tool       -> $.tool          bash 28.218 / read 14.130 / edit 9.080 / ...
--   $.output and $.patch are NOT the content paths (0 non-empty) - do not use.
--
-- Not indexed on purpose: step-start, step-finish, agent, compaction (boilerplate),
-- file (88 rows, 35 MB of base64 blobs). patch (8.842) has no text at $.patch and
-- is deferred until someone shows a real use case.

INSERT OR IGNORE INTO sift_doc
  (part_id, message_id, session_id, kind, tool, title, directory, project_id,
   time_created, body, body_fp)
SELECT
  x.id,
  x.message_id,
  x.session_id,
  x.kind,
  x.tool,
  s.title,
  s.directory,
  s.project_id,
  x.time_created,
  x.body,
  length(x.body) || ':' || substr(x.body, 1, 64)
FROM (
  SELECT
    id,
    message_id,
    session_id,
    time_created,
    CASE json_extract(data, '$.type')
      WHEN 'text'      THEN 'text'
      WHEN 'reasoning' THEN 'reasoning'
      WHEN 'tool'      THEN 'tool'
    END AS kind,
    CASE WHEN json_extract(data, '$.type') = 'tool'
      THEN json_extract(data, '$.tool') END AS tool,
    CASE json_extract(data, '$.type')
      WHEN 'text'      THEN json_extract(data, '$.text')
      WHEN 'reasoning' THEN json_extract(data, '$.text')
      -- 8192 chars cap: bash outputs dominate volume (166 MB of ~284 MB total)
      WHEN 'tool'      THEN substr(coalesce(json_extract(data, '$.state.output'), ''), 1, 8192)
    END AS body
  FROM src.part
  WHERE json_extract(data, '$.type') IN ('text', 'reasoning', 'tool')
    -- >= not > : opencode can write several parts with the same ms timestamp.
    -- INSERT OR IGNORE on part_id makes the overlap harmless.
    AND time_created >= coalesce(
      (SELECT CAST(value AS INTEGER) FROM sift_meta WHERE key = 'watermark_ms'), 0)
) x
JOIN src.session s ON s.id = x.session_id
WHERE x.body IS NOT NULL
  AND length(x.body) > 0;

-- coalesce matters: on an empty sidecar max() is NULL and sift_meta.value is NOT NULL.
INSERT INTO sift_meta(key, value)
VALUES ('watermark_ms',
        coalesce((SELECT CAST(max(time_created) AS TEXT) FROM sift_doc), '0'))
ON CONFLICT(key) DO UPDATE SET value = excluded.value;

INSERT INTO sift_meta(key, value)
VALUES ('last_sync', CAST(strftime('%s', 'now') AS TEXT))
ON CONFLICT(key) DO UPDATE SET value = excluded.value;
