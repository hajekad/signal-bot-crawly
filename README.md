# Signal Bot Crawly

A zero-dependency Rust Signal bot powered by Open WebUI — chat summaries, web search, and image generation.

## Features

- **Zero dependencies** — pure Rust with only `std`, no external crates
- **Chat summaries** — @mention the bot to summarize group conversations on demand
- **Web search** — `@bot search <query>` searches the web via SearXNG
- **Image generation** — `@bot imagine <prompt>` generates images via ComfyUI
- **Scheduled summaries** — automatic daily, weekly, or monthly summaries
- **Open WebUI middleware** — routes all AI through Open WebUI, inheriting its configured integrations
- **Auto-discovers groups** — monitors all groups the bot is a member of
- **Identifies speakers** — summaries attribute messages to sender display names

## Architecture

```
Signal ←→ signal-cli-rest-api ←→ signal-bot-crawly ←→ Open WebUI
                                                         ├── Ollama (LLM)
                                                         ├── SearXNG (search)
                                                         └── ComfyUI (images)
```

The bot uses `std::net::TcpStream` for HTTP/1.1 — no TLS needed since everything stays on localhost.

## Setup

### 1. Prerequisites

You need these services running:
- **Open WebUI** with API keys enabled (`ENABLE_API_KEYS=True`)
- **Ollama** with at least one chat model pulled
- **SearXNG** (optional, for web search)
- **ComfyUI** (optional, for image generation)

### 2. Start signal-cli-rest-api

```bash
docker compose up -d signal-api
```

### 3. Link your Signal account

Open in your browser:
```
http://localhost:8080/v1/qrcodelink?device_name=signal-bot
```

On the phone with the bot's Signal account: **Settings → Linked Devices → "+"** → scan the QR code.

### 4. Generate an Open WebUI API key

In Open WebUI: **Profile → Settings → Account → API Keys → "+"**

### 5. Configure and start the bot

```bash
export SIGNAL_API_URL=http://localhost:8080
export OPEN_WEBUI_URL=http://localhost:3000
export OPEN_WEBUI_API_KEY=your-api-key-here
export OLLAMA_MODEL=dolphin-mistral
export SIGNAL_PHONE_NUMBER=+1234567890
export SCHEDULE=weekly

cargo run --release
```

Or with Docker:

```bash
# Edit docker-compose.yml with your values, then:
docker compose up -d signal-bot
```

## Configuration

All configuration is via environment variables:

| Variable | Default | Description |
|---|---|---|
| `SIGNAL_PHONE_NUMBER` | *required* | Bot's registered Signal phone number |
| `OPEN_WEBUI_API_KEY` | *required* | API key from Open WebUI |
| `SIGNAL_API_URL` | `http://signal-api:8080` | signal-cli-rest-api URL |
| `OPEN_WEBUI_URL` | `http://open-webui:8080` | Open WebUI URL |
| `OLLAMA_MODEL` | `gpt-oss:20b` | Model for chat/summarization |
| `SCHEDULE` | `weekly` | `daily`, `weekly`, or `monthly` |
| `SUMMARY_PROMPT` | *(built-in)* | Custom system prompt for the summarizer |

## Commands

Tag the bot in any group chat:

| Command | Description |
|---|---|
| `@bot summarize` | Summarize all messages since the last summary |
| `@bot search <query>` | Search the web and return results |
| `@bot imagine <prompt>` | Generate an image from a text prompt |
| `@bot help` | List available commands |

Scheduled summaries run automatically (Monday 09:00 UTC for weekly).

## Development

```bash
cargo test              # unit + integration tests
cargo clippy            # lint
cargo build --release
```

Integration tests require a running Ollama instance on localhost:11434.

## License

MIT
