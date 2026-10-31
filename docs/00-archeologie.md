# sift — Archäologie

Alle Zahlen hier sind **am 2026-10-08 gegen die laufende Datenbank gemessen**, nicht geschätzt
oder aus Dokumentation abgeleitet. Bei Widerspruch gilt die Messung, nicht diese Datei.

Quelle: `~/.local/share/opencode/opencode.db`

## Größe und Zustand

| | |
|---|---|
| `opencode.db` | **5,46 GB** (5.457.252.832 B) |
| `opencode.db-wal` | **406 MB**, wächst weiter |
| `opencode.db-shm` | 64 KB |
| Journal-Modus | WAL |
| mtime | während der Analyse laufend aktualisiert → opencode schreibt **gerade** |

Daneben liegen `opencode.db.bak` (2,8 GB), `opencode_biegen.db` (545 MB),
`opencode.db_vor execution` (546 MB) sowie `bak/ bak2/ backup/ rescue/ rescued/`.
opencode.db ist damit das größte Artefakt im Home-Verzeichnis.

## Tabellen

| Tabelle | Zeilen | Rolle |
|---|---|---|
| `session` | 1.522 | 34 Projekte, 42 Verzeichnisse, alle 1.522 mit Titel |
| `message` | 62.426 | `data` = JSON mit Rolle |
| `part` | 265.225 | `data` = JSON, Actual-Content-Ebene |
| `project` | — | `time_initialized` markiert die Projektordner-Initialisierung |
| `project_directory` | — | `(project_id, directory, type, strategy)` |
| `workspace` | — | `(project_id, branch, directory, time_used)` |
| `session_message`, `session_context_epoch`, `session_share`, `session_input` | — | opencode-intern |
| `research_messages` | 49.699 | **vergessene manuelle Nutzung**, siehe unten |
| `research_fts` | 49.699 | FTS5-Schattenindex darüber |

`sqlite_stat1` und `sqlite_stat4` existieren → jemand hat `ANALYZE` gelaufen.

## part.data — verifizierte Feldpfade

`part` hat `id, message_id, session_id, time_created, time_updated, data`.
`time_created` ist **ms-Epoch**. `data` ist ein JSON-Objekt.

| part.type | Inhalt | Pfad | Zeilen | nicht-leer | ø Länge | Volumen |
|---|---|---|---|---|---|---|
| `tool` | Output | **`$.state.output`** | 65.141 | 64.218 (99 %) | 2.588 B | ~166 MB |
| `reasoning` | Text | **`$.text`** | 50.458 | 43.879 (87 %) | 1.465 B | ~64 MB |
| `text` | Assistant-Text | **`$.text`** | 31.552 | 31.531 (99 %) | 1.721 B | ~54 MB |
| `step-start` | Boilerplate | — | 54.737 | — | — | 4 MB |
| `step-finish` | Metadaten | — | 54.205 | — | — | 11 MB |
| `patch` | **kein Text** | `$.patch` = leer | 8.842 | **0** | — | 3,5 MB |
| `file` | base64-Blobs | — | 88 | — | — | 35 MB |
| `compaction`, `agent` | ignoriert | — | 239 | — | — | — |

Tool-Namen unter `$.tool`: bash 28.218 · read 14.130 · edit 9.080 · grep 3.848 ·
todowrite 2.428 · write 1.654 · sshmcp_execute 1.016 · task 899 · glob 740 ·
webfetch 580 · websearch 343 · question 341.

> **Fallstrick:** `$.output` und `$.patch` sind **nicht** die Inhaltspfade (0 nicht-leer).
> Eine falsche Pfadangabe liefert still leere Texte statt eines Fehlers.

Indexierbare Textmenge: **~284 MB**.

## Der vorhandene FTS5-Index

```sql
CREATE VIRTUAL TABLE research_fts USING fts5(
    message_id UNINDEXED,
    session_id UNINDEXED,
    content
)
```

* 49.699 Zeilen, alle mit gültiger `message_id`/`session_id` (0 verwaist)
* Tokenizer **unicode61** (Default), `version=4` in `research_fts_config`
* **Keine Trigger** — reines `content=`-freier Snapshot, kein Schattenindex
* `research_messages(message_id, session_id, session_title, role, cwd, raw_json)`
  ist die flachgezogene Message-Kopie dahinter

**Wer das schreibt, ist nicht auffindbar**: kein `plugin/`, kein `command/` in
`~/.config/opencode/`, kein Treffer in `~/.opencode-mem/`. Es ist ein
**abgefahrener Weg** — eingefroren, Alterung nicht nachweisbar (keine Zeitstempel
in der FTS-Tabelle).

### Herkunft: eingegrenzt, aber nicht auflösbar (2026-10-08)

