# x-monitor Agent Notes

## Project Goal
Build and maintain a Rust TUI for live monitoring X filtered-stream data with optional AI provider analysis per monitor.

## Core Commands
- `cargo fmt`
- `cargo check`
- `cargo run`

## Configuration
- Main config file: `x-monitor.toml`
- Monitor state file: `x-monitor-state.json`
- Required X token key: `x_bearer_token` (or env `X_BEARER_TOKEN` / `x_bearer_token`)
- AI provider config keys:
  - `default_ai_provider`
  - `[[ai_providers]]`

## Terminology
- Use the term `AI provider` everywhere.
- Keep naming consistent with `provider` fields and labels.

## UX Expectations
- Keep the TUI clean and keyboard-driven.
- Home view must show monitored targets and a live feed.
- Feed entries should include clickable post URLs via the `o` action.
- AI analysis should be asynchronous and appear as separate feed events.

## Code Organization
- `src/main.rs`: app loop, input, async orchestration.
- `src/ui.rs`: Ratatui rendering.
- `src/x_api.rs`: X rules + stream API integration.
- `src/ai.rs`: OpenAI-compatible chat completion calls.
- `src/config.rs`: config/env loading and provider resolution.
- `src/models.rs`: shared data models and feed formatting.
