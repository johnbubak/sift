# AGENTS.md — sift

Gilt für jede Session in diesem Repo. Canonical source, nicht die Chat-Historie.

## Reihenfolge

1. **Erst lesen, dann schreiben.** `docs/00-archeologie.md` enthält die gemessenen
   Fakten. Bei Widerspruch zu einer Erinnerung oder zu Modellwissen gilt die Datei.
2. Bei einer Design-Frage `docs/02-adr.md` — dort stehen auch die **verworfenen**
   Wege. Wenn jemand sie erneut vorschlägt, ist der Grund dort dokumentiert.
3. `docs/03-phasen.md` sagt, was als fertig gilt und welche Einstiegsbedingung
   für die nächste Phase gilt. Eine Phase nicht ohne ihr Verifikationskommando
   abschließen.

## Harte Regeln

* `~/.local/share/opencode/opencode.db` ist **read-only**. Kein `CREATE`, kein
  `INSERT`, kein Trigger. 5,46 GB, WAL, live beschrieben.
* **Niemals `immutable=1`.** Die Quelle wird aktiv geschrieben; `immutable=1`
  überspringt WAL/SHM und liefert still veraltete Zeilen. Nur `mode=ro`.
* Das SQL bleibt in Dateien (`src/sql.rs` bindet es per `include_str!` — auch
  `sql/extract_lines.sql` und `sql/extract_matches.sql` für `sift extract`).
  Nicht in Rust-Strings umziehen und nicht "aufräumen" — es ist das validierte
  Artefakt.
* Der Sidecar gehört uns: Trigger, Migrationen und `sift_seen` sind dort
  ausdrücklich erlaubt und richtig.

## Kopplungen, die man nicht sieht

* **Ein Gewicht berührt vier Stellen.** `sql/search.sql` (?N-Reihenfolge),
  `src/weights.rs` (Struct + Default), `src/search.rs` (`params!`-Reihenfolge)
  **und** `scripts/verify-sql.sh` (setzt ?1..?9 von Hand). Wird eine Stelle
  vergessen, ist es kein Compile-Fehler, sondern still falsches Ranking.
* **`verify-sql.sh` patcht `sql/extract.sql` per `sed` auf die wörtliche Zeile
  `AND length(x.body) > 0;`** und hängt daran ein `LIMIT`. Wird diese Zeile in
  `extract.sql` umformatiert, injiziert das Skript kein LIMIT mehr, extrahiert
  den Vollkorpus (minutenlang) und meldet trotzdem PASS. Nach jeder Änderung an
  `extract.sql` die Dauer im Auge behalten.
* **`sql/extract_v2.sql`** extrahiert aus `src.session_message` (v2-Sessions).
  Gleiche ATTACH/DETACH-Konvention, gleiche `AND length(x.body) > 0;`-Zeile.
  `verify-sql.sh` testet derzeit nur `extract.sql` — `extract_v2.sql` manuell
  mit LIMIT prüfen (siehe verify-sql.sh-Pattern).
* **`ATTACH` steht nicht in `extract.sql`**, weil `rusqlite::execute_batch`
  keine Parameter binden kann. Aufrufer macht ATTACH → Skript → DETACH
  (`src/db.rs`, `verify-sql.sh`). Das ist Absicht, kein Versehen.
* **`snippet()` muss im Query mit dem `MATCH` stehen**, nicht im äußeren SELECT.
  `bm25()`/`snippet()` sind FTS5-Aux-Functions und werden als normale
  Skalarfunktionen gelesen — das `rusqlite`-Feature `functions` ist dafür weder
  nötig noch vorhanden. `bundled` kompiliert FTS5 unbedingt mit; `bundled-full`
  nicht dazufügen.
* `body_fp` speist keine Deduplizierung in `search.sql` (dort läuft nur
  `snip_rank` über den Snippet-Text). Ein echter Hash ändert erst dann etwas,
  wenn er auch im Query benutzt wird.
