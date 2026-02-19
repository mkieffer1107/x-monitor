# python_test

Official X SDK (`xdk`) test harness using `uv` so you can compare stream behavior against the Rust client.

## Setup

From this folder:

```bash
uv sync
```

Token loading:
- reads `X_BEARER_TOKEN` (or `x_bearer_token`) from environment
- also loads root project `.env` automatically if present

## Commands

Show help:

```bash
uv run python main.py --help
```

Rich TUI (similar layout to Rust app):

```bash
uv run tui
```

Rich TUI with session logging:

```bash
uv run tui --log-session
# or: uv run tui --log-file logs/tui-debug.log
```

Rich TUI AI behavior:
- uses `x-monitor.toml` (same provider config/default models as Rust)
- supports per-target AI analysis with custom prompt
- providers use OpenAI-compatible endpoints through the `openai` Python package
- built-in defaults:
  - `grok` -> `grok-4-1-fast-non-reasoning`
  - `openrouter` -> `x-ai/grok-4.1-fast`
  - `gemini` -> `gemini-3-flash-preview`
  - `openai` -> `gpt-5-nano`
  - `custom` (manual endpoint/model/key)

Rich TUI keys:
- `a`: add target (account or phrase)
- `d`: delete selected target
- `r`: reconnect selected target rule
- `x`: terminate all filtered-stream connections
- `Tab`: switch focus between monitor/feed panes
- `Up/Down`: navigate selected pane
- `o`: open selected feed URL
- `q`: quit

List current filtered-stream rules:

```bash
uv run python main.py rules list
```

Add an account rule:

```bash
uv run python main.py rules add --account @mkieffer1107
```

Add a phrase rule:

```bash
uv run python main.py rules add --phrase "rust tui"
```

Delete rules by id:

```bash
uv run python main.py rules delete --ids 1234567890
```

Clear all rules:

```bash
uv run python main.py rules clear
```

Run stream with full raw event output and session log:

```bash
uv run python main.py --log-session stream --raw
```

Run stream and stop after first 5 posts:

```bash
uv run python main.py --log-session stream --max-posts 5
```

## Logging

- `--log-session`: writes to `python_test/logs/session-YYYYMMDD-HHMMSS.log`
- `--log-file <path>`: writes to a custom path

Logs include command line, streamed events, and full stack traces for errors.
