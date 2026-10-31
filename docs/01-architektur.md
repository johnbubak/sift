# sift — Architektur

## Kernidee

Die KI ist der **Konsument** von Wissen, nicht die Suchmaschine. Gesucht wird
ausschließlich in SQLite; in den Prompt gelangen nur ~30-Token-Snippets.

```
opencode.db  5.46 GB  WAL  read-only        sift.db   unser Besitz
  session  ─┐                              ┌─ sift_doc(...)          Metadaten + Body
  message  ─┼─ ATTACH ?mode=ro ─► extract ─┤─ sift_fts(title, body)  FTS5, external content
  part     ─┘                              ├─ sift_seen(scope,doc_id)  Anti-Redundanz
                                           └─ sift_meta(...)          Watermark
```

**Warum ein Sidecar und nicht `research_fts`?** Fünf Gründe, alle gemessen:

1. Coverage — `research_fts` deckt Messages ab, nicht Parts. ~19 % des Volumens.
2. Alterung — keine Trigger, keine Zeitstempel, Verfasser nicht auffindbar.
3. Schreibrecht — in eine 5,46-GB-WAL-DB eines Fremdsystems schreiben wir nicht.
4. Tokenizer — unicode61 ist dort fest verdrahtet.
5. Dedup — `sift_seen` ist Kern des Features und existiert dort nicht.

**Warum Trigger im Sidecar?** Weil die Datei uns gehört. External-Content-FTS5
ohne Trigger driftet bei jedem `INSERT OR IGNORE` auseinander; mit Triggern ist
Extraktion idempotent und überlappende Zeitfenster sind harmlos. Die Warnung
„keine Trigger anlegen" aus der Planungsphase galt der **Quell**-DB, nicht dem
Sidecar.

## Schema

`migrations/001_sidecar.sql`

* `sift_doc` — eine Zeile = ein Quell-`part`. `part_id UNIQUE` macht Extraktion
  idempotent. `body_fp` ist ein Fingerprint für Corpus-Statistiken und als
  Prototyp-Ersatz für einen echten Hash (Rust: Phase 1).
* `sift_fts` — external content über `sift_doc(title, body)`.
  **Halber Speicherbedarf** gegenüber standalone, und mit
  `INSERT INTO sift_fts(sift_fts) VALUES('rebuild')` jederzeit reparierbar.
* `sift_seen` — Anti-Redundanz. `WITHOUT ROWID`, PK `(scope, doc_id)`.
* `sift_meta` — `watermark_ms`, `last_sync`.

### Tokenizer-Entscheidung

`unicode61 remove_diacritics 2`, **kein trigram**.

Trigram ist verfügbar (SQLite ≥ 3.34, hier 3.53) und findet Teilwörter und
Tippfehler ohne Stemmer. Es indexiert aber jedes 3-Zeichen-Fenster: Indexgröße
~3–5× des Textes, und **Suchbegriffe unter 3 Zeichen treffen nie**. Für ~284 MB
Text ist das eine spürbare Investition für einen unproven Nutzen.

Der Weg zu Rekalle ist billiger: Präfixqueries (`term*`) über unicode61, die der
Sanitizer ohnehin erzeugt. Trigram ist ein **Experiment für Phase 5**, nicht die
Grundannahme.

## Extraktion

`sql/extract.sql`, idempotent über `INSERT OR IGNORE` + Watermark.

* Quelle read-only per `ATTACH 'file:…?mode=ro'`, **niemals** `immutable=1`
* `time_created >= watermark` (nicht `>`) — opencode schreibt mehrere Parts mit
  derselben ms; die Überlappung fängt `part_id UNIQUE` ab
* Tool-Output auf 8.192 Zeichen gekappt — bash dominiert mit 166 MB
* Nicht indiziert: `step-start`, `step-finish`, `agent`, `compaction` (Boilerplate),
  `file` (88 Zeilen, 35 MB base64). `patch` hat keinen Text an `$.patch`.

Messung: 4.000 Rows in 0,3 s → voller Lauf ~265k Rows in der Größenordnung
einer Minute, davon ist der FTS-Aufbau der teure Teil.

## Suche

`sql/search.sql`

```
BM25(Titelgewicht, 3.0) ─┐
                         ├─ RRF:  1/(60+r_bm) + w_rec/(60+r_rec)
Recency(time_created)  ──┘
        │
        └─► Diversity: snip_rank = 1, sess_rank <= 2  ─►  LIMIT
```

