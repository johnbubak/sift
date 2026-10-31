<!--
Startprompt für die nächste Session in /home/jan/projects/sift.
Einfügen und den Auftrag dahinter ersetzen.
-->

Du arbeitest in `/home/jan/projects/sift`. Lies zuerst, in dieser Reihenfolge:

1. `AGENTS.md` — die Regeln dieses Repos
2. `docs/00-archeologie.md` — gemessene Fakten über die opencode-DB
3. `docs/01-architektur.md` — Sidecar, Ranking, Dedup, Sanitizer
4. `docs/02-adr.md` — Entscheidungen **inklusive der verworfenen Wege**
5. `docs/03-phasen.md` — aktueller Stand, offene Punkte

## Was sift ist

Lokale Wissens-Wiederverwendung für Coding-Agenten. Die KI ist der **Konsument**
von Wissen, nicht die Suchmaschine: Gesucht wird ausschließlich in SQLite
(FTS5 + BM25 + RRF), in den Prompt gelangen nur ~30-Token-Snippets mit
Herkunftsangabe. Zwei Ziele, die alles andere bestimmen:

* **Token-Budget:** `snippet(…, 30)` zählt Tokens, nicht Wörter. Drei Treffer
  ≈ 150 Tokens Fremdkontext.
* **Anti-Redundanz:** `sift_seen(scope, doc_id)` merkt serverseitig, was bereits
  ausgeliefert wurde. Der Agent übergibt **eine** Scope-ID pro Conversation,
  keine ID-Listen.

## Stand (2026-10-08)

Fertig und **gemessen**:

* Sidecar `~/.local/share/sift/sift.db`, 462 MB, 139.493 Docs, FTS synchron
* `sift build` → 139.493 Docs in **20,2 s** (Vollbuild über 265.225 Parts)
* `sift search` mit Sanitizer, RRF-Fusion, Scope-Dedup, Session-/Snippet-Diversität
* `scripts/verify-sql.sh` → PASS inklusive Dedup-Nachweis gegen die echte DB
* `cargo build --release` → grün, ohne Warnungen

Offen, in dieser Reihenfolge:

1. **MCP-Server (`rmcp`).** `Cargo.toml` enthält die Dependency bewusst als
   Kommentar: `rmcp` 3.5.1 war drei Tage alt, und ungeprüfter Makro-Glue ist
   schlechter als ein funktionierendes CLI. Vier Tools: `sift_search`,
   `sift_mark_seen`, `sift_index_status`, `sift_rebuild`.
2. **Snippet-Fingerprint in `sift_seen`.** Aktuell merkt die Tabelle `doc_id`.
   Dadurch rückt der Zwillings-Snippet eines gemerkten Dokuments im nächsten
   Durchlauf nach (in Phase 0 beobachtet: `doc 2694` → `doc 2693`).
3. **Echter 64-Bit-`body_fp`.** Aktuell `length + 64 Zeichen` als Heuristik.
   Braucht das `rusqlite`-Feature `functions` mit `sift_fp()` als SQL-Funktion.
4. **Skill** `~/.config/opencode/skills/sift/SKILL.md` (Phase 3).

## Regeln, die nicht verhandelbar sind

* `~/.local/share/opencode/opencode.db` ist **read-only**: kein `CREATE`, kein
  `INSERT`, kein Trigger. 5,46 GB, WAL-Modus, live beschrieben.
* **Niemals `immutable=1`.** Überspringt WAL/SHM und liefert still veraltete
  Zeilen. Nur `mode=ro`.
* Trigger im **Sidecar** sind korrekt und nötig — dort gehört uns die Datei.
* SQL bleibt in Dateien (`src/sql.rs` bindet per `include_str!`). Nicht in
  Rust-Strings umziehen.
* Nach jeder Änderung an `sql/*.sql` oder `migrations/*.sql`:
  `scripts/verify-sql.sh` laufen lassen. Läuft in ~2 s.
* Keine Zahl erfinden. Jede Doku-Zahl ist gemessen (mit Datum) oder als
  Vermutung markiert. `weights.toml` ist **nicht** getunt.

## Technische Kurzform

* Quelle: `opencode.db`, `part.data` ist JSON. Verifizierte Pfade:
  `$.text` (type `text` **und** `reasoning`), `$.state.output` (type `tool`),
  `$.tool` (Tool-Name). `$.output` und `$.patch` sind **keine** Inhaltspfade.
* `rusqlite{bundled}` kompiliert SQLite mit FTS5 unbedingt mit. Kein
  `bundled-full` nötig.
* RRF: `1/(60 + r_bm25) + w_rec/(60 + r_recency)`. `k = 60` bewusst nicht getunt.
* Rust ↔ Julia: **Gewichtsdatei**, kein `jlrs`. Begründung in ADR-3.
  Julia ist Offline-Labor für den Gewichts-Sweep, nicht Runtime.

## Verifikation

```bash
export PATH="$HOME/.cargo/bin:$PATH"
scripts/verify-sql.sh
cargo build --release
./target/release/sift status
./target/release/sift search "<term>" --limit 3 --scope test
./target/release/sift search "<term>" --limit 3 --scope test   # darf nichts wiederholen
```

Ein Feature gilt erst als fertig, wenn das zugehörige Kommando grün ist.
