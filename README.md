# 𝕏 Monitor

Terminal app for monitoring 𝕏 filtered-stream posts in real time with AI analysis.

## Quickstart

First, you'll need to get your 𝕏 API bearer token:

1. Create an 𝕏 developer account and open the [console](https://console.x.com/).
2. Navigate to `Apps` in the left sidebar and create a new app.
3. Copy your Bearer Token to be used as the `X_BEARER_TOKEN` environment variable.

Then create a local environment file:
```bash
cp .example.env .env
```
Fill in the `X_BEARER_TOKEN` environment variable and any optional LLM API keys. Then start the app:

```bash
cargo run
```

## Monitor Config Files

For the YAML file picker, keep monitor target files in `monitor-configs/`.

An example file is already included at [`monitor-configs/example-account.yaml`](monitor-configs/example-account.yaml):

```yaml
label: "Example"
kind: account
target: "@mkieffer1107, @sentdefender, @WarMonitors, @DeptofWar"
ai:
  enabled: true
  provider: openrouter
  model: x-ai/grok-4.1-fast
  prompt: "Are we going to war with Iran? Respond with YES or NO."
```
