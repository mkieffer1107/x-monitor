# x-monitor

monitor the situation on 𝕏.

A clean Ratatui terminal app for monitoring X filtered-stream results in real time.

## Features

- Home view with:
  - monitored targets (accounts or phrases)
  - live feed of incoming posts
  - links to original X posts
- Add monitors from inside the TUI (`a`)
  - account monitors (`from:<handle>`)
  - phrase monitors
  - load monitor settings from YAML files (`y` in Add Monitor)
  - optional AI analysis per monitor with:
    - AI provider
    - model ID
    - OpenAI-compatible endpoint override (for `custom` provider or manual override)
    - API key override (required for `custom` provider)
    - custom prompt
- AI analysis runs asynchronously:
  - post appears in feed immediately
  - analysis appears as a second feed event once response returns
- OpenAI-compatible AI endpoint support with default provider presets:
  - `grok`
  - `openrouter`
  - `gemini`
  - `openai`
  - `custom`

## Setup

1. Create config file:

```bash
cp x-monitor.example.toml x-monitor.toml
```

2. Add YAML target files in `monitor-configs/` (or set `monitor_config_dir` in `x-monitor.toml`):

```yaml
label: "My account watch"
kind: account
target: "@handle_1, handle2, @handle_3"
ai:
  enabled: true
  provider: grok
  model: grok-4-1-fast-non-reasoning
  prompt: "Summarize why this post matters and what to watch next."
```

3. Set your X bearer token as an environment variable:

```bash
export X_BEARER_TOKEN=...
# or: export x_bearer_token=...
```

4. Provide at least one AI API key env var if you want AI analysis with default providers (or use per-monitor API key override for `custom`):

```bash
export OPENAI_API_KEY=...
# or OPENROUTER_API_KEY / XAI_API_KEY / GEMINI_API_KEY
```

5. Build and run:

```bash
cargo run
```

Optional: enable persistent session logs (full feed lines, errors, URLs):

```bash
cargo run -- --log-session
# writes to ./logs/session-YYYYMMDD-HHMMSS.log
```

Or write to a custom file path:

```bash
cargo run -- --log-file logs/my-session.log
```

## Keybindings

- `a`: add monitor
- `e`: edit selected monitor (temporarily disconnects it; reconnects on exit)
- `d`: delete selected monitor
- `s`: toggle selected target active/inactive
- `r`: reconnect selected target (refresh its X stream rule)
- `x`: terminate all filtered-stream connections for the app
- `Tab`: switch focus between monitors and feed
- `Up/Down`: navigate selected pane
- `o`: open selected feed URL in browser
- `c`: clear live feed
- `q`: quit

Inside add-monitor modal:

- `Up/Down` (or `Tab`): move field
- `Left/Right`: toggle/cycle type/provider/options
- `Enter`: advance field / submit on "Create monitor"
- `y`: open YAML config picker (file list + preview; Enter connects selected file)
- `q`: cancel

## Notes

- The app persists monitor definitions in `x-monitor-state.json` by default.
- The stream reconnects automatically on disconnect.
- Deactivated targets stay saved but do not stream until re-activated.
- If a provider has no configured key, you can set an API key override in Add Monitor.
- AI API key input is prefilled with provider env-var names for built-in providers (custom starts blank).
- Account targets support comma-separated handles and `@` is optional.
  - examples: `handle_1, handle2, handle_3`
  - examples: `@handle_1, @handle2, @handle_3`
  - examples: `handle_1, handle2, @handle_3`

## Python SDK Test Harness

There is also a uv-based test harness in `/Users/mkieffer/programming/x-monitor/python_test` that uses the official X Python SDK (`xdk`) for comparison.

Quick start:

```bash
cd python_test
uv sync
uv run python main.py rules list
uv run python main.py --log-session stream --raw
uv run tui --log-session
```

The Python Rich TUI reads AI providers/default model IDs from `x-monitor.toml` and runs AI analysis via OpenAI-compatible endpoints.