* **`sift extract` ist ein eigener Pfad.** Eigene SQL-Dateien, eigene
  In-Memory-FTS5 (eine Row pro Zeile, rowid = Zeilennummer), **nie**
  Sidecar-Zugriff. Die Vollständigkeit („jede Trefferzeile überlebt“) ist
  strukturell, kein Ranking-Effekt — `snippet()` verliert gemessen 8 von 9
  Treffern. Die Schwellen in `src/extract.rs` (2000 Zeichen, 25 MB, ±6 Zeilen,
  60 %) sind **Heuristiken**, keine Messungen.

## Fallen beim Ausführen

* **`sift search --scope S` schreibt.** Jeder Lauf mit einem Scope markiert die
  gelieferten `doc_id` in `sift_seen` — auch der erste. Für reproduzierbare
  Prüfungen jedes Mal einen neuen Scope-Namen nehmen, sonst prüft man den zweiten Lauf.
* `sift build --full` leert auch `sift_seen` und den Watermark. Nach einem
  `--full` ist die Anti-Redundanz-Historie weg.
* **`sift … | head` ist kein Fehlerfall:** stdout läuft durch `outln!`/`out!`
  (`src/main.rs`); ein geschlossener Pipe endet mit Exit 141 (SIGPIPE-Konvention),
  ohne Panic und mit leerem stderr. Nicht auf `println!` zurückbauen.
* **`sift extract`-Verdicts sind keine Fehler** (Exit 0): `small_file`,
  `binary`, `too_large`, `no_hits`, `too_broad` heißen „Datei normal lesen“.
  Exit 1 nur bei echten Fehlern (fehlende Datei, ungültiges `--for`/`--context`).
* `weights.toml` wird so gesucht: `SIFT_WEIGHTS` → neben dem Binary → cwd.
  Aus dem Repo-Root wird also das Repo-`weights.toml` gelesen, aus einem anderen
  cwd stillschweigend der eingebaute Default. Bei abweichenden Ergebnissen
  zuerst das prüfen.
* Es gibt Tests: 15 in `src/trigger.rs`, 10 in `src/extract.rs`, 9 in
  `src/search.rs`, 4 in `src/main.rs` (`cargo test` → 38). Sie decken **nur**
  Trigger-Erkennung, Datums-/Filter-Logik und Datei-Extraktion ab — **keine**
  Sidecar-SQL, kein Ranking. Der einzige SQL-Regressionsschutz bleibt
  `scripts/verify-sql.sh` plus der echte CLI-Aufruf.
* `weights.toml` hat `snippet_tokens = 18`, der eingebaute Default in
  `src/weights.rs` ist **30**. Zwei Werte an zwei Stellen — das ist Absicht
  (Tuning lebt in der Datei, nicht im Binary), aber eine Änderung an einem
  ändert nicht den anderen.
* `cargo fmt --check` ist aktuell **nicht** grün (27 Diffs). Nicht mit einem
  pauschalen `cargo fmt` aufräumen — das erzeugt Diff-Rauschen. Nur die eigene
  Änderung sauber halten.
* `rustfmt` und `clippy` laufen sauber durch; Warnungen bei `cargo build` gelten
  als Fehler.

## Korrektheit

* Keine Zahl erfinden. Jede Doku-Zahl ist entweder gemessen (mit Datum) oder als
  Vermutung markiert. Ein Feature ohne Messung wird als Vermutung geführt.
* Ausgaben von Subagenten gelten als ungeprüft, bis sie gegen die DB oder gegen
  `cargo build` verifiziert sind. Vorher gab es mehrere Falschmeldungen zu
  FTS5-Fähigkeiten und DB-Größen.
* Wenn eine Aussage nicht belegbar ist: sagen, dass sie unbelegt ist.

## Der Trigger-Erkener darf recall nie verschlechtern

`sift should-search` ist **beratend**. Ein Detektor, der Recall schlechter macht
als gar keiner, ist wertlos. Daraus folgt die Regel:

