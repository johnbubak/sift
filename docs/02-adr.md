# sift — Entscheidungen

Format: Entscheidung · Begründung · **verworfene Alternativen und warum**.
Die verworfenen Wege stehen hier, weil sie in einer späteren Session sonst erneut
vorgeschlagen werden.

---

## ADR-1 · Sidecar-Index statt `research_fts`

**Entscheidung.** Eigenes `sift.db`, gefüllt aus `opencode.db` per read-only ATTACH.

**Begründung.** Fünf gemessene Gründe: Coverage-Lücke (~19 % des Volumens, Tool-Outputs
und Reasoning fehlen), fehlende Trigger und Zeitstempel (Alterung nicht nachweisbar),
Schreibrecht auf einer fremden 5,46-GB-WAL-DB, fest verdrahteter Tokenizer, fehlende
Dedup-Tabelle.

**Verworfen — `research_fts` direkt nutzen.** Null Aufbaulauf, aber eingefrorener
Snapshot eines nicht auffindbaren Skripts, und das Kernproblem (Tool-Wissen fehlt)
bliebe ungelöst.

**Verworfen — Trigger in `opencode.db`.** opencode schreibt dort aktiv. Ein
Fremdsystem, das in dessen Datenbank Trigger anlegt, erzeugt Konflikte bei jedem
opencode-Schemaupdate.

**Verworfen — gemeinsame Datenbank per ATTACH der Quelle in unsere Datei.** Geht,
ist im Kern dasselbe; `ATTACH` ist hier nur der billigere Weg für denselben
Lesevorgang.

---

## ADR-2 · Rust als Runtime

**Entscheidung.** Rust (1.96, Toolchain vorhanden) mit `rmcp` + `rusqlite{bundled}`.

**Begründung.** `rusqlite` kompiliert SQLite mit `-DSQLITE_ENABLE_FTS5` **unbedingt**
mit (`libsqlite3-sys/build.rs:163`) — kein Feature-Flag-Raten. `bundled` genügt,
`bundled-full` wäre Overkill. `-DSQLITE_USE_URI` ist gesetzt, damit
`file:…?mode=ro` funktioniert. `snippet()`/`bm25()` sind FTS5-Aux-Functions und
werden als normale Skalarfunktionen gelesen — das `functions`-Feature ist hier
**nicht** nötig (es ist für Rust-Callbacks *in* SQLite). `rmcp` 3.5.1 ist das
offizielle SDK der MCP-Organisation mit stdio-Transport und 33 Mio. Downloads;
alle Alternativen (`mcp-server`, `fastmcp-rs`, `rust-mcp-schema`) sind aufgegeben,
Typhasen oder reine Typdefinitionen. Ergebnis: **eine** Codebasis, die zugleich
Single-Binary und MCP-Server ist.

**Verworfen — TypeScript zuerst.** Schneller zum ersten Treffer und der MCP-SDK
lag schon vor. Aber zwei Codebasen, und der Rust-Port wäre eine zweite
Implementierung derselben SQL-Logik. Der Verzicht kostet ~1 Tag.

**Verworfen — sql.js.** Nicht Präferenz, sondern Messung: kein FTS5, und die Datei
lässt sich physisch nicht öffnen (5,46 GB > 2-GiB-wasm-Cap). `/home/jan/projects/opencode-mcp`
ist aus diesem Grund tot und wird hier nicht angefasst.

**Nicht gebraucht.** `node:sqlite` (3.53.4, FTS5 + trigram, live verifiziert) wäre
die schnellere Abkürzung — sie ist nicht nötig, seit ADR-2 steht.

---

## ADR-3 · Gewichtsdatei statt `jlrs`

**Entscheidung.** Julia erzeugt `weights.toml`; Rust liest es zur Startzeit.
Kein Julia-Prozess im Runtime-Pfad.

**Begründung.** Der Hot Path ist FTS5-Matching plus Ranking — optimiertes C mit
mmap. Lineare Algebra beschleunigt daran nichts. `jlrs` würde dafür JIT-Startup,
zwei Build-Systeme und eine C-ABI-Grenze kosten, die bei jeder Julia-Signaturänderung
mitwandert.

**Verworfen — `jlrs` (Julia im Rust-Prozess).** Null IPC und echte In-Process-Matrix-
Operationen. Aber: `ki_such_schleife` in Julia zu *skillen* ist Overkill — RRF ist
fünf Zeilen Arithmetik. Und 3–5 s Kaltstart für einen Pfad ohne Julia-Bedarf.