**RRF statt handgewichteter Matrix.** Recency und BM25 sind zwei Rankings, kein
gemeinsamer Score. `Σ 1/(k + rank)` fusioniert sie parameterarm — `k = 60` ist
der Standardwert und muss nicht getunt werden.

**snippet() zählt Tokens, nicht Wörter.** `snippet(sift_fts, 1, '«','»',' … ', 30)`
≈ 30 Tokens pro Treffer; `limit 3` hält eine Suche unter ~150 Tokens Fremdkontext.
`snippet()` **muss** in der Query mit dem `MATCH` berechnet werden, nicht im
outeren SELECT.

### Diversität (aus Messung, nicht aus Theory of Mind)

| Befund (4.000-Row-Stichprobe) | Konsequenz |
|---|---|
| 8,8 % der Bodies sind byte-identisch bei verschiedenen `part_id` | Duplikate *innerhalb* einer Ergebnisliste |
| alle Top-3-Treffer aus **einer** Session | `sess_rank <= 2` |
| zwei Dokumente aus zwei Sessions mit identischem Snippet-Fenster | Dedup auf **Snippet**, nicht Body |

`snip_rank` ist ein harter Filter und kann bei kleinen Korpora die Ergebnisliste
leerziehen (im 4.000-Row-Test: 1 Treffer statt 5). Deshalb über-fetcht `top`
50 Kandidaten. Auf dem vollen Korpus ist die Vielfalt deutlich größer — vor
der Freigabe messen, nicht annehmen.

## Anti-Redundanz — zwei Schichten

Der ursprüngliche Entwurf ließ den Agenten bereits gesehene IDs mitschicken.
Das kostet genau die Tokens, die gespart werden sollen. Die Server-Schicht
löst das ohne Tokenkosten:

1. **Scope** (Primärweg): Der Agent übergibt **einmalig** eine Scope-ID
   (Conversation). Ab da filtert der Server selbst:
   `AND d.id NOT IN (SELECT doc_id FROM sift_seen WHERE scope = ?4)`.
   Nach der Antwort werden die gelieferten `doc_id`s in `sift_seen` geschrieben.
2. **`exclude`-Parameter** (Fallback): falls Sidecar-State verloren geht.

`LIMIT` steht **nach** dem Filter — sonst liefert die Query stillschweigend zu wenig.

### Bekannte Lücke

Snippet-Dedup greift nur innerhalb **einer** Ergebnisliste. Wurde `doc 2694` in
einem Scope gemerkt, rückt beim nächsten Mal `doc 2693` mit **identischem Snippet**
nach (im Test beobachtet). Abhilfe: zusätzlich einen Snippet-Fingerprint in
`sift_seen` speichern, nicht nur `doc_id`. Phase-1-Aufgabe, siehe `03-phasen.md`.

## Query-Sanitizer

Freier Nutzertext ist keine FTS5-Query. `"foo-bar"`, `NEAR`, `*`, `:`, `(`, `-`
als Query erzeugen Syntaxfehler. `" AND ".join(terms)` crasht bei Anführungszeichen.

Regeln: in Whitespace zerlegen, jedes Token in `"` klammern, innere `"` verdoppeln,
Tokens ≥ 3 Zeichen mit `*` versehen (Präfixrecherche). Phrasen in Anführungszeichen
durchreichen. Ergebnis ablehnen, wenn es leer ist.

## Rust ↔ Julia

**Gewichtsdatei.** `weights.toml` wird von Julia erzeugt (Sweep gegen gelabelte
Queries) und von Rust zur Startzeit gelesen. Kein Julia im Runtime-Pfad.

Begründung gegen `jlrs`: der Hot Path ist FTS5-Matching und -Ranking in
optimiertem C mit mmap. Julia bringt dort keine Beschleunigung, dafür JIT-Startup,
zwei Build-Systeme und eine C-ABI-Grenze, die gepflegt werden muss. Für ein
verteilbares Binary ist das der falsche Preis.

`weights.toml` (Defaults, von Julia überschreibbar):

```toml
recency_weight   = 0.3
title_weight     = 1.0
body_weight      = 3.0     # steht in search.sql, hier zur Dokumentation
snippet_tokens   = 30
max_per_session  = 2
limit_default    = 3
rrf_k            = 60
```

## Tokens im Prompt

```
💡 [sift] <directory> · <session_title> · <kind>[/<tool>] · <zeit>
   «…30 Tokens…»
```

Ein Treffer, eine Zeile Herkunft, 30 Tokens Inhalt. Kein Titel, kein Zeitstempel,
kein JSON-Blob im Prompt.