* **Fail-open.** Kein Signal darf eine Suche blockieren. Ohne Signal: S0 mit
  `hard = false` — erlaubt, nur nicht empfohlen. Leere Eingabe: S2.
* **`hard = true` nur bei positivem Gegenbeweis** — Verneinung oder Zitat. Das ist
  der einzige Fall, in dem nicht gesucht werden soll.
* Jedes Signal geht durch `evaluate()`, damit Quotation und Verneinung **unabhängig
  vom Tier** wirken. Ein unterdrückter Tier-A-Treffer darf nicht von einem schwächeren
  Tier-B-Treffer gerettet werden.
* Das Modell bleibt der Klassifikator. Das Modul liefert Zustand + Evidenz +
  extrahierte Terme, keine boolsche Entscheidung.

`cargo test` muss grün sein. Beim Hinzufügen von Trigger-Wörtern gehören
Korpus-Häufigkeiten als Kommentar dazu — Precision kommt aus der Rareheit, nicht
aus der Intuition.

* `README.md` und `STARTPROMPT.md` nennen `julia/sweep.jl` — das Verzeichnis
  existiert nicht. Phase 6 ist unangetastet; die Defaults in `weights.toml` sind
  ausdrücklich **nicht** getunt.

## Bekannte offene Punkte

* `body_fp` ist eine Heuristik (`length + 64 Zeichen`), kein Hash.
* `sift_seen` merkt `doc_id`, nicht den Snippet-Fingerprint → Zwillings-Snippets
  rücken im nächsten Durchlauf nach.
* `snip_rank` und `sess_rank` sind harte Filter und können kleine Korpora leerziehen.
* Der MCP-Server ist implementiert (`sift mcp`, stdio). `rmcp` aktiv.
* `weights.toml` ist **nicht** getunt.
* v2-Sessions werden jetzt indexiert (14.213 Docs). `compaction`-Parts und
  `session_context_epoch` sind noch nicht als Signals genutzt.

## Verifikation

```bash
export PATH="$HOME/.cargo/bin:$PATH"    # cargo ist nicht im Standard-PATH
scripts/verify-sql.sh [rows] [term]     # default: 4000 rows, term "error", ~2 s
cargo build --release                   # Warnungen müssen weg bleiben
./target/release/sift status
./target/release/sift search "<term>" --limit 3 --scope probe1
./target/release/sift search "<term>" --limit 3 --scope probe1  # darf nichts wiederholen
./target/release/sift extract src/main.rs --for "<term>"  # jede Trefferzeile + Kontext
./target/release/sift spur --limit 5    # Selbstaudit: Trunkierung, Bad Endings, Loops
```

Ein Feature gilt erst als fertig, wenn das zugehörige Kommando grün ist.

## Umgebung

* Sidecar `~/.local/share/sift/sift.db` (~651 MB), Quelle
  `~/.local/share/opencode/opencode.db` (5,46 GB). `data/` ist untracked;
  `.gitignore` deckt `*.sqlite*` ab, aber nicht `*.db`.
* Fünf Commits: `a9d30d4` Phase 0+1 · `6f2bbd7` research_fts-Herkunft ·
  `5cabc1c` Phase 2a · `bae156d` SIFT-1-Handoff · `90fa4cd` SIFT-9 ·
  `0f78ca5` SIFT-10 v2-Sessions.
* `sift` ist global erreichbar über `~/.local/bin/sift` — ein **Wrapper**, kein
  Symlink, weil er `SIFT_WEIGHTS` pinnt. Die systemd-Unit `opencode.service`
  pinnt `PATH` und enthält `~/.local/bin` nur über ein Drop-In
  (`~/.config/systemd/user/opencode.service.d/path.conf`). Ohne dieses Drop-In
  findet ein Agent `sift` nicht, obwohl der Skill geladen wird.
* Neues Kommando: `sift spur [--limit N] [--min-repeat N]` — Selbstaudit der
  Agent-Spur (Trunkierung, Bad Endings, Loops). Read-only auf opencode.db,
  kein Sidecar, kein neues Schema.
EOF
echo "written"