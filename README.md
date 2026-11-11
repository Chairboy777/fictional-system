# Connect Planner

A personal itinerary planner for **PHUSE EU Connect 2026** (Glasgow, 15–18 November).

You tell it your existing interests, new interests, key topics and presenters you want to see. It builds a
clash-free plan for each day, mixes in a few "something different" sessions from streams you didn't mention, and
lets you refine the plan by clicking or by chatting with Claude.

## Run it

```bash
# optional: enables chat with Claude
echo "ANTHROPIC_API_KEY=sk-ant-..." > .env

docker compose up --build
# open http://localhost:8080
```

Without an API key everything still works except the chat. The chat then accepts simple commands instead:
`add ET11`, `remove ET11`, `details ET11`, `find shiny on Tue`, `what's on Tue 14:00`.

| Variable | Default | Purpose |
|---|---|---|
| `ANTHROPIC_API_KEY` | – | Enables chat with Claude |
| `CLAUDE_MODEL` | `claude-opus-5-5` | Model used for chat |
| `CLAUDE_EFFORT` | `medium` | `low` makes replies faster and cheaper |
| `REFRESH_ON_START` | `1` in compose | Pull the latest agenda from the PHUSE site at start-up; falls back to `data/sessions.json` |
| `PORT` | `8080` | |

To refresh the bundled agenda file without starting the server:
`docker run --rm -v "$PWD/data:/app/data" phuse-planner phuse-planner fetch`

## What you get

- **Day-by-day timeline** with the stream, title, presenters and their companies, room, audience level, and why
  each session was picked. Every card expands to show the abstract.
- **Alternatives** for every slot (everything running at the same time, most relevant first) with one-click swap.
- **Open slots** where nothing was picked, listing what's on.
- **Browse all** to search all 266 agenda items.
- **Chat** to refine the plan, e.g. "swap the advanced sessions on Tuesday for beginner ones", "what's on at 14:00
  on Wednesday?", "I've changed my mind, I'm more interested in real-world evidence than define.xml".
- **Export .ics** for your calendar, and **Print** for all days.

Your interests, itinerary and chat are kept in your browser's local storage. The server keeps no per-user state.

## How it works

```
src/
  fetch.rs      Reads the event id and snapshot versions from the public agenda page, then pages
                through the same public GraphQL "Sessions" query the page uses. Normalises into data/sessions.json.
  model.rs      Session / Speaker / Catalog types; clash rules (posters share one session, so never clash).
  text.rs       HTML→text, tokenising, light stemming (incl. -ise/-ize).
  recommend.rs  Scoring and itinerary building (below).
  chat.rs       Claude tool-use loop over the Messages API (raw HTTP), plus the offline command parser.
  main.rs       axum server; the static UI in static/ is compiled into the binary.
```

**Scoring.** Each interest phrase is scored against every session with a field-weighted BM25 (title ×3, stream ×2,
abstract, tags, companies), with a bonus for an exact phrase hit and a few domain synonyms (AI → LLM / machine
learning, R → Shiny / pharmaverse, RWE ↔ real-world data, …). Scores are normalised per phrase so every interest
counts equally, then weighted: key topics 1.5, new interests 1.2, existing interests 1.0. A presenter name match makes
a session a must-see; a company name matches any presenter from that company.

**Building the plan.** Sessions are added greedily without clashes in this order: sessions you chose → must-see
presenters → plenary keynotes → strong matches → "something different" picks (one or three per day from streams
outside your interests, preferring beginner-level sessions) → weaker matches to fill the remaining time.

**Chat.** Claude receives the full agenda (without abstracts) in a cached system prompt, your current profile and
plan with each message, and six tools: `search_sessions`, `get_session`, `sessions_at`, `add_session`,
`remove_session` and `rebuild_itinerary`. Rebuilding keeps the sessions you chose yourself. The browser holds the raw
API transcript and sends it back each turn, so the conversation is replayed unchanged. Requests opt into
server-side refusal fallback (`fallbacks: "default"`).

## Development

There is no local Rust toolchain required; everything builds in Docker.

```bash
docker run --rm -v "$PWD":/app -w /app rust:1.88-bookworm cargo test
docker build -t phuse-planner . && docker run --rm -p 8080:8080 phuse-planner
```

Agenda data © PHUSE, fetched from the public agenda at https://events.phuse.global/event/EUConnect26/agenda.
Speaker email addresses are never requested or stored.
