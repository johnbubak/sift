# sift — Phasen

Jede Phase hat ein Deliverable und ein Verifikationskommando. Eine Phase gilt
erst als fertig, wenn das Kommando grün ist.

---

## Phase 0 · SQL-Kern ✅ (fertig)

**Deliverable.** `migrations/001_sidecar.sql`, `sql/extract.sql`, `sql/search.sql`,
`scripts/verify-sql.sh`.

**Verifiziert am 2026-10-08:**

```
$ scripts/verify-sql.sh
docs=4000 fts=4000
PASS: FTS in sync (4000 docs); 0 of [2694] leaked into scope=demo
```

Damit steht: Schema, idempotente Extraktion gegen die echte 5,46-GB-DB,
FTS-Sync, BM25 + snippet() + RRF, Snippet- und Session-Diversität, und die
Kernanforderung — gemerkte Dokumente tauchen im nächsten Scope nicht wieder auf.

**Noch offen aus dieser Phase:**
* Snippet-Fingerprint in `sift_seen` ergänzen (Zwillings-Snippets, siehe ADR-6)
* `body_fp` ist ein Prototyp (`length + 64 Zeichen`) — echter 64-Bit-Hash in Rust
* `snip_rank` ist ein harter Filter: auf dem vollen Korpus messen, ob er
  Ergebnislisten leerzieht

---

## Phase 1 · Rust-Indexierer ✅ (fertig, 2026-10-08)

**Deliverable.** `sift build` — `src/index.rs`, `src/db.rs`.

**Gemessen:**

```
$ sift build
added 139493 · 139493 docs · 139493 index rows · 231 MB body · watermark 2026-10-08 10:57
real  0m20.2s          sidecar 462 MB
```

Die Zahl ist vollständig, nicht geschätzt: 31.531 (`text`) + 43.879 (`reasoning`)
+ 64.218 (`tool`) = 139.628 nicht-leere Parts, minus 135 verwaiste Parts ohne
passende Session = **139.493**. `fts_rows == docs`.

**Offen aus dieser Phase:**
* echter 64-Bit-`body_fp` statt der Heuristik `length + 64 Zeichen`
  (nötig: `rusqlite`-Feature `functions`, `sift_fp()` als SQL-Funktion)
* Snippet-Fingerprint in `sift_seen`, sonst rückt der Zwillings-Snippet eines
  gemerkten Dokuments im nächsten Durchlauf nach (in Phase 0 beobachtet)

---

## Phase 2a · Token-Budget, OR-Semantik, Trigger-Erkennung ✅ (2026-10-08)

**Deliverable.** `src/trigger.rs`, `sift should-search`, `--and`, neu kalibriertes
`snippet_tokens`. 21 Unit-Tests grün, `cargo build` warnungsfrei, `verify-sql.sh` PASS.

**Drei gemessene Korrekturen, alle durch Testen gefunden:**

1. **FTS5-Token sind keine LLM-Token.** `snippet(…, 30)` ergab über 200 echte Treffer
   204 Zeichen ≈ **51 LLM-Tokens** — der unicode61-Tokenizer zählt auf deutschem/
   Code-Text ~4 Zeichen pro Token, ein LLM-Tokenizer ~2,8. `snippet_tokens` 30 → **18**.
   Eine 3-Treffer-Suche kostete real 163 Tokens, jetzt **114**.
2. **AND war zu strikt.** Über die reale Query-Liste gemessen:

   | Query | AND | OR |
   |---|---|---|
   | jwt cache invalidierung | 1 | 500+ |
   | fts5 trigram tokenizer | 26 | 500+ |
   | surrealdab async integration | 147 | 500+ |

   „jwt cache invalidierung" gab schlicht `no hits`. OR ist hier sicher, weil bm25
   Mehr-Term-Treffer per Konstruktion höher rankt — die Präzision kommt aus dem
   Ranking, nicht aus der Booleschen Klebstoff-Schicht. `--and` bleibt verfügbar.
3. **Trigger-Erkennung.** Siehe `src/trigger.rs`. Fail-open ist harte Anforderung;
   Wortlisten sind mit **phrasen-adjacenten** Korpus-Häufigkeiten belegt
   (`"wie haben wir"` = 3, aber AND-anywhere 823 — für eine eingehende Nachricht
   ist Adjazenz die ehrliche Präzisionsschätzung).

**Dabei gefundene echte Bugs** (alle mit Regressionstest festgenagelt):

* Byte-Offsets der normalisierten Zeichenkette wurden mit Offsets der
  Originalnachricht verglichen — zwei Koordinatensysteme. Zitat-Erkennung war
  dadurch wirkungslos.
