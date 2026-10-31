-- Extraction v2: source (opencode v2 sessions) -> sidecar.
--
-- Same contract as extract.sql: caller ATTACHes src, runs this, DETACHes.
-- Reads from src.session_message (content stored as JSON array of blocks) and
-- src.session_v2 (title, directory, project_id).
--
-- Verified field paths in session_message.data (2026-10-09):
--   data.content[].type  = text | reasoning | tool
--   text/reasoning       -> data.content[].text
--   tool                 -> data.content[].name  (tool name)
--   tool                 -> data.content[].state.output (capped at 8192)
--   data.content[].state.status = completed | error | running
--
-- part_id is synthesized as session_message.id || ':' || block index, because
-- v2 has no separate part rows. INSERT OR IGNORE keeps it idempotent.

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
    sm.id || ':' || c.key AS id,
    sm.id AS message_id,
    sm.session_id,
    sm.time_created,
    CASE json_extract(c.value, '$.type')
      WHEN 'text'      THEN 'text'
      WHEN 'reasoning' THEN 'reasoning'
      WHEN 'tool'      THEN 'tool'
    END AS kind,
    CASE WHEN json_extract(c.value, '$.type') = 'tool'
      THEN json_extract(c.value, '$.name') END AS tool,
    CASE json_extract(c.value, '$.type')
      WHEN 'text'      THEN json_extract(c.value, '$.text')
      WHEN 'reasoning' THEN json_extract(c.value, '$.text')
      WHEN 'tool'      THEN substr(coalesce(json_extract(c.value, '$.state.output'), ''), 1, 8192)
    END AS body
  FROM src.session_message sm,
       json_each(sm.data, '$.content') c
  WHERE json_extract(c.value, '$.type') IN ('text', 'reasoning', 'tool')
    AND sm.time_created >= coalesce(
      (SELECT CAST(value AS INTEGER) FROM sift_meta WHERE key = 'watermark_ms'), 0)
) x
JOIN src.session_v2 s ON s.id = x.session_id
WHERE x.body IS NOT NULL
  AND length(x.body) > 0;

-- watermark + last_sync are set by extract.sql (runs first). This file only adds rows.
-- Update watermark to include v2 rows.
INSERT INTO sift_meta(key, value)
VALUES ('watermark_ms',
        coalesce((SELECT CAST(max(time_created) AS TEXT) FROM sift_doc), '0'))
ON CONFLICT(key) DO UPDATE SET value = excluded.value;

INSERT INTO sift_meta(key, value)
VALUES ('last_sync', CAST(strftime('%s', 'now') AS TEXT))
ON CONFLICT(key) DO UPDATE SET value = excluded.value;
