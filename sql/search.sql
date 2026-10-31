-- Search: BM25 (FTS5) fused with recency via Reciprocal Rank Fusion, plus the
-- server-side "already seen" filter and a hard token budget via snippet().
--
-- Every tunable lives in a parameter, because weights.toml is what julia/sweep.jl
-- writes. Hardcoding a constant here would make the sweep a lie.
--
-- Parameter order (rusqlite / SQLite) — mirrored in src/search.rs::SEARCH_PARAMS:
--   ?1 MATCH expression       already sanitized, see sanitize_fts_query()
--   ?2 limit                 hits to return, 3-5 is plenty
--   ?3 recency_weight        0.0 = pure BM25, 1.0 = equal weight
--   ?4 scope                 '' disables dedup, otherwise sift_seen scope id
--   ?5 title_weight          bm25 weight for the title column
--   ?6 max_per_session       diversity guard, 2 keeps one session from filling the page
--   ?7 snippet_tokens        snippet() budget in TOKENS (not words)
--   ?8 body_weight           bm25 weight for the body column
--   ?9 rrf_k                 RRF damping constant, 60 is the literature default
--
-- Result filters (SIFT-1). Same rule for all four: an empty/zero value is INACTIVE,
-- so the default search path is byte-identical to the pre-filter behaviour.
--   ?10 directory             exact match on sift_doc.directory, '' = inactive
--   ?11 directory_contains    substring match on sift_doc.directory, '' = inactive
--   ?12 after                 time_created >= ?12 (ms epoch), 0 = inactive
--   ?13 before                time_created <= ?13 (ms epoch), 0 = inactive
--
-- These four live in the `bm` CTE, i.e. BEFORE `top`'s LIMIT 50. Filtering after
-- the over-fetch window would silently return fewer hits than the LIMIT asks for
-- and would rank against documents the caller never wanted. Filter early, then
-- over-fetch, then rank.
--
-- At limit=3 and snippet_tokens=18 a search stays under ~150 tokens of foreign
-- context (measured 114 real LLM tokens). That budget is the whole point of the
-- project. FTS5 tokens are NOT LLM tokens: unicode61 averages ~4 chars/token,
-- an LLM ~2.8, so snippet_tokens=18 yields ~30 real tokens.

WITH bm AS (
  SELECT
    d.id           AS id,
    d.session_id   AS session_id,
    d.title        AS title,
    d.directory    AS directory,
    d.project_id   AS project_id,
    d.kind         AS kind,
    d.tool         AS tool,
    d.time_created AS tc,
    d.body_fp      AS fp,
    bm25(sift_fts, ?5, ?8)                       AS bm,
    -- snippet() MUST be computed in the query carrying the MATCH constraint;
    -- it cannot be deferred to an outer SELECT. Its last argument counts TOKENS.
    snippet(sift_fts, 1, '«', '»', ' … ', ?7)    AS snip
  FROM sift_fts
  JOIN sift_doc d ON d.id = sift_fts.rowid
  WHERE sift_fts MATCH ?1
    AND (?4 = '' OR d.id NOT IN (SELECT doc_id FROM sift_seen WHERE scope = ?4))
    AND (?10 = '' OR d.directory = ?10)
    AND (?11 = '' OR d.directory LIKE '%' || ?11 || '%')
    AND (?12 = 0 OR d.time_created >= ?12)
    AND (?13 = 0 OR d.time_created <= ?13)
),
top AS (
  -- over-fetch: fusion and diversity need more candidates than we return
  SELECT * FROM bm ORDER BY bm LIMIT 50
),
ranked AS (
  SELECT
    top.*,
    ROW_NUMBER() OVER (ORDER BY bm)      AS r_bm,
    ROW_NUMBER() OVER (ORDER BY tc DESC) AS r_rec
  FROM top
),
fused AS (
  SELECT
    ranked.*,
    (1.0 / (?9 + r_bm)) + (?3 / (?9 + r_rec)) AS rrf
  FROM ranked
),
-- Two diversity guards, both motivated by measurement (2026-10-08):
--   snip_rank  : dedup on the SNIPPET, not on the body. Two different documents in
--                two different sessions produced a byte-identical 30-token window
--                (the same diagnostics block). A body fingerprint cannot catch that —
--                what the reader sees twice is the snippet.
--   sess_rank  : without a cap, all three hits came from a single session.
-- Both are hard filters and can starve a small corpus; `top` over-fetches to compensate.
diverse AS (
  SELECT
    fused.*,
    ROW_NUMBER() OVER (PARTITION BY snip       ORDER BY rrf DESC) AS snip_rank,
    ROW_NUMBER() OVER (PARTITION BY session_id ORDER BY rrf DESC) AS sess_rank
  FROM fused
)
SELECT
  d.id            AS doc_id,
  d.session_id    AS session_id,
  d.title         AS session_title,
  d.directory     AS directory,
  d.project_id    AS project_id,
  d.kind          AS kind,
  d.tool          AS tool,
  d.tc            AS time_created,
  d.snip          AS snippet,
  round(d.bm, 4)  AS bm25,
  d.r_bm          AS rank_bm25,
  d.r_rec         AS rank_recency,
  round(d.rrf, 6) AS rrf
FROM diverse d
WHERE d.snip_rank = 1
  AND d.sess_rank <= ?6
ORDER BY d.rrf DESC, d.r_bm ASC
LIMIT ?2;