* Die Verneinungssuche schaute nur rückwärts; Deutsch negiert vorwärts
  („ich erinnere mich **nicht**").
* Quotation/Verneinung wirkten nur auf Tier A — ein gequotetes „gelöst" rettete
  einen unterdrückten Tier-A-Treffer. Jetzt geht **jedes** Signal durch `evaluate()`.
* „sag" stand als Write-Verb in der Liste und unterdrückte „sag mir, wie wir das
  gelöst haben" — ein Recall-Bitte-Satz. Das Objekt entscheidet, nicht das Verb.
* Der Sanitizer ließ reine Punkt-Tokens (`...`, `../..`) durch, weil `.` in der
  erlaubten Zeichenliste ist.
* „sag mir wie wir das gelöst haben" ergab S2 **ohne** Terms, weil „gelöst" selbst
  ein Trigger-Wort ist. Jetzt Fallback auf die Trigger-Begriffe selbst.

**Noch offen aus dieser Phase:**
* ~~`--directory` und `--after` als Filter~~ → **SIFT-1 abgeschlossen
  2026-10-08**, vier Filter als ?10..?13. Gemessen: `error` global liefert 4
  hictus-Treffer im Top-10, mit `--directory /home/jan/projects/hictus` sind es
  10/10 — die Filter sitzen also vor dem `LIMIT 50` und verengen die Auswahl
  selbst, nicht nur die Ausgabe. `fts5` 153 Kandidaten/4 Verzeichnisse →
   15 (−90 %) bei `--directory /home/jan/projects/sift`. `cargo test` 38 grün.
* ~~Der Skill selbst (Phase 3)~~ → **Skill ist live** (2026-10-08), siehe Phase 3.
* Bekannte Schwäche: Prefix-Match auf häufige Wörter erzeugt Falschtreffer
  („jwt cache invalidierung" führt auf `_EMBED_CACHE_MAX`). Inherent für `term*`,
  aber eine offene Frage für Phase 5.

**Zu beachten.** Auch mit aktiviertem `sess_rank` kamen die drei Treffer eines
Laufs aus drei Sessions **desselben** Projekts. Ein Diversitäts-Guard auf
`directory` statt `session_id` ist eine offene Frage für Phase 5 — erst messen,
dann bauen.

**Verifikation bleibt:**

```
$ sift search 'wal checkpoint' --limit 3 --scope demo
$ sift search 'wal checkpoint' --limit 3 --scope demo   # keine Wiederholung
```

Der zweite Aufruf darf keine der doc_ids des ersten liefern.

---

## Phase 2b · Datei-Extraktion (`sift extract`) ✅ (2026-10-08)

**Deliverable.** `sift extract` — `src/extract.rs`, `sql/extract_lines.sql`,
`sql/extract_matches.sql`. Extrahiert Trefferzeilen aus einer Datei mit Kontext,
ohne Sidecar-Zugriff.

**Gemessen:**

* 10 Unit-Tests grün (`cargo test` → 38 gesamt).
* **Jeder Treffer überlebt** — kein snippet-Sampling. `offsets()` liefert keine
  Trefferpositionen; Einzelabfrage je Treffer ist der einzige Weg zu vollständiger
  Abdeckung. Gemessen: `src/search.rs --for "sanitize weights directory"` →
  31 Trefferzeilen, 8 Regionen, **31/31** grep-Zeilen in der Ausgabe.
  `--for sanitize` allein: 11/361 Zeilen, 3 Regionen, kept 24 %.
* `too_broad` ab 60 % Trefferdeckung — fail-closed, keine neue Default-Regel.
* Kein Sidecar: eigene In-Memory-FTS5, eine Row pro Zeile, rowid = Zeilennummer.

**Verdicts sind keine Fehler** (Exit 0): `small_file`, `binary`, `too_large`,
`no_hits`, `too_broad` heißen „Datei normal lesen". Exit 1 nur bei echten Fehlern
(fehlende Datei, ungültiges `--for`/`--context`).

---

## Phase 3 · Skill ✅ (live, 2026-10-08)

**Datei.** `~/.config/opencode/skills/sift/SKILL.md`

**Verifikation erfüllt** — die Phase verlangt eine echte Session mit einer Frage
aus einer früheren Session. Nach Neustart von `opencode.service` geladen
(`opencode debug skill` → 13 Skills, `sift` dabei) und in einer echten Session
geprüft: Frage nach der Claude-Transcripts-Architektur, Antwort mit Snippet und
korrekter Herkunft, kein Volldump. Zusätzlich gemessen: `error` mit
`--directory hictus` liefert 10/10 statt 4/10 — die Filter-Regel aus SIFT-1 greift.

**Voraussetzung, die real war und nicht im Ticket stand:** Der Server pinnt
`PATH=/usr/local/bin:/usr/bin:/bin` in der systemd-Unit. `~/.local/bin` fehlte,
`~/.local/bin/sift` wäre für jeden Agenten `command not found` gewesen, obwohl
der Skill geladen war. Gelöst über
`~/.config/systemd/user/opencode.service.d/path.conf`. `~/.local/bin/sift` ist
ein **Wrapper**, kein Symlink: er pinnt `SIFT_WEIGHTS`, weil ein Symlink auf
`target/release/` aus fremdem cwd sonst stillschweigend die eingebauten Defaults
nutzen würde.

**Zusätzlich in Grundprinzip 8 von `/home/jan/projects/OpenCode/agents.md`**
verankert — das ist die global verdrahtete `instructions`-Datei und damit der
einzige Ort, den wirklich jede Session liest.

**Regeln im Skill.**
1. Pro Conversation EINMAL eine Scope-ID setzen.
2. `should-search` zuerst: S0 → nicht suchen, S1 → erst `git log -S`/`rg`,
   S2/S3 → genau eine Suche mit den extrahierten Terms.
3. Ausschließlich die gelieferten Snippets übernehmen — nie `sift_doc.body`
   nachladen.
4. Jede übernommene Erkenntnis mit Herkunft markieren:
   `💡 [sift] <directory> · <session_title> · <kind>[/<tool>] · <zeit>`
5. `--scope` markiert beim Ausliefern, `mark-seen` ist Teil von `search`.
6. Keine Treffer → das sagen. Kein Raten aus Modellwissen.
7. Folge-Suchen **immer** mit `--directory`/`--after` eingrenzen.

**Fail-open** ist im Skill explizit: `hard = no` heißt suchen ist erlaubt, nur
nicht empfohlen. Nur `hard = true` (Verneinung oder Zitat) rechtfertigt, nicht zu
suchen.

---

## Phase 4 · Verteilung

Single-Binary, keine Abhängigkeiten. README mit Einzeiler-Install,
`opencode.json`-Fragment, Lizenz. Erst wenn Phase 3 nachweislich trägt.

---

## Phase 5 · Messungen, keine Features

Nur Experimente, jeweils mit Einstiegsbedingung:

| Experiment | Eintritt | Abbruch |
|---|---|---|
| Trigram statt unicode61 | Indexgröße und Trefferquote auf 10 gelabelten Fragen | kein messbarer Gewinn → weg |
| Semantische Suche | **Präzedenzfall existiert:** opencode-mem fährt hier bereits `memories.vector F32_BLOB(768)` via sqlite-vec, 15 Projekt-Shards, ohne API-Kosten (`docs/00-archeologie.md`). Also: lokale Embeddings sind hier keine Theoriefrage. Hybrid muss FTS-only bei 10 gelabelten Fragen messbar schlagen. | nicht messbar besser → weg |
| `patch` indizieren | jemand nennt einen echten Anwendungsfall | — |
| Diversitäts-Guard auf `directory` statt `session_id` | in Phase 2 beobachtet: drei Treffer aus drei Sessions **desselben** Projekts | Guard bringt auf gelabelten Fragen nichts → weg |

**Einstiegsbedingungen, bevorzugt:** Python-Benchmarks (`fast-recall` /
`sqlite-vec`) nur als Referenz gegen die eigenen Messwerte, nie als Begründung.

---

## Phase 6 · Julia-Labor

`julia/sweep.jl`: variiert `rrf_k`, Recency-Gewicht, Titelgewicht,
`max_per_session` gegen ein gelabeltes Query-Set, schreibt `weights.toml`.

**Eintritt.** Es gibt ein gelabeltes Query-Set (mindestens 10 Fragen mit
Soll-Treffer). Ohne Labels ist ein Sweep Raten und wird nicht gebaut.

**Verifikation.** `weights.toml` aus dem Sweep muss die Defaults schlagen oder
das Experiment wird beendet und die Defaults bleiben.

---

## Phase 7 · Multi-Quellen-Ingest (offen, spezifiziert 2026-10-08)

**Ziel.** Nicht nur Session-Chats, sondern alle Textquellen des Rechners in
einen Index. Ausgangsmessung (nur lesend, gegen die echten Pfade):

| Quelle | Umfang | Extrahierbar | Zeitraum | Status |
|---|---|---|---|---|
| `~/.claude/projects/**/*.jsonl` | 234 Datei / 115,4 MB | **17,3 MB** | 27.01.–07.05. | eingefroren |
| `~/.claude/memory/*.md` | 36 Datei / 106 KB | 106 KB | — | aktiv |
| `~/.claude/plans/*.md` | 13 Datei / 110 KB | 110 KB | — | aktiv |
| `~/.claude/paste-cache/*.txt` | 57 Datei / 1,25 MB | 1,25 MB | — | aktiv |
| `~/Downloads` | 356 Datei / 12.984 MB | ~50,8 MB (Text) | — | verrauscht |
| `~/.claude/todos/*.json` | 147 Datei / **3,5 KB** | ~0 | — | Rauschen |
| `~/projects/claude-brain` | **0** Arbeitsbaum-Dateien | — | — | ausschließen |

**Die entscheidende Messung: die Zeiträume überlappen nicht.**
`opencode.db` beginnt 2026-05-17 13:45, das letzte Claude-Transcript endet
2026-05-07. Der Claude-Korpus ist also **kein Duplikat**, sondern rund vier Monate
Historie, die sift bisher strukturell nicht sehen kann.

**Einschränkung, die vor der Architekturentscheidung kommt.** Claude Code
schreibt seit 2026-05-17 nichts mehr: keine Transcripts, `history.jsonl` ist
144 Tage alt, kein Prozess läuft. Das ist ein **Archiv-Import**, kein zweiter
Live-Feed. Er verkleinert sich nicht, wächst aber auch nicht.

**Warum das kein `SIFT_SOURCE`-Wert ist.** `sql/extract.sql` liest per
`ATTACH … AS src` aus SQLite. Claude Code ist JSONL auf der Platte. Ein zweiter
Ingest-Pfad ist nötig (63 Top-Level-Keys, `message.content` ist String *oder*
Blockliste aus `text`/`thinking`/`tool_use`/`tool_result`).

**Gemessene Formate (nicht geraten):**
* Kern-Record: `parentUuid, isSidechain, promptId, type, message, uuid, timestamp,
  userType, entrypoint, cwd, sessionId, version, gitBranch, slug`
* Ordnername = `re.sub(r'[^A-Za-z0-9]', '-', cwd)` — **108/108** bestätigt.
  `_` wird zu `-`, es überlebt nicht.
* `sessionId` == Dateiname ohne `.jsonl` — 12/12 stichproben geprüft.
* `message.content` als String: 345 · als Blockliste: 5.327.

**Schema-Haken, vor dem ersten Commit zu klären.** Claude hat kein
`title` pro Session (nur `cwd`, teils `aiTitle`/`customTitle`). Das Schema
verlangt `title TEXT NOT NULL` — ein Import bricht dort mit einer
NOT-NULL-Verletzung ab, wenn man es nicht vorher behandelt. Fallback auf den
Verzeichnisnamen.

**Empfohlene Reihenfolge, nach Signalqualität pro Byte:**

1. `memory` + `plans` + `paste-cache` — 106 Datei, 1,46 MB, maximale
   Signaldichte. Braucht keinen Parser, nur Datei → Text.
2. Claude-Transcripts — 17,3 MB, echter Parser nötig, einmalig historisch.
3. `Downloads` — nur mit Filtern: `*.json.md` raus (19 Stück duplizieren
   bereits indexiertes Material), > 5 MB raus, PDF separat (56 Datei, 37,7 MB,
   braucht Extraktion und ist der teuerste Posten pro Treffer).

**`claude-brain` ist ausgeschlossen:** 0 Dateien im Arbeitsbaum, alle 18 Dateien
liegen in `.git/hooks/` (14 `.sample`), Repo ohne Commits.

**Einstieg.** `sift search` muss auf einem Januar-Thema aus den Claude-Transcripts
einen Treffer mit korrekter Herkunft liefern. Vorher wird nichts gebaut.

---

## Phase 6 · Spur — Selbstaudit (live, Iteration 0, 2026-10-09)

**Name.** Spur (Track/Trail). Bringt sift (Recall), flowhound (exakter Variablen-Fluss)
und Selbstaudit (Drift-Erkennung) unter einen Hut.

**Datei.** `src/main.rs` → `cmd_spur()`. Read-only auf `opencode.db`, kein
Sidecar, kein neues Schema.

**Drei Signale:**
1. Unpaired step-starts — strukturelle Trunkierung (Schritt begann, nie beendet).
2. Bad session endings — Session endete in Fehler/Abbruch/leer.
3. Loops — gleiche Tool+Signatur ≥ N× in einer Session (Dichte, nicht Adjazenz).

**Verifikation:**
```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test                      # 38 Tests, grün
cargo build --release           # 0 Warnungen
./target/release/sift spur --limit 5   # 3 Sektionen mit Daten
```

**Befund (gemessen):** 562 ungepaarte Starts / 503 Messages, 159 Bad Endings,
64 Loop-Sessions (Spitze: 337× dieselbe Signatur). Adjacent-Repeat-Rate ~6 %,
Dichte-Erkennung findet 62/1.575 Sessions.

**Offene Iterationen:**
- Iteration 1: Schwellenwert-Kalibrierung, Scope-Filter (`--session`, `--since`).
- Iteration 2: Shingle-Jaccard über `text`-Parts (Plain-Table neben FTS5).
- Iteration 3: MCP-Exposition (abhängig von Phase 2 / rmcp).