**Verworfen — Langzeit-lebender Julia-Worker als Kindprozess.** Die Mitte, die am
ehesten sauber funktioniert (~300 ms amortisiert). Scheidet aus, solange die
Gewichte in einer Datei stehen können — eine weniger bewegliche Komponente ist
zwei zusätzliche beweglichen wert.

**Julia behält eine Aufgabe:** die Gewichte *empirisch* zu bestimmen. `rrf_k`,
Recency-Decay, Spaltengewichte und `max_per_session` gegen ein gelabeltes
Query-Set zu sweepen ist eine Optimierungsaufgabe, und dafür ist Julia das
richtige Werkzeug. Offline, als Labor.

---

## ADR-4 · RRF statt handgewichteter Gewichtung

**Entscheidung.** `score = 1/(60 + r_bm25) + w_rec/(60 + r_recency)`.

**Begründung.** Zwei Rankings, kein gemeinsamer Score. RRF braucht keine
skalierten Gewichte — der Grund, warum `bm25()`-Werte und Zeitstempel sich nicht
vergleichen lassen. `k = 60` ist der Standardwert aus der IR-Literatur und muss
nicht getunt werden.

**Verworfen — Spreading Activation (Matrix-Multiplikation).** Der Ansatz aus der
Vorbetrachtung. Scheitert daran, dass im Beispiel die Kanten `A[1,2] = 0.8`
**handgemalt** sind. Auf echtem Session-Text kennt man die Kanten nicht vorab, man
müsste sie aus Co-Occurrence oder Embeddings lernen. Die lineare Algebra ist der
letzte Schritt, nicht der erste.

**Verworfen — Gewichte als freie Parameter.** Sobald man `a*bm25 + b*recency`
einstellen kann, muss man sie gegen etwas tun. RRF hat nur eine Stellschraube
statt drei.

**Verworfen — Rekursives SQL (`WITH RECURSIVE`) als Suchschleife.** Der Code aus
der Vorbetrachtung läuft nicht: `JOIN fts_documents doc ON doc.content MATCH loop.search_term`
ist kein gültiges FTS5, `MATCH` verlangt die FTS-Tabelle als linken Operanden.
Selbst korrigiert bringt die Rekursion nichts: `bm25()` kann erst *nach* der
Zeilenauswahl gewichten. Query-Expansion gehört in den Rust-Request-Pfad, nicht
in ein CTE.

---

## ADR-5 · Embeddings sind Phase 5, nicht Kern

**Entscheidung.** Semantische Suche wird als messbares Experiment behandelt, nicht
als Ausgangspunkt.

**Begründung.** `sqlite-vec` + OpenAI-Embeddings widerspricht der Kernphilosophie
(lokal, die DB sucht, 0 API-Tokens) und verlangt eine API-Rechnung für ~50k+
Dokumente — bei privaten Session-Transkripten zusätzlich ein Datenschutzproblem.
ChromaDB ist abgekündigt; `sqlite-vec` bleibt als einzige SQLite-native Option.

**Verworfen — OpenAI `text-embedding-3-small`.** Kostet und exfiltriert.

**Vorgemerkt.** Falls Semantik gewünscht ist: lokales Modell (ONNX/transformers.js),
`vec0`-Tabelle in `sift.db`, Hybrid über RRF(FTS, Vektor). Eintrittsbedingung ist ein
Messwert: 10 gelabelte Fragen, Hybrid muss FTS-only messbar schlagen.

---

## ADR-6 · Anti-Redundanz serverseitig

**Entscheidung.** `sift_seen(scope, doc_id)`; der Agent übergibt **eine** Scope-ID
pro Conversation, keine ID-Listen.

**Begründung.** Der ursprüngliche Entwurf (Agent übergibt gesehene `obs_id`s als
`NOT IN (...)`) kostet genau die Tokens, die gespart werden sollen, und
vergisst zuverlässig mal welche.

**Verworfen — ID-Liste im Query.** Siehe Begründung.

**Verworfen — nur Agent-seitig merken.** Fehleranfällig, nicht tokenfrei.

**Offen.** Snippet-Fingerprint zusätzlich zu `doc_id` speichern, sonst rückt der
Zwillings-Snippet eines gemerkten Dokuments im nächsten Scope-Durchlauf nach
(im Test mit `doc 2694` → `doc 2693` beobachtet).
