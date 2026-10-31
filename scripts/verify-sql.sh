#!/usr/bin/env bash
# End-to-end proof of the sift sidecar pipeline against the live opencode DB.
# Uses a throwaway sidecar and a bounded extract, so it is safe to run at any time.
#
#   scripts/verify-sql.sh [rows] [term]   (default 4000 rows, term "error")
#
# Proves: schema -> extract -> FTS sync -> search -> dedup across queries.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROWS="${1:-4000}"
TERM="${2:-error}"
SOURCE="${SIFT_SOURCE:-$HOME/.local/share/opencode/opencode.db}"
SOURCE_URI="file:${SOURCE}?mode=ro"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
DB="$TMP/sift.db"

[ -f "$SOURCE" ] || { echo "FAIL: source DB not found: $SOURCE"; exit 1; }

# Bound extract: keep the LIMIT for a fast run. ATTACH is the caller's job
# (rusqlite's execute_batch cannot bind parameters), so it wraps the script here.
sed "s/AND length(x.body) > 0;/AND length(x.body) > 0\nLIMIT ${ROWS};/" \
  "$ROOT/sql/extract.sql" > "$TMP/extract-bounded.sql"
{
  printf "ATTACH DATABASE '%s' AS src;\n" "$SOURCE_URI"
  cat "$TMP/extract-bounded.sql"
  printf "DETACH DATABASE src;\n"
} > "$TMP/run-extract.sql"

# search <scope> [directory] [directory_contains] [after_ms] [before_ms] -> JSON
# on stdout. Snippets contain newlines, so list mode cannot be cut into fields
# reliably; .mode json escapes them properly.
#
# The four optional arguments feed ?10..?13. They default to the inactive values
# ('' and 0), so the two original calls keep their exact old behaviour. The order
# is pinned by hand here and must stay in lockstep with sql/search.sql — a
# missing line is not a compile error, it is a silently wrong filter.
search() {
  local scope="$1"
  local directory="${2-}" directory_contains="${3-}" after="${4-0}" before="${5-0}"
  local out="$TMP/q.sql"
  cat > "$out" <<SQL
.parameter init
.parameter set ?1 '${TERM}'
.parameter set ?2 5
.parameter set ?3 0.3
.parameter set ?4 '${scope}'
.parameter set ?5 1.0
.parameter set ?6 2
.parameter set ?7 30
.parameter set ?8 3.0
.parameter set ?9 60.0
.parameter set ?10 '${directory}'
.parameter set ?11 '${directory_contains}'
.parameter set ?12 ${after}
.parameter set ?13 ${before}
.mode json
.read ${ROOT}/sql/search.sql
SQL
  sqlite3 "$DB" ".read $out"
}

doc_ids() { grep -o '"doc_id":[0-9]*' <<<"$1" | cut -d: -f2 | paste -sd' ' -; }
show()    { sed 's/\\n/\n/g' <<<"$1" | cut -c1-110; }

echo "==> migration"
sqlite3 "$DB" ".read $ROOT/migrations/001_sidecar.sql" >/dev/null

echo "==> extract ($ROWS rows, read-only ATTACH of $SOURCE)"
sqlite3 "$DB" ".read $TMP/run-extract.sql"

DOCS=$(sqlite3 "$DB" "SELECT count(*) FROM sift_doc;")
FTS=$(sqlite3 "$DB" "SELECT count(*) FROM sift_fts;")
echo "==> docs=$DOCS fts=$FTS"
[ "$DOCS" = "$FTS" ] || { echo "FAIL: FTS out of sync ($FTS vs $DOCS)"; exit 1; }

echo
echo "==> search #1 (scope = '')"
RUN1="$(search '')"
RUN1_IDS="$(doc_ids "$RUN1")"
[ -n "$RUN1_IDS" ] || { echo "FAIL: no hits for '$TERM' in $ROWS rows - pass a different term"; exit 1; }
show "$RUN1"

echo
echo "==> marking returned doc_ids as seen: $RUN1_IDS"
UNION=""
for id in $RUN1_IDS; do
  UNION="${UNION:+$UNION UNION ALL }SELECT $id AS doc_id"
done
sqlite3 "$DB" "INSERT OR IGNORE INTO sift_seen(scope, doc_id, seen_at)
               SELECT 'demo', doc_id, 0 FROM ($UNION);"
sqlite3 "$DB" "SELECT '  seen rows: ' || count(*) FROM sift_seen WHERE scope='demo';"

echo
echo "==> search #2 (scope = 'demo') - those doc_ids must be gone"
RUN2="$(search demo)"
RUN2_IDS="$(doc_ids "$RUN2")"
[ -n "$RUN2_IDS" ] && show "$RUN2" || echo "  (nothing left - dedup removed every candidate)"

LEAK=0
for id in $RUN1_IDS; do
  case " $RUN2_IDS " in *" $id "*) LEAK=$((LEAK+1)) ;; esac
done

echo
if [ "$LEAK" -gt 0 ]; then
  echo "FAIL: $LEAK already-seen doc(s) still served under scope=demo"
  exit 1
fi

# ?10..?13 smoke test. Scope='' keeps the dedup table out of the picture, and the
# three probes are corpus-independent: a directory nobody has, a lower bound in
# the far future (every doc is older), an upper bound before any doc exists.
# A dropped .parameter line in search() must make one of them fail loudly.
#
# Exit status is checked too: `.mode json` prints nothing at all for an empty
# result, and it prints nothing on stdout for an SQL error as well — so "no rows"
# alone cannot tell "the filter worked" from "the query never ran".
FILT=0
probe_filter() {
  local label="$1" rc=0 out
  shift
  out="$(search "$@")" || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "FAIL: $label — search exited $rc (SQL error: the filter never ran)"
    FILT=1
  elif [ -n "$(doc_ids "$out")" ]; then
    echo "FAIL: $label — filter returned rows"
    FILT=1
  fi
}
probe_filter "?10 directory"       '' /no/such/directory '' 0 0
probe_filter "?12 after"           '' '' '' 99999999999999 0
probe_filter "?13 before"          '' '' '' 0 1
[ "$FILT" -eq 0 ] || exit 1

echo
echo "PASS: FTS in sync ($DOCS docs); 0 of [$RUN1_IDS] leaked into scope=demo"
echo "PASS: ?10/?12/?13 filters applied (3 negative probes: 0 rows, exit 0)"
