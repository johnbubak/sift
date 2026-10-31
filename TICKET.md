# TICKET — Spur: Selbstaudit der Agent-Spur

**Name.** *Spur* (Track/Trail). Bringt drei Wege unter einen Hut:
- **sift** — Session-Recall: „was habe ich damals gesagt" (statistisch, pointer)
- **flowhound** — Exakter Variablen-Fluss durch Code (gerichtet, verifizierend)
- **Spur** — Selbstaudit der Agent-Spur: „wo habe ich den Faden verloren"
  (strukturell exakt für Trunkierung/Loops, statistisch für Drift)

**Status:** Iteration 0 implementiert (`sift spur`, 2026-10-09). Iterationen 1–3 offen.
**Priorität:** hoch · **Abhängigkeiten:** opencode.db (read-only, `mode=ro`)

---

## Kernprinzip

Spur ist **kein** codemap und **kein** exakter Verifier. Es ist ein
Aufmerksamkeitslenker: es zeigt, *wo* etwas sein könnte, und die KI entscheidet.
Ungewissheit wird getragen, nicht versteckt.

Flowhound kann `NOT NULL` sagen, Spur kann nur `vielleicht`. Deshalb tragen
Spur-Ausgaben Ungewissheit sichtbar im Schema (z. B. „candidate, not verdict").

## Iteration 0 — implementiert (`sift spur`)

Drei exakte Signale gegen `opencode.db`, read-only, ohne Heuristik:

1. **Unpaired step-starts** — strukturelle Trunkierung: Schritt begann, wurde
   nie beendet. `step-start` ohne passendes `step-finish` im selben Message.
2. **Bad session endings** — Sessions, deren letzter Message in Fehler, Abbruch
   oder leer endet (nicht `assistant`/`stop`).
3. **Loops** — gleiche Tool+Signatur ≥ N× in einer Session. Erkennung über
   Dichte (`GROUP BY session, tool, sig HAVING count >= N`), nicht über
   Adjazenz — Wiederholung im Agent-Verhalten ist nicht kontingent (gemessen:
   Adjacent-Repeat-Rate ~6 %, Dichte-Erkennung findet 62/1.575 Sessions).

**Befund (gemessen 2026-10-09):** 562 ungepaarte Starts / 503 Messages,
159 Bad Endings, 64 Loop-Sessions (bis 337× dieselbe Signatur).

**Einstieg:** `sift spur --limit 20 --min-repeat 10`. Kein Rust-Schema-Change,
kein neues Binary, keine neuen Tabellen — reines SQL über Daten, die sift
bereits liest.

## Iteration 1 — Dichte statt Fluss (offen)

- Was ist ein *guter* Loop? `grep -n 'foo'` 20× ist normaler Debugging-Fluch,
  kein Vergehen. Ohne Labels nur eine Zahl.
- Schwellenwert-Kalibrierung: `--min-repeat` braucht einen gemessenen
  Default, nicht Intuition (Precision aus Rareheit, nicht aus Intuition —
  wie bei Trigger-Wörtern in `src/trigger.rs`).
- Scope-Filter: `--session`, `--since`, `--directory` analog zu `sift search`.

## Iteration 2 — Shingle-Jaccard über `text`-Parts (offen, teuer)

Echte Wiederholungs-*Rede* statt Wiederholungs-*Handlungen*. Braucht:
- Plain-Table `(shingle, doc)` neben FTS5 (FTS5 kann Jaccard nicht).
- Kandidaten-Filter über `body_fp` — aber `body_fp` ist laut `AGENTS.md`
  weiterhin eine Heuristik (`length + 64 Zeichen`), kein Hash.
- Iteration über Falsch-Positive, weil „vergessen" keine Ground-Truth hat.

## Iteration 3 — MCP-Exposition (offen)

Spur als MCP-Tool, damit opencode es direkt aufrufen kann. Abhängig von
Phase 2 (rmcp) in `docs/03-phasen.md`.

---

## Der Konflikt mit `sift_seen`

`sift_seen` macht serverseitig unsichtbar, was schon gesehen wurde —
Anti-Redundanz als Kernprinzip. Spur lebt von genau dem, was `sift_seen`
versteckt. Vermutlich braucht Spur einen eigenen Marker (`noted`, nicht `seen`),
sonst frisst es sich selbst.

## Offene Tickets (konsolidiert)

### SIFT-2 — MCP-Server (`rmcp`)
Phase 2 in `docs/03-phasen.md`. `rmcp 3.5.1` war beim Schreiben 3 Tage alt.
Einstieg: stdio-Transport, `sift search` + `sift spur` als Tools.

### SIFT-3 — Dedup-Vollständigkeit
`sift_seen` merkt `doc_id`, nicht Snippet-Fingerprint → Zwillings-Snippets
rücken im nächsten Durchlauf nach (beobachtet: `doc 2694` → `doc 2693`).
Fix: `body_fp` zusätzlich speichern. Aber: `body_fp` ändert erst dann etwas,
wenn er auch im Query benutzt wird (`search.sql`).

### SIFT-4 — `--directory` / `--directory-contains` Filter
Implementiert in SIFT-1. Verifizieren, dass leere Werte Filter inaktiv lassen.

### SIFT-5 — `--after` / `--before` Datumsfilter
Implementiert in SIFT-1. `day_bound_ms` in `main.rs`.

### SIFT-6 — Skill `~/.config/opencode/skills/sift/SKILL.md`
Live seit 2026-10-08. Spur-Skill ergänzen: wann `sift spur` statt `sift search`.

### SIFT-7 — `should-search` Trigger-Erkener
Fail-open. `hard = true` nur bei Verneinung oder Zitat. Jedes Signal durch
`evaluate()`. 38 Tests grün.

### SIFT-8 — Claude-Code-Transcript-Import
Archiv-Import (Claude schreibt seit 2026-05-17 nichts mehr). 63 Top-Level-Keys,
`message.content` String oder Blockliste. Schema-Haken: `title TEXT NOT NULL`,
Fallback auf Verzeichnisnamen. Reihenfolge nach Signaldichte: memory+plans+
paste-cache (106 Dateien, 1,46 MB) → Transcripts (17,3 MB) → Downloads (Filter).

### SIFT-9 — `gaps`-Kommando
Observability-Gaps aus flowhound übernehmen: welche Felder haben kein
SDB-Gegenstück. Als Spur-Signal: welche Variablen erreichen keine Senke.

### SIFT-10 — v2-Sessions / Compaction ✅ (2026-10-09)
`session_v2` + `session_message` werden jetzt indexiert (`sql/extract_v2.sql`,
`src/sql.rs::EXTRACT_V2`, `src/index.rs`). Content wird als JSON-Array von
Blöcken (text/reasoning/tool) unnested via `json_each(data, '$.content')`.
`part_id` = `session_message.id || ':' || block-index`. 14.213 v2-Docs
extrahiert (Full Rebuild). `compaction`-Parts (159 gemessen) als
Session-Grenzen. `session_context_epoch` als Epoch-Marker. Spur kann
Compaction als „Neuanfang"-Signal nutzen.

---

## Harte Regeln (unverändert)

- `opencode.db` ist read-only. `mode=ro`, niemals `immutable=1`.
- SQL bleibt in Dateien (`src/sql.rs` bindet per `include_str!`).
- `verify-sql.sh` patcht `extract.sql` per `sed` auf `AND length(x.body) > 0;`.
- `body_fp` speist keine Deduplizierung, solange er nicht im Query benutzt wird.
- `cargo fmt --check` ist nicht grün (18 Diffs) — nicht pauschal aufräumen.
- Warnungen bei `cargo build` gelten als Fehler.

## Verifikation

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test                      # 38 Tests, grün
cargo build --release           # 0 Warnungen
./target/release/sift spur --limit 5
./target/release/sift status
```

Ein Feature gilt erst als fertig, wenn das zugehörige Kommando grün ist.
