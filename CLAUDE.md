# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Signal Bot Crawly is a self-hosted Signal messenger bot written in **pure Rust with zero external crate dependencies** (only std library). It bridges Signal group chats with Open WebUI to provide LLM-powered chat summaries, web search, image generation, and fact-checking. All HTTP, JSON, Base64, and cron scheduling are hand-implemented.

## Build & Run Commands

```bash
# Build
cargo build --release        # Output: target/release/signal-bot-crawly (~3MB)

# Run (requires env vars, see below)
cargo run --release

# Unit tests (no external dependencies)
cargo test --bin signal-bot-crawly

# Integration tests (requires Ollama on localhost:11434)
cargo test --test integration

# Lint
cargo clippy
```

## Required Environment Variables

| Variable | Required | Default | Purpose |
|----------|----------|---------|---------|
| `SIGNAL_PHONE_NUMBER` | Yes | — | Bot's registered Signal phone number |
| `OPEN_WEBUI_API_KEY` | Yes | — | API key from Open WebUI |
| `SIGNAL_API_URL` | No | `http://signal-api:8080` | signal-cli-rest-api endpoint |
| `OPEN_WEBUI_URL` | No | `http://open-webui:8080` | Open WebUI endpoint |
| `OLLAMA_MODEL` | No | `gpt-oss:20b` | Default LLM model |
| `POLL_INTERVAL` | No | `10` | Polling interval in seconds (1-300) |
| `SCHEDULE` | No | `weekly` | Summary frequency: daily/weekly/monthly |
| `SUMMARY_PROMPT` | No | built-in | Custom system prompt for summarization |
| `DM_PROMPT` | No | built-in | Custom system prompt for DM chat responses |
| `SEARCH_PROMPT` | No | built-in | Custom system prompt for search result summarization |
| `FACT_CHECK_PROMPT` | No | built-in | Custom system prompt for fact-checking |

## Architecture

```
Open WebUI (Ollama + SearXNG + ComfyUI)
           │ HTTP + Bearer Auth
    Signal Bot Crawly (Rust binary)
     ├─ 30-sec polling event loop
     ├─ Command routing (@bot mentions / DMs)
     ├─ In-memory message accumulation per group
     ├─ Reply chain tracking (~10k msg archive)
     ├─ Per-group model overrides
     └─ Scheduled summarization (09:00 UTC)
           │ HTTP
    signal-cli-rest-api (Signal gateway)
```

### Module Responsibilities

| File | Purpose |
|------|---------|
| `main.rs` | Event loop, command routing, all handler functions (summarize, search, imagine, fact-check) |
| `config.rs` | Environment variable parsing into `Config` struct |
| `signal.rs` | Signal API client: list groups, receive/send messages, send images, mention detection |
| `webui.rs` | Open WebUI client: LLM chat, web search, image generation, model listing |
| `http.rs` | Hand-written HTTP/1.1 client with chunked transfer encoding, Bearer auth, retry logic |
| `json.rs` | Hand-written JSON parser: extract strings/numbers/arrays, escape handling, nested structures |
| `scheduler.rs` | Cron-like scheduling with calendar math (leap years, month lengths, UTC) |
| `base64.rs` | Base64 encoder for image attachment payloads |

### Key Design Decisions

- **Zero dependencies**: All protocol handling (HTTP, JSON, Base64) is hand-implemented using only std. This is intentional — do not add external crates.
- **Blocking I/O**: Uses `thread::sleep` for polling, no async runtime. The 30-second poll interval is the core loop cadence.
- **In-memory state only**: Messages stored in `HashMap<group_id, Vec<Message>>`, archive in `HashMap<timestamp, Message>`. No database or persistence. Archive caps at ~10k entries (evicts oldest at 12k).
- **Destructive message reads**: Signal API returns messages once — they're consumed on read. Messages must be archived immediately.
- **Release profile optimized for size**: `-Oz`, LTO, single codegen unit, panic=abort, stripped binary.

### Bot Commands

Commands are triggered by `@bot` mentions in groups or plain text in DMs:
`summarize`, `search <query>`, `imagine <prompt>`, `models`, `use <model>`, `help`, `is this true?` (reply-chain fact-check).

DMs without a command trigger automatic LLM chat response.

## Docker

```bash
docker compose up -d signal-api   # Start Signal gateway first
docker compose up -d               # Start everything
```

Signal account linking: visit `http://localhost:8080/v1/qrcodelink?device_name=signal-bot` and scan with Signal mobile.