Mit sift selbst nachgeforscht:

| Prüfung | Ergebnis |
|---|---|
| `research_fts` in der Session-Historie | **32 Treffer, alle von heute.** Vor 2026-10-08 kommt das Wort nicht vor. |
| `fts5` in der Session-Historie | 6 Treffer im 2026-09, 147 im 2026-10 |
| Die 6 September-Treffer | Session `/home/jan/projects/hictus`, 2026-09-02: Vergleich von **thoth-mem**, **opencode-mem**, claude-memory, mcp-memory-keeper. Beschreibt opencode-mem wörtlich als „combines SQLite FTS5/BM25 with sqlite-vec semantic search, reranks results". |
| opencode-mem-Shards (`~/.opencode-mem/data/projects/*.db`) | 15 Stück, **alle** mit identischem 5-Tabellen-Schema, **keine** mit `research_fts` |

**Fazit:** Die Tabelle wurde **nicht innerhalb einer opencode-Session** angelegt —
sonst stünde das DDL in einem Tool-Output. Sie ist auch kein Teil von
opencode-mems Schema. Wahrscheinlichstes Szenario: ein Skript in einem nackten
Terminal, dessen Output nie in einer Session landete.

**Fallstrick bei der Suche:** `grep -rl research_fts ~/.opencode-mem` liefert einen
Treffer in `project_9aad22d1e91e9642_shard_0.db` — das ist ein **Fehlpositiv**.
Der String steht in `memories.content`, weil opencode-mem die Archäologie-Notizen
dieses Projekts bereits auto-captured hat. Circular hit.

### opencode-memShard-Schema (relevant für Phase 5)

```sql
CREATE TABLE memories (
    id TEXT PRIMARY KEY, content TEXT NOT NULL,
    vector F32_BLOB(768) NOT NULL, tags_vector F32_BLOB(768),
    container_tag TEXT NOT NULL, tags TEXT, type TEXT,
    created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, ...
)
-- plus: memories_vec_idx_shadow, memories_tags_vec_idx_shadow,
--       libsql_vector_meta_shadow, shard_metadata
```

**opencode-mem betreibt auf dieser Maschine bereits lokale Vektorsuche** —
768 Dimensionen via sqlite-vec, ohne API-Kosten. Das ist ein vorhandener Präzedenzfall
für eine semantische Phase in sift: lokale Embeddings sind hier keine Theoriefrage,
sondern laufen bereits.

**Coverage-Lücke:** 49.699 Messages gegenüber 265.225 Parts. Reasoning und
Tool-Outputs sind **nicht** abgedeckt — das sind ~230 MB der 284 MB bzw. ~81 %
des Volumens, in denen das eigentliche Debug- und Entscheidungswissen liegt.

## Toolchain auf dieser Maschine

| | |
|---|---|
| Rust | **installiert**, `~/.cargo/bin` + Toolchain `stable`; fehlt nur im PATH |
| Julia | **nicht installiert**, `pacman -Ss julia` → `extra/julia 2:1.13.0-2` |
| Node | 26.10.0, `node:sqlite` → SQLite 3.53.4, FTS5 + trigram + bm25 + snippet ✅ |
| `sqlite3` CLI | 3.53.4, FTS5 + trigram ✅ |
| sql.js 1.14.1 | SQLite 3.49.1, **FTS3 ja, FTS5 nein**, 2-GiB-wasm-Cap |

Bereits vorhandene Rust-Binaries: `~/.cargo/bin/rust-analyzer-mcp` (2 MB),
`~/.cargo/bin/kraken` (16 MB).

## Negativbefunde, die Arbeit sparen

**sql.js kann diese DB nicht öffnen.** Zwei unabhängige Blocker:

```
no such module: fts5
ERR_FS_FILE_TOO_LARGE: 5.457.508.304 > 2 GiB   (+ wasm-Cap 2 GiB)
```

Damit ist `/home/jan/projects/opencode-mcp` (sql.js, `src/index.ts`, 7 Tools) gegen
die echte DB **nicht lauffähig** — unabhängig davon, ob FTS5 gebraucht wird.
Das Projekt wird hier **nicht** angefasst; sift ist davon getrennt.

**FTS5-Bedarf im Quell-Schema:** es gibt keine `observations`-Tabelle, keine
`_fts`-Trigger, nichts woran man anknüpfen könnte. `research_fts` ist der einzige
vorhandene Index und wird aus den Gründen in `01-architektur.md` nicht benutzt.

**WAL-Falle:** die Quelle wird aktiv geschrieben. `immutable=1` überspringt
WAL/SHM und liefert still veraltete Ergebnisse. Nur `mode=ro`.
