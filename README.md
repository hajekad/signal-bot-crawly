<p align="center">
  <h1 align="center">Signal Bot Crawly</h1>
  <p align="center">
    A zero-dependency Rust bot that brings AI capabilities to Signal group chats — powered by Open WebUI.
  </p>
  <p align="center">
    <strong>Chat Summaries</strong> &bull; <strong>Web Search</strong> &bull; <strong>Image Generation</strong> &bull; <strong>Multi-Model Support</strong>
  </p>
</p>

---

## Overview

Signal Bot Crawly is a self-hosted Signal messenger bot written in pure Rust with **zero external crate dependencies**. It acts as middleware between Signal and [Open WebUI](https://github.com/open-webui/open-webui), giving your group chats access to LLM summarization, web search (via SearXNG), and image generation (via ComfyUI) — all running locally on your own hardware.

### Key Features

- **Zero dependencies** — Pure Rust using only `std`. Hand-written HTTP client, JSON parser, base64 encoder, and scheduler.
- **Chat summaries** — On-demand or scheduled (daily/weekly/monthly) summaries of group conversations, attributed by speaker name.
- **Web search** — Query the web through SearXNG with AI-summarized results.
- **Image generation** — Generate images from text prompts via ComfyUI, sent as Signal attachments.
- **Per-group model selection** — Each group can use a different LLM model.
- **Auto-discovery** — Automatically monitors all groups the bot is a member of, with dynamic refresh.
- **Privacy-first** — All data stays on your machine. No cloud APIs, no telemetry.

## Architecture

```
                    ┌─────────────────────────────────────────┐
                    │              Open WebUI                  │
                    │  ┌─────────┐  ┌────────┐  ┌──────────┐ │
                    │  │ Ollama  │  │SearXNG │  │ ComfyUI  │ │
                    │  │  (LLM)  │  │(Search)│  │ (Images) │ │
                    │  └────┬────┘  └───┬────┘  └────┬─────┘ │
                    │       └───────────┴────────────┘        │
                    └──────────────────┬──────────────────────┘
                                       │ HTTP + Bearer Auth
                                       │
┌──────────┐    HTTP     ┌─────────────┴──────────────┐
│  Signal  │◄──────────► │    Signal Bot Crawly        │
│  Users   │             │    (Rust binary, ~3MB)       │
└──────────┘             └─────────────┬──────────────┘
                                       │ HTTP
                           ┌───────────┴───────────┐
                           │ signal-cli-rest-api    │
                           │ (Signal gateway)       │
                           └───────────────────────┘
```

The bot polls Signal for messages every 30 seconds, detects `@mention` commands, routes them through Open WebUI's API, and sends results back. All communication uses plain HTTP over localhost — no TLS needed.

## Commands

Tag the bot using Signal's native @mention in any group chat:

| Command | Description |
|---|---|
| `@bot help` | Show available commands and current model |
| `@bot summarize` | Summarize all messages since the last summary |
| `@bot search <query>` | Search the web and return AI-summarized results |
| `@bot imagine <prompt>` | Generate an image from a text prompt |
| `@bot models` | List available LLM models |
| `@bot use <model>` | Switch the LLM for this group |

Scheduled summaries also run automatically at 09:00 UTC (configurable as daily, weekly, or monthly).

## Prerequisites

You need the following services running **before** starting the bot:

| Service | Purpose | Default URL |
|---|---|---|
| [Open WebUI](https://github.com/open-webui/open-webui) | AI middleware (required) | `http://localhost:3000` |
| [Ollama](https://ollama.ai) | LLM inference (required) | `http://localhost:11434` |
| [SearXNG](https://github.com/searxng/searxng) | Web search (optional) | `http://localhost:8888` |
| [ComfyUI](https://github.com/comfyanonymous/ComfyUI) | Image generation (optional) | `http://localhost:8188` |

### Open WebUI Configuration

Open WebUI must be configured with the following:

1. **API keys enabled** — Set the environment variable `ENABLE_API_KEYS=True` on the Open WebUI container.
2. **Ollama connected** — Set `OLLAMA_BASE_URL` pointing to your Ollama instance.
3. **SearXNG connected** (for search) — Configure the web search integration in Open WebUI admin settings to point to your SearXNG instance (`http://localhost:8888`).
4. **ComfyUI connected** (for images) — Set `COMFYUI_BASE_URL`, `IMAGE_GENERATION_ENGINE=comfyui`, and `ENABLE_IMAGE_GENERATION=true` on the Open WebUI container.

Pull at least one chat model in Ollama:

```bash
ollama pull dolphin-mistral
```

## Setup

### 1. Start signal-cli-rest-api

```bash
docker compose up -d signal-api
```

This starts the [signal-cli-rest-api](https://github.com/bbernhard/signal-cli-rest-api) container which acts as a gateway between the bot and the Signal network.

### 2. Link a Signal account

You need a dedicated Signal account for the bot (a separate phone number).

Open the QR code link in your browser:

```
http://localhost:8080/v1/qrcodelink?device_name=signal-bot
```

On the phone with the bot's Signal account:

1. Open Signal
2. Go to **Settings → Linked Devices**
3. Tap **"+"** (Link New Device)
4. Scan the QR code from the browser

Verify the account is linked:

```bash
curl http://localhost:8080/v1/accounts
# Should return: ["+1234567890"]
```

### 3. Generate an Open WebUI API key

1. Open **http://localhost:3000** and log in
2. Click your **profile avatar** (bottom-left)
3. Go to **Settings → Account**
4. Scroll to **API Keys** and click **"+"**
5. Copy the generated key

### 4. Store credentials

```bash
mkdir -p ~/.config/signal-bot-crawly

# Store your Open WebUI API key
echo -n "your-api-key-here" > ~/.config/signal-bot-crawly/api_key
chmod 600 ~/.config/signal-bot-crawly/api_key

# Store your bot's phone number
echo -n "+1234567890" > ~/.config/signal-bot-crawly/phone_number
chmod 600 ~/.config/signal-bot-crawly/phone_number
```

### 5. Start the bot

#### Option A: Run directly

```bash
export SIGNAL_API_URL=http://localhost:8080
export OPEN_WEBUI_URL=http://localhost:3000
export OPEN_WEBUI_API_KEY=$(cat ~/.config/signal-bot-crawly/api_key)
export SIGNAL_PHONE_NUMBER=$(cat ~/.config/signal-bot-crawly/phone_number)
export OLLAMA_MODEL=dolphin-mistral
export SCHEDULE=weekly

cargo run --release
```

#### Option B: Run with Docker Compose

Edit `docker-compose.yml` with your values:

```yaml
- SIGNAL_PHONE_NUMBER=+1234567890
- OPEN_WEBUI_API_KEY=your-api-key-here
- OLLAMA_MODEL=dolphin-mistral
```

Then:

```bash
docker compose up -d
```

### 6. Add the bot to Signal groups

Add the bot's phone number to any Signal group. The bot auto-discovers all groups it's a member of and refreshes the list dynamically.

## Configuration

All configuration is via environment variables:

| Variable | Required | Default | Description |
|---|---|---|---|
| `SIGNAL_PHONE_NUMBER` | Yes | — | Bot's registered Signal phone number |
| `OPEN_WEBUI_API_KEY` | Yes | — | API key from Open WebUI |
| `SIGNAL_API_URL` | No | `http://signal-api:8080` | signal-cli-rest-api URL |
| `OPEN_WEBUI_URL` | No | `http://open-webui:8080` | Open WebUI URL |
| `OLLAMA_MODEL` | No | `gpt-oss:20b` | Default LLM model |
| `SCHEDULE` | No | `weekly` | `daily`, `weekly`, or `monthly` |
| `SUMMARY_PROMPT` | No | *(built-in)* | Custom system prompt for summarization |

## Project Structure

```
signal-bot-crawly/
├── Cargo.toml              # Zero dependencies, optimized release profile
├── Dockerfile              # Multi-stage Alpine build (~3MB final image)
├── docker-compose.yml      # signal-api + bot services
├── src/
│   ├── main.rs             # Event loop, command routing, orchestration
│   ├── config.rs           # Environment variable configuration
│   ├── http.rs             # Pure TcpStream HTTP/1.1 client (GET/POST, chunked, auth)
│   ├── json.rs             # Hand-written JSON parser and serializer
│   ├── signal.rs           # Signal API client (groups, messages, mentions, attachments)
│   ├── webui.rs            # Open WebUI client (chat, search, image gen, model listing)
│   ├── scheduler.rs        # Cron-like scheduling with UTC date math
│   └── base64.rs           # Base64 encoder for image attachments
└── tests/
    └── integration.rs      # Integration tests (requires running Ollama)
```

## Development

```bash
# Run all tests (unit tests don't require external services)
cargo test --bin signal-bot-crawly

# Run integration tests (requires Ollama on localhost:11434)
cargo test --test integration

# Lint
cargo clippy

# Build optimized release binary
cargo build --release
```

## How It Works

1. **Startup** — Loads config, resolves the bot's UUID via the Signal API, fetches all groups, clears stale messages.
2. **Poll loop** — Every 30 seconds, fetches new messages from Signal (destructive read — messages are consumed).
3. **Mention detection** — Checks the `mentions` array in each message envelope for the bot's UUID.
4. **Command parsing** — Strips the U+FFFC mention placeholder, matches against known commands.
5. **Execution** — Routes to the appropriate handler (summarize, search, imagine, models, use, help).
6. **Response** — Sends results back to the group via Signal, including formatted text and image attachments.
7. **Scheduled runs** — At the configured interval, summarizes all accumulated messages across all groups.
8. **Group refresh** — Dynamically discovers new groups when receiving messages from unknown group IDs.

## License

[AGPLv3](LICENSE)
