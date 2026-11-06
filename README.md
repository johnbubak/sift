# sift

**Session recall for coding agents.** The agent is the consumer of knowledge, not
the search engine: queries run entirely in SQLite; only ~30-token snippets with
provenance reach the prompt.

```console
$ sift search surrealdab async integration --limit 3 --scope demo
# marked 3 doc(s) as seen in scope 'demo'
💡 [sift] /home/jan/projects/hictus · Pipeline integrity and healer … · text · 2026-06-21 18:44
    … WS-«Integration»** | Thread + «asyncio» Hack | Bereits da (MPSC) | 🦀 |
| **State Management** | Dict mit GIL | Struct + Ownership | 🦀 |
```

The same call with the same scope returns **different** hits. The server remembers
what it already delivered — the agent never passes a single ID.

[![CI](https://github.com/johnbubak/sift/actions/workflows/ci.yml/badge.svg)](https://github.com/johnbubak/sift/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.96%2B-orange.svg)](https://www.rust-lang.org)

## Why

Coding agents (opencode, claude-code, …) accumulate thousands of session
messages. When a question like "how did we integrate surrealdb last time?"
comes up, the agent either re-derives the answer or stuffs the whole history
into context. sift indexes opencode's `opencode.db` into a local sidecar
(`sift.db`) with FTS5 + BM25 + RRF + snippet budgeting, and keeps track of
what it already handed out per scope.

## Status

| Phase | | |
|---|---|---|
| 0 · SQL core | ✅ | Schema, extraction, FTS, RRF, dedup — `scripts/verify-sql.sh` PASS |
| 1 · Indexer | ✅ | `sift build`: 169,013 docs (v1+v2) in 41.6 s |
| 2a · Token budget | ✅ | FTS-tokens ≠ LLM-tokens fixed, OR instead of AND, `should-search` |
| 2 · MCP server | ✅ | stdio server with `sift_search` and `sift_spur`, verified |
| 3 · Skill | ✅ | live, `opencode debug skill` shows sift |
| 4 · Distribution | ⬜ | single binary, npm |
| 5 · Measurements | ⬜ | trigram? embeddings? only with entry condition |
| 6 · Julia lab | ⬜ | weights not tuned |

## Install

```sh
git clone https://github.com/johnbubak/sift.git
cd sift
cargo build --release
# binary at ./target/release/sift  (or: cp to ~/.local/bin/sift)
```

Requires Rust 1.96+. SQLite is bundled (FTS5 compiled in).

## Usage

```sh
# Build the sidecar index from opencode.db (read-only source)
sift build

# Search
sift search "surrealdb async integration" --limit 5 --scope demo

# Extract matching lines from one file, with context
sift extract src/main.rs --for "surrealdb"

# Should I search at all? (fail-open detector)
sift should-search "how did we do X last time"

# Self-audit of the agent's own trace
sift spur --limit 5
```

Configuration via env vars: `SIFT_SOURCE` (opencode.db), `SIFT_DB` (sidecar),
`SIFT_WEIGHTS` (weights.toml). Defaults live under `~/.local/share/...`.

## Rules

1. **`opencode.db` is never written.** No triggers, no migrations, no
   `immutable=1` — the DB is 5.46 GB, WAL mode, actively written.
2. **No triggers in the source.** Triggers in `sift.db` are correct and required.
3. **SQL lives in files**, not Rust strings. After any SQL change run
   `scripts/verify-sql.sh`.
4. **No invented numbers.** Every number in the docs is measured or marked as
   an estimate.
5. **No feature without an entry condition.** See `docs/03-phasen.md` Phase 5.

## Architecture

See `docs/`:
- `00-archeologie.md` — measured facts about the source DB
- `02-adr.md` — decisions, including rejected paths
- `03-phasen.md` — what counts as done, entry conditions

## ☕ Support this project

Built something with sift? Coffee appreciated ☕

| Method | Link |
|---|---|
| ⭐ GitHub Sponsors | [github.com/sponsors/johnbubak](https://github.com/sponsors/johnbubak) |
| ☕ Ko-fi | [ko-fi.com/johnbubak](https://ko-fi.com/johnbubak) |
| 💳 PayPal | [paypal.me/janBobka](https://paypal.me/janBobka) |
| 🟡 Binance Pay | [JohnBubak](https://app.binance.com/qr/dplkj8) |

## License

MIT — see [LICENSE](LICENSE). The core is free; advanced MCP features may be
offered separately.
