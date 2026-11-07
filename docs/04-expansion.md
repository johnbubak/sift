# sift — Expansion: Git, Feedback, Web

Ziel: sift von Session-Recall zu allgemeinem Recall-Layer erweitern, ohne
Kernphilosophie (lokal, read-only, token-budgetiert) zu brechen.

## Leitplanken (unverhandelbar)

- Kein Schreiben in fremde Systeme ohne Opt-in.
- Keine Telemetrie; Feedback nur explizit (`sift feedback`).
- Pro-Features hinter Cargo-Feature-Gates (`pro`, `git`, `web`).
- Jede neue Quelle als eigene Sidecar-Tabelle + eigener Watermark.

## Phase A — Git/Gitea/GitHub (Pro)

Ziel: Commit-/PR-/Issue-Historie lokal durchsuchbar machen.

Kandidaten:
- `sift git add <repo-url>`: shallow clone in `~/.local/share/sift/repos/<hash>`.
- `sift git sync`: fetch + index neue Commits/PRs/Issues.
- Quellen: `git log`, GitHub/Gitea REST API (Issues, PRs, Reviews).
- Schema: `git_doc(id, repo, kind, ref, author, ts, body, body_fp)`.
- Filter: `--repo`, `--author`, `--since`, `--kind`.
- Token-Budget: Commit-Message + Diff-Stat, nicht ganzer Diff.

Entry: Phase 4 (Distribution) mindestens lauffähig; `git` CLI vorhanden.
Verify: `sift git sync <repo>`; `sift search <term> --repo <name>`; `cargo test`.

## Phase B — Feedback-Loop (Core, opt-in)

Ziel: anonymisierte Nutzungs-/Audit-Daten als Ticket an sift zurücksenden.

- `sift feedback`: erzeugt GitHub/Gitea-Issue mit:
  - Version, OS, Feature-Flags
  - Aggregatwerte (Docs, Treffer, Spur-Signale)
  - Keine Inhalte aus opencode.db.
- Nutzt `gh` CLI oder Gitea-API; nur nach Bestätigung.
- Rate-Limit: max 1/Tag.

Entry: Phase A stabil.
Verify: `sift feedback --dry-run` zeigt Payload; kein DB-Inhalt.

## Phase C — Web-Suche (Pro)

Ziel: Online-Inhalte siften und cachen.

- `sift web <query>`: nutzt konfigurierbare Suchmaschine (Exa/Brave/SearXNG).
- Cache in Sidecar (`web_doc`), TTL 7 Tage.
- Nur Snippets + URL, nie Volltext dauerhaft (robots/ToS beachten).
- Opt-in API-Key via `SIFT_SEARCH_API_KEY`.

Entry: Phase A/B stabil; API-Key vorhanden.
Verify: `sift web rust fts5` liefert ≥3 Treffer; Cache-Hit bei Wiederholung.

## Koordinations-Startprompt

Neue Session mit:
1. Diese Datei lesen.
2. `docs/02-adr.md` prüfen (verworfene Wege).
3. Nur eine Phase pro Session; Entry-Condition zuerst verifizieren.
4. Jede neue Quelle: eigene Tabelle + Watermark + `verify-sql.sh`-Analogon.
5. Keine Zahl ohne Messung.

## Offene Fragen (für User) — entschieden in Phase A (ADR-7)

- Welche Git-Hosts priorisieren (GitHub, Gitea, GitLab)?
- Feedback an GitHub-Issues oder Gitea?
- Web-Suche: Exa (bereits im Tooling) oder SearXNG self-hosted?
- Pro-Gating: `git`/`web` als eigene Features oder unter `pro`?