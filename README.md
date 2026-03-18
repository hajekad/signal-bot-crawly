# Signal Bot Crawly

A zero-dependency Rust bot that summarizes Signal group chats using a local LLM via Ollama.

## Features

- **Zero dependencies** — pure Rust with only `std`, no external crates
- **On-demand summaries** — @mention the bot in any group to trigger an instant summary
- **Scheduled summaries** — automatic daily, weekly, or monthly summaries
- **Local LLM** — uses Ollama for privacy-preserving summarization, no data leaves your machine
- **Auto-discovers groups** — monitors all groups the bot account is a member of
- **Identifies speakers** — summaries attribute messages to sender display names

## Architecture

Three components communicate over plain HTTP on a Docker bridge network:

```
signal-cli-rest-api (Signal gateway, port 8080)
        |
   signal-bot-crawly (Rust binary, polls every 30s)
        |
     Ollama (LLM inference, port 11434)
```

The bot uses `std::net::TcpStream` for HTTP/1.1 — no TLS needed since everything stays on localhost.

## Setup

### 1. Start the services

```bash
docker compose up -d signal-api ollama
```

### 2. Link your Signal account

Open in your browser:
```
http://localhost:8080/v1/qrcodelink?device_name=signal-bot
```

On the phone with the bot's Signal account: **Settings → Linked Devices → "+"** → scan the QR code.

### 3. Pull an Ollama model

```bash
docker compose exec ollama ollama pull llama3.2
# or any model you prefer: mistral, qwen2.5, dolphin-mistral, etc.
```

### 4. Configure and start the bot

Edit `docker-compose.yml` and set your phone number and preferred model:

```yaml
- SIGNAL_PHONE_NUMBER=+1234567890  # Your bot's Signal phone number
- OLLAMA_MODEL=llama3.2            # Must match the model you pulled
```

Then:

```bash
docker compose up -d signal-bot
```

### Running without Docker

```bash
export SIGNAL_API_URL=http://localhost:8080
export OLLAMA_API_URL=http://localhost:11434
export OLLAMA_MODEL=llama3.2
export SIGNAL_PHONE_NUMBER=+1234567890
export SCHEDULE=weekly

cargo run --release
```

## Configuration

All configuration is via environment variables:

| Variable | Default | Description |
|---|---|---|
| `SIGNAL_PHONE_NUMBER` | *required* | Bot's registered Signal phone number |
| `SIGNAL_API_URL` | `http://signal-api:8080` | signal-cli-rest-api URL |
| `OLLAMA_API_URL` | `http://ollama:11434` | Ollama API URL |
| `OLLAMA_MODEL` | `gpt-oss:20b` | Ollama model for summarization |
| `SCHEDULE` | `weekly` | `daily`, `weekly`, or `monthly` |
| `SUMMARY_PROMPT` | *(built-in)* | Custom system prompt for the summarizer |

## Usage

- **On-demand**: @mention the bot in any group chat. It will summarize all messages accumulated since the last summary.
- **Scheduled**: The bot automatically summarizes all groups at the scheduled time (Monday 09:00 UTC for weekly).
- The bot replies "No messages to summarize yet" if tagged before any messages have accumulated.

## Development

```bash
cargo test          # 83 unit + integration tests
cargo clippy        # lint
cargo build --release
```

Integration tests require a running Ollama instance on localhost:11434.

## License

MIT
