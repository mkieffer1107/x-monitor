from __future__ import annotations

import argparse
import json
import os
import queue
import select
import sys
import termios
import threading
import time
import traceback
import tty
import urllib.error
import urllib.request
import webbrowser
from collections import deque
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from uuid import uuid4

from dotenv import load_dotenv
from openai import OpenAI
from rich.console import Group
from rich.layout import Layout
from rich.live import Live
from rich.panel import Panel
from rich.text import Text
from xdk import Client
from xdk.stream.models import UpdateRulesRequest
from xdk.streaming import StreamConfig

MAX_FEED_ITEMS = 500
DEFAULT_ANALYSIS_PROMPT = "Summarize why this post matters and what to watch next."


def now_ts() -> str:
    return datetime.now().strftime("%H:%M:%S")


def to_dict(value: Any) -> dict[str, Any]:
    if value is None:
        return {}
    if isinstance(value, dict):
        return value
    if hasattr(value, "model_dump"):
        return value.model_dump()
    if hasattr(value, "dict"):
        return value.dict()
    return {"value": str(value)}


def quote_phrase(phrase: str) -> str:
    trimmed = phrase.strip()
    if not trimmed:
        raise ValueError("phrase cannot be empty")
    if trimmed.startswith('"') and trimmed.endswith('"') and len(trimmed) >= 2:
        return trimmed
    if " " in trimmed:
        escaped = trimmed.replace('"', '\\"')
        return f'"{escaped}"'
    return trimmed


def normalize_handle(handle: str) -> str:
    normalized = handle.strip().lstrip("@")
    if not normalized:
        raise ValueError("account handle cannot be empty")
    return normalized


def build_query(kind: str, target: str) -> str:
    trimmed = target.strip()
    if not trimmed:
        raise ValueError("target cannot be empty")
    if kind == "account":
        return f"from:{normalize_handle(trimmed)}"
    return quote_phrase(trimmed)


def load_env() -> None:
    load_dotenv()
    root_env = Path(__file__).resolve().parents[2] / ".env"
    if root_env.exists():
        load_dotenv(root_env)


def resolve_bearer_token(cli_value: str | None) -> str:
    if cli_value:
        return cli_value
    token = os.getenv("X_BEARER_TOKEN") or os.getenv("x_bearer_token")
    if not token:
        raise ValueError("missing X bearer token. Set X_BEARER_TOKEN or pass --bearer-token")
    return token


def is_env_var_name(name: str) -> bool:
    return bool(name) and all(
        ch == "_" or ch.isascii() and (ch.isupper() or ch.isdigit()) for ch in name
    )


def resolve_api_key_input(input_value: str) -> str | None:
    trimmed = input_value.strip()
    if not trimmed:
        return None

    if trimmed.startswith("$"):
        env_name = trimmed[1:]
        if is_env_var_name(env_name):
            value = os.getenv(env_name, "").strip()
            return value or None
        return trimmed

    if is_env_var_name(trimmed):
        value = os.getenv(trimmed, "").strip()
        return value or None

    return trimmed


def mask_secret(input_value: str) -> str:
    return "*" * len(input_value) if input_value else ""


def api_key_input_display(input_value: str) -> str:
    trimmed = input_value.strip()
    if not trimmed:
        return ""
    if trimmed.startswith("$") and is_env_var_name(trimmed[1:]):
        return trimmed
    if is_env_var_name(trimmed):
        return trimmed
    return mask_secret(trimmed)


@dataclass
class AiProvider:
    name: str
    base_url: str
    model: str
    api_key: str | None = None
    api_key_env: str | None = None

    def resolved_api_key(self) -> str | None:
        if self.api_key and self.api_key.strip():
            return self.api_key.strip()
        if self.api_key_env:
            value = os.getenv(self.api_key_env, "").strip()
            if value:
                return value
        return None


def default_ai_providers() -> list[AiProvider]:
    return [
        AiProvider(
            name="grok",
            base_url="https://api.x.ai/v1",
            model="grok-4-1-fast-non-reasoning",
            api_key_env="XAI_API_KEY",
        ),
        AiProvider(
            name="openrouter",
            base_url="https://openrouter.ai/api/v1",
            model="x-ai/grok-4.1-fast",
            api_key_env="OPENROUTER_API_KEY",
        ),
        AiProvider(
            name="gemini",
            base_url="https://generativelanguage.googleapis.com/v1beta/openai",
            model="gemini-3-flash-preview",
            api_key_env="GEMINI_API_KEY",
        ),
        AiProvider(
            name="openai",
            base_url="https://api.openai.com/v1",
            model="gpt-5-nano",
            api_key_env="OPENAI_API_KEY",
        ),
        AiProvider(name="custom", base_url="", model="", api_key=None, api_key_env=None),
    ]


def default_ai_provider_name() -> str:
    return "grok"


def _parse_ai_provider(raw: dict[str, Any]) -> AiProvider | None:
    name = str(raw.get("name", "")).strip()
    if not name:
        return None
    base_url = str(raw.get("base_url", "")).strip()
    model = str(raw.get("model", "")).strip()
    api_key_value = raw.get("api_key")
    api_key_env_value = raw.get("api_key_env")
    api_key = str(api_key_value).strip() if api_key_value is not None else None
    api_key_env = (
        str(api_key_env_value).strip() if api_key_env_value is not None else None
    )
    return AiProvider(
        name=name,
        base_url=base_url,
        model=model,
        api_key=api_key or None,
        api_key_env=api_key_env or None,
    )


def merge_default_providers(existing: list[AiProvider]) -> list[AiProvider]:
    remaining = existing[:]
    merged: list[AiProvider] = []
    for default in default_ai_providers():
        position = next(
            (
                idx
                for idx, provider in enumerate(remaining)
                if provider.name.lower() == default.name.lower()
            ),
            None,
        )
        if position is None:
            merged.append(default)
        else:
            merged.append(remaining.pop(position))
    merged.extend(remaining)
    return merged


@dataclass
class AppConfig:
    default_ai_provider: str
    ai_providers: list[AiProvider]

    @classmethod
    def load(cls, config_override: str | None = None) -> tuple["AppConfig", Path]:
        root_dir = Path(__file__).resolve().parents[2]
        env_path = os.getenv("X_MONITOR_CONFIG")
        path = (
            Path(config_override).expanduser()
            if config_override
            else Path(env_path).expanduser()
            if env_path
            else root_dir / "x-monitor.toml"
        )
        if not path.is_absolute():
            path = (root_dir / path).resolve()
        path = path.resolve()

        parsed: dict[str, Any] = {}
        if path.exists():
            import tomllib

            with path.open("rb") as fh:
                parsed = tomllib.load(fh)

        configured_default = str(
            parsed.get("default_ai_provider", default_ai_provider_name())
        ).strip() or default_ai_provider_name()
        raw_providers = parsed.get("ai_providers")
        configured_providers: list[AiProvider] = []
        if isinstance(raw_providers, list):
            for item in raw_providers:
                if isinstance(item, dict):
                    provider = _parse_ai_provider(item)
                    if provider:
                        configured_providers.append(provider)

        providers = (
            default_ai_providers()
            if not configured_providers
            else merge_default_providers(configured_providers)
        )

        env_default = (
            os.getenv("X_MONITOR_DEFAULT_AI_PROVIDER")
            or os.getenv("x_monitor_default_ai_provider")
            or ""
        ).strip()
        if env_default:
            configured_default = env_default

        if not any(
            provider.name.lower() == configured_default.lower() for provider in providers
        ):
            configured_default = providers[0].name if providers else default_ai_provider_name()

        return cls(default_ai_provider=configured_default, ai_providers=providers), path

    def provider_names(self) -> list[str]:
        return [provider.name for provider in self.ai_providers]

    def provider_by_name(self, name: str) -> AiProvider | None:
        lowered = name.lower()
        for provider in self.ai_providers:
            if provider.name.lower() == lowered:
                return provider
        return None


@dataclass
class SessionLogger:
    path: Path
    _lock: threading.Lock = field(default_factory=threading.Lock)

    def __post_init__(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)

    def log(self, message: str) -> None:
        stamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        line = f"[{stamp}] {message}\n"
        with self._lock:
            with self.path.open("a", encoding="utf-8") as fh:
                fh.write(line)


@dataclass
class FeedItem:
    kind: str
    message: str
    url: str | None = None
    at: str = field(default_factory=now_ts)

    def summary(self) -> str:
        return f"[{self.at}] {self.kind} {self.message}"


@dataclass
class MonitorAnalysis:
    enabled: bool = False
    provider: str = "grok"
    model: str = ""
    endpoint: str = ""
    api_key: str = ""
    prompt: str = DEFAULT_ANALYSIS_PROMPT

    @classmethod
    def from_dict(cls, raw: Any) -> "MonitorAnalysis":
        if not isinstance(raw, dict):
            return cls()
        return cls(
            enabled=bool(raw.get("enabled", False)),
            provider=str(raw.get("provider", "grok")),
            model=str(raw.get("model", "")),
            endpoint=str(raw.get("endpoint", "")),
            api_key=str(raw.get("api_key", "")),
            prompt=str(raw.get("prompt", DEFAULT_ANALYSIS_PROMPT)),
        )


@dataclass
class Monitor:
    id: str
    label: str
    kind: str
    target: str
    query: str
    rule_id: str
    rule_tag: str
    analysis: MonitorAnalysis = field(default_factory=MonitorAnalysis)
    active: bool = False
    created_at: str = field(default_factory=lambda: datetime.now(timezone.utc).isoformat())

    @classmethod
    def from_dict(cls, raw: dict[str, Any]) -> "Monitor":
        monitor_id = str(raw.get("id") or uuid4())
        kind = str(raw.get("kind", "phrase")).lower()
        if kind in ("account", "acct"):
            kind = "account"
        else:
            kind = "phrase"
        target = str(raw.get("target") or raw.get("input_value") or "")
        if kind == "account" and target and not target.startswith("@"):
            target = f"@{target.lstrip('@')}"

        query = str(raw.get("query", ""))
        if not query and target:
            try:
                query = build_query(kind, target)
            except Exception:  # noqa: BLE001
                query = ""

        rule_tag = str(raw.get("rule_tag", "")).strip()
        if not rule_tag:
            rule_tag = f"xmon:{monitor_id.replace('-', '')[:24]}"

        return cls(
            id=monitor_id,
            label=str(raw.get("label", "")).strip() or target or monitor_id,
            kind=kind,
            target=target,
            query=query,
            rule_id=str(raw.get("rule_id", "")),
            rule_tag=rule_tag,
            analysis=MonitorAnalysis.from_dict(raw.get("analysis")),
            active=bool(raw.get("active", False)),
            created_at=str(raw.get("created_at", datetime.now(timezone.utc).isoformat())),
        )


@dataclass
class AddForm:
    field_index: int = 0
    kind: str = "account"
    target: str = "@"
    display_name: str = ""
    ai_enabled: bool = False
    ai_provider_index: int = 0
    ai_model: str = ""
    ai_endpoint: str = ""
    ai_api_key: str = ""
    ai_prompt: str = DEFAULT_ANALYSIS_PROMPT

    @classmethod
    def new(cls, config: AppConfig, provider_names: list[str]) -> "AddForm":
        default_provider = config.default_ai_provider
        index = next(
            (
                idx
                for idx, provider_name in enumerate(provider_names)
                if provider_name.lower() == default_provider.lower()
            ),
            0,
        )
        form = cls(ai_provider_index=index)
        form.apply_provider_defaults(config, provider_names)
        return form

    def selected_provider(self, provider_names: list[str]) -> str:
        if not provider_names:
            return default_ai_provider_name()
        if self.ai_provider_index < 0 or self.ai_provider_index >= len(provider_names):
            return provider_names[0]
        return provider_names[self.ai_provider_index]

    def cycle_provider(self, provider_names: list[str], delta: int) -> None:
        if not provider_names:
            self.ai_provider_index = 0
            return
        self.ai_provider_index = (self.ai_provider_index + delta) % len(provider_names)

    def apply_provider_defaults(self, config: AppConfig, provider_names: list[str]) -> None:
        provider_name = self.selected_provider(provider_names)
        provider = config.provider_by_name(provider_name)
        if provider is None:
            self.ai_model = ""
            self.ai_endpoint = ""
            self.ai_api_key = ""
            return
        self.ai_model = provider.model
        self.ai_endpoint = provider.base_url
        if provider.name.lower() == "custom":
            self.ai_api_key = ""
        else:
            self.ai_api_key = provider.api_key_env or provider.api_key or ""

    def cycle_kind(self) -> None:
        self.kind = "phrase" if self.kind == "account" else "account"
        if self.kind == "account":
            if not self.target:
                self.target = "@"
            elif not self.target.startswith("@"):
                self.target = f"@{self.target}"
        elif self.target == "@":
            self.target = ""

    def move_field(self, delta: int) -> None:
        self.field_index = (self.field_index + delta) % 10


class KeyReader:
    def __init__(self) -> None:
        self.fd = sys.stdin.fileno()
        self._old_settings: list[Any] | None = None

    def __enter__(self) -> "KeyReader":
        self._old_settings = termios.tcgetattr(self.fd)
        tty.setcbreak(self.fd)
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        if self._old_settings is not None:
            termios.tcsetattr(self.fd, termios.TCSADRAIN, self._old_settings)

    def read_key(self) -> str | None:
        readable, _, _ = select.select([self.fd], [], [], 0.0)
        if not readable:
            return None
        first = os.read(self.fd, 1).decode("utf-8", errors="ignore")
        if not first:
            return None
        if first == "\x03":
            return "ctrl_c"
        if first in ("\n", "\r"):
            return "enter"
        if first == "\t":
            return "tab"
        if first in ("\x7f", "\x08"):
            return "backspace"
        if first != "\x1b":
            return first

        sequence = first
        time.sleep(0.001)
        while True:
            readable, _, _ = select.select([self.fd], [], [], 0.0)
            if not readable:
                break
            sequence += os.read(self.fd, 1).decode("utf-8", errors="ignore")
        mapping = {
            "\x1b[A": "up",
            "\x1b[B": "down",
            "\x1b[C": "right",
            "\x1b[D": "left",
            "\x1b[Z": "shift_tab",
            "\x1b": "esc",
        }
        return mapping.get(sequence, "esc")


def extract_chat_text(response: Any) -> str:
    choices = getattr(response, "choices", None)
    if isinstance(choices, list):
        parts: list[str] = []
        for choice in choices:
            message = getattr(choice, "message", None)
            content = getattr(message, "content", None) if message else None
            if isinstance(content, str) and content.strip():
                parts.append(content.strip())
                continue
            if isinstance(content, list):
                chunks: list[str] = []
                for chunk in content:
                    if isinstance(chunk, dict):
                        value = chunk.get("text")
                        if isinstance(value, str) and value.strip():
                            chunks.append(value.strip())
                    else:
                        value = getattr(chunk, "text", None)
                        if isinstance(value, str) and value.strip():
                            chunks.append(value.strip())
                if chunks:
                    parts.append("\n".join(chunks))
        if parts:
            return "\n\n".join(parts)

    output_text = getattr(response, "output_text", None)
    if isinstance(output_text, str) and output_text.strip():
        return output_text.strip()
    return json.dumps(to_dict(response), ensure_ascii=False)


class XMonitorRichTui:
    def __init__(
        self,
        client: Client,
        bearer_token: str,
        state_path: Path,
        config: AppConfig,
        logger: SessionLogger | None = None,
    ) -> None:
        self.client = client
        self.bearer_token = bearer_token
        self.state_path = state_path
        self.config = config
        self.provider_names = config.provider_names()
        self.logger = logger

        self.feed: deque[FeedItem] = deque(maxlen=MAX_FEED_ITEMS)
        self.monitors: list[Monitor] = self._load_state()
        self.focus = "monitors"
        self.selected_monitor = 0
        self.selected_feed = 0
        self.stream_connected = False
        self.status = "Ready"
        self.running = True
        self.events: queue.Queue[dict[str, Any]] = queue.Queue()
        self.stream_stop = threading.Event()
        self.stream_thread: threading.Thread | None = None
        self.add_form: AddForm | None = None

        self._set_all_active(False)
        self._push_info("press 'a' to add monitor target, 'q' to quit")

    def run(self) -> int:
        self._ensure_stream_thread()
        with KeyReader() as key_reader:
            with Live(self._render(), screen=True, refresh_per_second=10) as live:
                while self.running:
                    self._drain_events()
                    self._ensure_stream_thread()
                    key = self._read_key_safe(key_reader)
                    if key:
                        self._handle_key(key)
                    live.update(self._render())
                    time.sleep(0.06)
        self.stream_stop.set()
        self._save_state()
        return 0

    def _read_key_safe(self, key_reader: KeyReader) -> str | None:
        try:
            return key_reader.read_key()
        except Exception as exc:  # noqa: BLE001
            self._push_error(f"key read failure: {exc}")
            return None

    def _push_feed(self, item: FeedItem) -> None:
        self.feed.appendleft(item)
        if self.selected_feed >= len(self.feed):
            self.selected_feed = max(len(self.feed) - 1, 0)
        if self.logger:
            line = item.summary()
            if item.url:
                line += f" | URL: {item.url}"
            self.logger.log(line)

    def _push_info(self, message: str) -> None:
        self.status = message
        self._push_feed(FeedItem(kind="INFO", message=message))

    def _push_error(self, message: str) -> None:
        self.status = message
        self._push_feed(FeedItem(kind="ERROR", message=message))

    def _push_post(self, author: str, text: str, labels: list[str], url: str | None) -> None:
        label = ", ".join(labels) if labels else "unknown"
        summary = text.replace("\n", " ").strip()
        self._push_feed(
            FeedItem(
                kind="POST",
                message=f"@{author} | monitor: {label} | {summary}",
                url=url,
            )
        )

    def _push_analysis(
        self, monitor_label: str, provider: str, model: str, output: str, url: str | None
    ) -> None:
        text = output.replace("\n", " ").strip()
        self._push_feed(
            FeedItem(
                kind="AI",
                message=f"({provider}:{model}) [{monitor_label}] {text}",
                url=url,
            )
        )

    def _set_all_active(self, active: bool) -> None:
        for monitor in self.monitors:
            monitor.active = active

    def _save_state(self) -> None:
        body = {"monitors": [asdict(monitor) for monitor in self.monitors]}
        self.state_path.parent.mkdir(parents=True, exist_ok=True)
        self.state_path.write_text(json.dumps(body, indent=2), encoding="utf-8")

    def _load_state(self) -> list[Monitor]:
        if not self.state_path.exists():
            return []
        try:
            raw = json.loads(self.state_path.read_text(encoding="utf-8"))
            items = raw.get("monitors", [])
            monitors: list[Monitor] = []
            if isinstance(items, list):
                for item in items:
                    if isinstance(item, dict):
                        monitors.append(Monitor.from_dict(item))
            return monitors
        except Exception:  # noqa: BLE001
            return []

    def _ensure_stream_thread(self) -> None:
        if not self.monitors:
            if self.stream_thread and self.stream_thread.is_alive():
                self.stream_stop.set()
                self.stream_thread = None
            if self.stream_connected:
                self.stream_connected = False
                self._set_all_active(False)
                self._push_info("stream paused (add a monitor to reconnect)")
            return

        if self.stream_thread and self.stream_thread.is_alive():
            return

        self.stream_stop = threading.Event()
        self.stream_thread = threading.Thread(target=self._stream_loop, daemon=True)
        self.stream_thread.start()
        self._push_info("stream thread started")

    def _stream_loop(self) -> None:
        retry_seconds = 2
        no_rules_sent = False
        too_many_sent = False
        provisioning_sent = False

        while not self.stream_stop.is_set():
            self.events.put({"type": "info", "message": "connecting to filtered stream"})

            def on_connect() -> None:
                nonlocal retry_seconds, no_rules_sent, too_many_sent, provisioning_sent
                retry_seconds = 2
                no_rules_sent = False
                too_many_sent = False
                provisioning_sent = False
                self.events.put({"type": "stream_connected", "connected": True})
                self.events.put({"type": "info", "message": "stream connected"})

            stream_config = StreamConfig(
                max_retries=0,
                initial_backoff=2.0,
                max_backoff=60.0,
                backoff_multiplier=2.0,
                jitter=False,
                on_connect=on_connect,
            )
            try:
                for event in self.client.stream.posts(stream_config=stream_config):
                    if self.stream_stop.is_set():
                        return
                    payload = to_dict(event)
                    self.events.put({"type": "stream_event", "payload": payload})
                raise RuntimeError("stream ended by remote host")
            except Exception as exc:  # noqa: BLE001
                text = str(exc)
                lowered = text.lower()
                self.events.put({"type": "stream_connected", "connected": False})

                if "ruleconfigurationissue" in lowered or "must define rules" in lowered:
                    if not no_rules_sent:
                        self.events.put(
                            {
                                "type": "info",
                                "message": "No stream rules are configured yet. Press 'a' to add a target.",
                            }
                        )
                        no_rules_sent = True
                    too_many_sent = False
                    provisioning_sent = False
                    if self.stream_stop.wait(5):
                        return
                    continue

                if "provisioning" in lowered:
                    if not provisioning_sent:
                        self.events.put(
                            {
                                "type": "info",
                                "message": "X is provisioning your subscription. Retrying in 60s.",
                            }
                        )
                        provisioning_sent = True
                    no_rules_sent = False
                    too_many_sent = False
                    if self.stream_stop.wait(60):
                        return
                    continue

                if "toomanyconnections" in lowered or "maximum allowed connection" in lowered:
                    if not too_many_sent:
                        self.events.put(
                            {
                                "type": "info",
                                "message": "X reports max active stream connections. Press 'x' to terminate all, then wait for reconnect.",
                            }
                        )
                        too_many_sent = True
                    no_rules_sent = False
                    provisioning_sent = False
                    if self.stream_stop.wait(60):
                        return
                    continue

                no_rules_sent = False
                too_many_sent = False
                provisioning_sent = False
                self.events.put({"type": "error", "message": f"stream disconnected: {text}"})
                self.events.put(
                    {
                        "type": "info",
                        "message": f"retrying stream connection in {retry_seconds}s",
                    }
                )
                if self.stream_stop.wait(retry_seconds):
                    return
                retry_seconds = min(retry_seconds * 2, 60)

    def _drain_events(self) -> None:
        while True:
            try:
                event = self.events.get_nowait()
            except queue.Empty:
                break
            self._handle_event(event)

    def _handle_event(self, event: dict[str, Any]) -> None:
        event_type = event.get("type")
        if event_type == "info":
            self._push_info(str(event.get("message", "")))
            return
        if event_type == "error":
            self._push_error(str(event.get("message", "")))
            return
        if event_type == "stream_connected":
            connected = bool(event.get("connected"))
            self.stream_connected = connected
            self._set_all_active(connected)
            return
        if event_type == "stream_event":
            payload = event.get("payload", {})
            if isinstance(payload, dict):
                self._handle_stream_payload(payload)
            return
        if event_type == "monitor_added":
            monitor = event.get("monitor")
            if isinstance(monitor, Monitor):
                monitor.active = self.stream_connected
                self.monitors.append(monitor)
                self.selected_monitor = max(len(self.monitors) - 1, 0)
                self._save_state()
                self._push_info(f"monitor added: {monitor.label}")
            else:
                self._push_error("monitor add failed: invalid response")
            return
        if event_type == "monitor_deleted":
            monitor_id = str(event.get("id", ""))
            label = str(event.get("label", "target"))
            position = next(
                (
                    idx
                    for idx, monitor in enumerate(self.monitors)
                    if monitor.id == monitor_id
                ),
                None,
            )
            if position is not None:
                self.monitors.pop(position)
                if self.selected_monitor >= len(self.monitors):
                    self.selected_monitor = max(len(self.monitors) - 1, 0)
                self._save_state()
            self._push_info(f"monitor removed: {label}")
            return
        if event_type == "monitor_reconnected":
            monitor_id = str(event.get("id", ""))
            label = str(event.get("label", "target"))
            new_rule_id = str(event.get("new_rule_id", ""))
            for monitor in self.monitors:
                if monitor.id == monitor_id:
                    monitor.rule_id = new_rule_id
                    monitor.active = True
                    break
            self._save_state()
            self._push_info(f"target reconnected: {label}")
            return
        if event_type == "analysis_completed":
            output = event.get("output")
            if isinstance(output, str):
                self._push_analysis(
                    str(event.get("monitor_label", "")),
                    str(event.get("provider", "")),
                    str(event.get("model", "")),
                    output,
                    str(event.get("url")) if event.get("url") else None,
                )
            else:
                self._push_error(
                    f"analysis failed for '{event.get('monitor_label', '')}' via "
                    f"{event.get('provider', '')}:{event.get('model', '')}: {event.get('error', 'unknown')}"
                )
            return

    def _handle_stream_payload(self, payload: dict[str, Any]) -> None:
        errors = payload.get("errors")
        if errors:
            self._push_error(f"stream response errors: {json.dumps(errors)}")
            return

        data = payload.get("data") or {}
        if not isinstance(data, dict) or not data:
            return

        includes = payload.get("includes") or {}
        users = includes.get("users") or []
        usernames = {
            user.get("id"): user.get("username")
            for user in users
            if isinstance(user, dict) and user.get("id")
        }

        post_id = str(data.get("id", ""))
        if not post_id:
            return
        author_id = data.get("author_id")
        author = usernames.get(author_id) or author_id or "unknown"
        text = str(data.get("text", ""))

        matching_rules = payload.get("matching_rules") or []
        tags = [
            rule.get("tag")
            for rule in matching_rules
            if isinstance(rule, dict) and isinstance(rule.get("tag"), str)
        ]
        matched = [monitor for monitor in self.monitors if monitor.rule_tag in tags]
        labels = [monitor.label for monitor in matched]

        if isinstance(author, str) and author and author != "unknown":
            url = f"https://x.com/{author}/status/{post_id}"
        else:
            url = f"https://x.com/i/web/status/{post_id}"

        self._push_post(str(author), text, labels, url)

        for monitor in matched:
            if not monitor.analysis.enabled:
                continue
            self._start_analysis(monitor, text, url)

    def _start_analysis(self, monitor: Monitor, post_text: str, url: str) -> None:
        provider_name = monitor.analysis.provider
        provider_config = self.config.provider_by_name(provider_name)
        if provider_config is None:
            self._push_error(f"AI provider '{provider_name}' is not configured")
            return

        api_key_override = resolve_api_key_input(monitor.analysis.api_key)
        api_key = provider_config.resolved_api_key()
        if api_key_override:
            api_key = api_key_override
        if not api_key:
            self._push_error(
                f"AI provider '{provider_name}' missing or no API key available"
            )
            return

        model = monitor.analysis.model.strip() or provider_config.model
        if not model:
            self._push_error(
                f"analysis skipped for '{monitor.label}' because model ID is empty"
            )
            return

        endpoint = monitor.analysis.endpoint.strip() or provider_config.base_url
        if not endpoint:
            self._push_error(
                f"analysis skipped for '{monitor.label}' because endpoint is empty"
            )
            return

        prompt = monitor.analysis.prompt.strip() or DEFAULT_ANALYSIS_PROMPT
        threading.Thread(
            target=self._analyze_post_worker,
            args=(monitor.label, provider_config.name, model, endpoint, api_key, prompt, post_text, url),
            daemon=True,
        ).start()

    def _analyze_post_worker(
        self,
        monitor_label: str,
        provider_name: str,
        model: str,
        endpoint: str,
        api_key: str,
        prompt: str,
        post_text: str,
        url: str,
    ) -> None:
        try:
            ai_client = OpenAI(api_key=api_key, base_url=endpoint)
            response = ai_client.chat.completions.create(
                model=model,
                messages=[
                    {"role": "system", "content": prompt},
                    {"role": "user", "content": post_text},
                ],
            )
            output = extract_chat_text(response).strip() or "(empty response)"
            self.events.put(
                {
                    "type": "analysis_completed",
                    "monitor_label": monitor_label,
                    "provider": provider_name,
                    "model": model,
                    "output": output,
                    "url": url,
                }
            )
        except Exception as exc:  # noqa: BLE001
            self.events.put(
                {
                    "type": "analysis_completed",
                    "monitor_label": monitor_label,
                    "provider": provider_name,
                    "model": model,
                    "error": str(exc),
                }
            )

    def _handle_key(self, key: str) -> None:
        if key == "ctrl_c":
            self.running = False
            return

        if self.add_form is not None:
            self._handle_add_form_key(key)
            return

        if key == "q":
            self.running = False
        elif key == "tab":
            self.focus = "feed" if self.focus == "monitors" else "monitors"
        elif key == "up":
            self._move_selection(-1)
        elif key == "down":
            self._move_selection(1)
        elif key == "a":
            self.add_form = AddForm.new(self.config, self.provider_names)
        elif key == "d":
            self._delete_selected_monitor()
        elif key == "r":
            self._reconnect_selected_monitor()
        elif key == "x":
            self._terminate_all_connections()
        elif key == "o":
            self._open_selected_feed_url()

    def _handle_add_form_key(self, key: str) -> None:
        form = self.add_form
        if form is None:
            return
        editing_text = form.field_index in (1, 2, 5, 6, 7, 8)

        if key == "q" and not editing_text:
            self.add_form = None
            self._push_info("add target canceled")
            return
        if key == "esc":
            self.add_form = None
            self._push_info("add target canceled")
            return

        if key == "up":
            form.move_field(-1)
            return
        if key in ("down", "tab"):
            form.move_field(1)
            return
        if key == "shift_tab":
            form.move_field(-1)
            return

        if key in ("left", "right"):
            delta = -1 if key == "left" else 1
            if form.field_index == 0:
                form.cycle_kind()
                return
            if form.field_index == 3:
                form.ai_enabled = not form.ai_enabled
                return
            if form.field_index == 4:
                form.cycle_provider(self.provider_names, delta)
                form.apply_provider_defaults(self.config, self.provider_names)
                return

        if key == "backspace":
            if form.field_index == 1:
                if form.kind == "account":
                    if len(form.target) > 1:
                        form.target = form.target[:-1]
                else:
                    form.target = form.target[:-1]
            elif form.field_index == 2:
                form.display_name = form.display_name[:-1]
            elif form.field_index == 5:
                form.ai_model = form.ai_model[:-1]
            elif form.field_index == 6:
                form.ai_endpoint = form.ai_endpoint[:-1]
            elif form.field_index == 7:
                form.ai_api_key = form.ai_api_key[:-1]
            elif form.field_index == 8:
                form.ai_prompt = form.ai_prompt[:-1]
            return

        if key == "enter":
            if form.field_index == 9:
                self._submit_add_form()
            else:
                form.move_field(1)
            return

        if len(key) == 1 and key.isprintable():
            if form.field_index == 1:
                if form.kind == "account" and form.target == "@" and key == "@":
                    return
                form.target += key
            elif form.field_index == 2:
                form.display_name += key
            elif form.field_index == 5:
                form.ai_model += key
            elif form.field_index == 6:
                form.ai_endpoint += key
            elif form.field_index == 7:
                form.ai_api_key += key
            elif form.field_index == 8:
                form.ai_prompt += key

    def _submit_add_form(self) -> None:
        form = self.add_form
        if form is None:
            return
        try:
            query = build_query(form.kind, form.target)
            target = form.target.strip()
            if form.kind == "account":
                target = f"@{normalize_handle(target)}"
            label = form.display_name.strip() or target
            if not label:
                raise ValueError("display name cannot be empty")

            analysis = MonitorAnalysis(
                enabled=form.ai_enabled,
                provider=form.selected_provider(self.provider_names),
                model=form.ai_model.strip(),
                endpoint=form.ai_endpoint.strip(),
                api_key=form.ai_api_key.strip(),
                prompt=form.ai_prompt.strip() or DEFAULT_ANALYSIS_PROMPT,
            )

            if analysis.enabled:
                if not analysis.model:
                    raise ValueError("AI model ID cannot be empty when analysis is enabled")
                if analysis.provider.lower() == "custom" and not analysis.endpoint:
                    raise ValueError("custom AI provider requires an endpoint")
                if analysis.provider.lower() == "custom" and not analysis.api_key:
                    raise ValueError("custom AI provider requires an API key")

            monitor_id = str(uuid4())
            pending: dict[str, Any] = {
                "id": monitor_id,
                "kind": form.kind,
                "target": target,
                "label": label,
                "query": query,
                "rule_tag": f"xmon:{monitor_id.replace('-', '')[:24]}",
                "analysis": analysis,
            }
        except Exception as exc:  # noqa: BLE001
            self._push_error(f"invalid monitor settings: {exc}")
            return

        self.add_form = None
        self._push_info(f"adding monitor '{pending['label']}'...")
        threading.Thread(target=self._add_monitor_worker, args=(pending,), daemon=True).start()

    def _add_monitor_worker(self, pending: dict[str, Any]) -> None:
        try:
            body = UpdateRulesRequest(
                add=[{"value": pending["query"], "tag": pending["rule_tag"]}]
            )
            response = self.client.stream.update_rules(body=body)
            payload = to_dict(response)
            rule_id = self._extract_rule_id(payload)
            monitor = Monitor(
                id=str(pending["id"]),
                label=str(pending["label"]),
                kind=str(pending["kind"]),
                target=str(pending["target"]),
                query=str(pending["query"]),
                rule_id=rule_id,
                rule_tag=str(pending["rule_tag"]),
                analysis=pending["analysis"]
                if isinstance(pending["analysis"], MonitorAnalysis)
                else MonitorAnalysis(),
            )
            self.events.put({"type": "monitor_added", "monitor": monitor})
        except Exception as exc:  # noqa: BLE001
            self.events.put({"type": "error", "message": f"failed to add monitor: {exc}"})

    def _extract_rule_id(self, payload: dict[str, Any]) -> str:
        self._raise_on_api_errors(payload)
        data = payload.get("data")
        if isinstance(data, list) and data and isinstance(data[0], dict):
            rule_id = data[0].get("id")
            if rule_id:
                return str(rule_id)
        if isinstance(data, dict):
            rule_id = data.get("id")
            if rule_id:
                return str(rule_id)
        raise RuntimeError(f"add rule response missing rule id: {json.dumps(payload)}")

    def _raise_on_api_errors(self, payload: dict[str, Any]) -> None:
        errors = payload.get("errors")
        if errors:
            raise RuntimeError(json.dumps(errors))

    def _selected_monitor(self) -> Monitor | None:
        if not self.monitors:
            return None
        return self.monitors[self.selected_monitor]

    def _delete_selected_monitor(self) -> None:
        monitor = self._selected_monitor()
        if not monitor:
            self._push_info("no monitor selected")
            return
        self._push_info(f"removing monitor '{monitor.label}'...")
        threading.Thread(target=self._delete_monitor_worker, args=(monitor,), daemon=True).start()

    def _delete_monitor_worker(self, monitor: Monitor) -> None:
        try:
            body = UpdateRulesRequest(delete={"ids": [monitor.rule_id]})
            response = self.client.stream.update_rules(body=body)
            payload = to_dict(response)
            self._raise_on_api_errors(payload)
            self.events.put(
                {"type": "monitor_deleted", "id": monitor.id, "label": monitor.label}
            )
        except Exception as exc:  # noqa: BLE001
            self.events.put({"type": "error", "message": f"failed to delete monitor: {exc}"})

    def _reconnect_selected_monitor(self) -> None:
        monitor = self._selected_monitor()
        if not monitor:
            self._push_info("no monitor selected")
            return
        monitor.active = False
        self._push_info(f"reconnecting target '{monitor.label}'...")
        threading.Thread(
            target=self._reconnect_monitor_worker, args=(monitor,), daemon=True
        ).start()

    def _reconnect_monitor_worker(self, monitor: Monitor) -> None:
        try:
            delete_body = UpdateRulesRequest(delete={"ids": [monitor.rule_id]})
            try:
                delete_response = self.client.stream.update_rules(body=delete_body)
                delete_payload = to_dict(delete_response)
                self._raise_on_api_errors(delete_payload)
            except Exception as delete_exc:  # noqa: BLE001
                lowered = str(delete_exc).lower()
                if "404" not in lowered and "not found" not in lowered:
                    raise

            add_body = UpdateRulesRequest(
                add=[{"value": monitor.query, "tag": monitor.rule_tag}]
            )
            response = self.client.stream.update_rules(body=add_body)
            payload = to_dict(response)
            new_rule_id = self._extract_rule_id(payload)
            self.events.put(
                {
                    "type": "monitor_reconnected",
                    "id": monitor.id,
                    "label": monitor.label,
                    "new_rule_id": new_rule_id,
                }
            )
        except Exception as exc:  # noqa: BLE001
            self.events.put({"type": "error", "message": f"failed to reconnect target: {exc}"})

    def _terminate_all_connections(self) -> None:
        self.stream_connected = False
        self._set_all_active(False)
        self._push_info("terminating all filtered stream connections...")
        threading.Thread(target=self._terminate_connections_worker, daemon=True).start()

    def _terminate_connections_worker(self) -> None:
        try:
            request = urllib.request.Request(
                "https://api.x.com/2/connections/all",
                method="DELETE",
                headers={
                    "Authorization": f"Bearer {self.bearer_token}",
                    "Content-Type": "application/json",
                },
            )
            with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310
                body = response.read().decode("utf-8", errors="ignore")
                status = response.status
            if status < 200 or status >= 300:
                raise RuntimeError(f"terminate connections failed ({status}): {body}")

            summary = "terminated all active stream connections"
            if body.strip():
                payload = json.loads(body)
                data = payload.get("data") if isinstance(payload, dict) else {}
                if isinstance(data, dict):
                    successful = data.get("successful_kills")
                    failed = data.get("failed_kills")
                    if successful is not None or failed is not None:
                        summary = (
                            f"terminate-all complete (successful: {successful or 0}, "
                            f"failed: {failed or 0})"
                        )
            self.events.put({"type": "info", "message": summary})
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="ignore")
            self.events.put(
                {
                    "type": "error",
                    "message": f"failed to terminate stream connections: {exc.code} {detail}",
                }
            )
        except Exception as exc:  # noqa: BLE001
            self.events.put(
                {
                    "type": "error",
                    "message": f"failed to terminate stream connections: {exc}",
                }
            )

    def _move_selection(self, delta: int) -> None:
        if self.focus == "monitors":
            if not self.monitors:
                return
            self.selected_monitor = max(
                0, min(self.selected_monitor + delta, len(self.monitors) - 1)
            )
            return
        if not self.feed:
            return
        self.selected_feed = max(0, min(self.selected_feed + delta, len(self.feed) - 1))

    def _open_selected_feed_url(self) -> None:
        if not self.feed:
            self._push_info("no feed item selected")
            return
        item = self.feed[self.selected_feed]
        if not item.url:
            self._push_info("selected feed item has no URL")
            return
        try:
            webbrowser.open(item.url)
            self._push_info(f"opened {item.url}")
        except Exception as exc:  # noqa: BLE001
            self._push_error(f"failed to open URL: {exc}")

    def _render_header(self) -> Panel:
        status = Text()
        if self.stream_connected:
            status.append("Stream: connected", style="bold green")
        else:
            status.append("Stream: disconnected", style="bold red")
        status.append("  |  ")
        status.append(f"Monitors: {len(self.monitors)}")
        status.append("  |  ")
        status.append(self.status)
        return Panel(status, title="Home", border_style="bright_black")

    def _render_monitors(self) -> Panel:
        title = "Monitored Targets (focused)" if self.focus == "monitors" else "Monitored Targets"
        border = "cyan" if self.focus == "monitors" else "bright_black"
        if not self.monitors:
            return Panel(Text("No monitors yet. Press 'a' to add one."), title=title, border_style=border)

        rows: list[Text] = []
        for idx, monitor in enumerate(self.monitors):
            selected = self.focus == "monitors" and idx == self.selected_monitor
            row = Text()
            row.append("» " if selected else "  ", style="bold yellow" if selected else "")
            row.append(
                "● active" if monitor.active else "● inactive",
                style="green" if monitor.active else "red",
            )
            kind = "acct" if monitor.kind == "account" else "phrase"
            ai = f"AI:{monitor.analysis.provider}" if monitor.analysis.enabled else "AI:off"
            row.append(f" {monitor.label} [{kind}] {ai}")
            rows.append(row)

        return Panel(Group(*rows), title=title, border_style=border)

    def _render_feed(self) -> Panel:
        title = "Live Feed (focused)" if self.focus == "feed" else "Live Feed"
        border = "cyan" if self.focus == "feed" else "bright_black"

        if not self.feed:
            message = (
                "Add a target to activate live feed."
                if not self.monitors
                else "Waiting for matching posts..."
            )
            return Panel(Text(message), title=title, border_style=border)

        rows: list[Text] = []
        for idx, item in enumerate(self.feed):
            selected = self.focus == "feed" and idx == self.selected_feed
            style = {
                "POST": "white",
                "AI": "bright_cyan",
                "INFO": "grey62",
                "ERROR": "bright_red",
            }.get(item.kind, "white")
            row = Text()
            row.append("» " if selected else "  ", style="bold yellow" if selected else "")
            row.append(item.summary(), style=style)
            rows.append(row)

        return Panel(Group(*rows), title=title, border_style=border)

    def _render_details(self) -> Panel:
        if self.add_form:
            return self._render_add_form_panel()

        if self.focus == "monitors":
            monitor = self._selected_monitor()
            if monitor is None:
                return Panel(Text("Select a monitor to inspect details."), title="Details", border_style="bright_black")

            details = Text()
            details.append(f"Display name: {monitor.label}\n")
            details.append("Status: ")
            details.append(
                "active\n" if monitor.active else "inactive\n",
                style="green" if monitor.active else "red",
            )
            details.append(
                f"Kind: {'Account' if monitor.kind == 'account' else 'Phrase'}\n"
            )
            details.append(f"Target: {monitor.target}\n")
            details.append(f"Query: {monitor.query}\n")
            details.append(f"Rule ID: {monitor.rule_id}\n")
            details.append(f"Rule Tag: {monitor.rule_tag}\n")
            details.append(
                "AI: "
                + (
                    f"enabled ({monitor.analysis.provider})"
                    if monitor.analysis.enabled
                    else "disabled"
                )
                + "\n"
            )
            details.append(
                "Model: "
                + (
                    monitor.analysis.model
                    if monitor.analysis.model.strip()
                    else "(provider default)"
                )
                + "\n"
            )
            details.append(
                "Endpoint: "
                + (
                    monitor.analysis.endpoint
                    if monitor.analysis.endpoint.strip()
                    else "(provider default)"
                )
                + "\n"
            )
            api_key_ref = monitor.analysis.api_key.strip()
            if not api_key_ref:
                api_key_state = "(provider default/env)"
            elif is_env_var_name(api_key_ref) or (
                api_key_ref.startswith("$") and is_env_var_name(api_key_ref[1:])
            ):
                api_key_state = f"env ref ({api_key_ref})"
            else:
                api_key_state = "(monitor override)"
            details.append(f"API key: {api_key_state}\n")
            details.append(f"Prompt: {monitor.analysis.prompt}")
            return Panel(details, title="Details", border_style="bright_black")

        if not self.feed:
            return Panel(Text("Select a feed item to inspect details."), title="Details", border_style="bright_black")
        item = self.feed[self.selected_feed]
        body = Text(item.summary())
        if item.url:
            body.append(f"\nURL: {item.url}")
        return Panel(body, title="Details", border_style="bright_black")

    def _render_add_form_panel(self) -> Panel:
        form = self.add_form
        assert form is not None
        blink_on = int(time.time() * 1.5) % 2 == 0

        def with_cursor(value: str, selected: bool) -> str:
            if not selected:
                return value
            return f"{value}{'_' if blink_on else ' '}"

        provider_name = form.selected_provider(self.provider_names)
        api_key_value = api_key_input_display(form.ai_api_key)
        rows_data = [
            f"Type: {'Account' if form.kind == 'account' else 'Phrase'}",
            f"Target: {with_cursor(form.target, form.field_index == 1)}",
            f"Display name: {with_cursor(form.display_name, form.field_index == 2)}",
            f"Run AI analysis: {'Yes' if form.ai_enabled else 'No'}",
            f"AI provider: {provider_name}",
            f"AI model ID: {with_cursor(form.ai_model, form.field_index == 5)}",
            f"AI endpoint: {with_cursor(form.ai_endpoint, form.field_index == 6)}",
            f"AI API key: {with_cursor(api_key_value, form.field_index == 7)}",
            f"AI prompt: {with_cursor(form.ai_prompt, form.field_index == 8)}",
            "Create monitor (press Enter)",
        ]

        rows: list[Text] = []
        for idx, text in enumerate(rows_data):
            selected = idx == form.field_index
            row = Text()
            row.append("> " if selected else "  ", style="bold yellow" if selected else "")
            row.append(text, style="yellow" if selected else "")
            if selected:
                if idx in (0, 3, 4):
                    hint = "[<- ->]"
                elif idx == 9:
                    hint = "[Enter]"
                else:
                    hint = "[type]"
                row.append(f"  {hint}", style="green")
            rows.append(row)

        return Panel(Group(*rows), title="Add Target", border_style="cyan")

    def _render_footer(self) -> Panel:
        if self.add_form is not None:
            hints = Text.from_markup(
                "[green]Up/Down[/green] field  [green]Left/Right[/green] toggle/cycle  "
                "[green]Enter[/green] next/submit  [green]q[/green] cancel"
            )
        else:
            hints = Text.from_markup(
                "[green]a[/green] add  [green]d[/green] delete  [green]r[/green] reconnect target  "
                "[green]x[/green] kill conns  [green]Tab[/green] switch pane  "
                "[green]Up/Down[/green] navigate  [green]o[/green] open URL  [green]q[/green] quit"
            )
        return Panel(hints, title="Keyboard", border_style="bright_black")

    def _render(self) -> Layout:
        layout = Layout()
        layout.split_column(
            Layout(name="header", size=3),
            Layout(name="body"),
            Layout(name="details", size=12),
            Layout(name="footer", size=3),
        )
        layout["body"].split_row(
            Layout(name="monitors", ratio=34),
            Layout(name="feed", ratio=66),
        )
        layout["header"].update(self._render_header())
        layout["monitors"].update(self._render_monitors())
        layout["feed"].update(self._render_feed())
        layout["details"].update(self._render_details())
        layout["footer"].update(self._render_footer())
        return layout


def configure_logger(args: argparse.Namespace) -> SessionLogger | None:
    if args.log_session and args.log_file:
        raise ValueError("use only one of --log-session or --log-file")
    if args.log_file:
        return SessionLogger(Path(args.log_file).expanduser().resolve())
    if args.log_session:
        stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
        path = (Path(__file__).resolve().parents[1] / "logs" / f"session-{stamp}.log").resolve()
        return SessionLogger(path)
    return None


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Rich TUI for X filtered-stream testing")
    parser.add_argument("--bearer-token", help="X bearer token (or use X_BEARER_TOKEN env var)")
    parser.add_argument(
        "--config-file",
        help="Path to x-monitor.toml (default: X_MONITOR_CONFIG or project x-monitor.toml)",
    )
    parser.add_argument(
        "--state-file",
        default="tui-state.json",
        help="Path for monitor state JSON (default: python_test/tui-state.json)",
    )
    parser.add_argument(
        "--log-session",
        action="store_true",
        help="Write session log to python_test/logs/session-YYYYMMDD-HHMMSS.log",
    )
    parser.add_argument("--log-file", help="Write session log to custom file")
    return parser


def main() -> int:
    load_env()
    parser = build_parser()
    args = parser.parse_args()

    try:
        logger = configure_logger(args)
        token = resolve_bearer_token(args.bearer_token)
        config, config_path = AppConfig.load(args.config_file)
        client = Client(bearer_token=token)

        state_file = Path(args.state_file)
        if not state_file.is_absolute():
            state_file = Path(__file__).resolve().parents[1] / state_file

        app = XMonitorRichTui(
            client=client,
            bearer_token=token,
            state_path=state_file.resolve(),
            config=config,
            logger=logger,
        )
        if logger:
            logger.log("tui session started")
            logger.log(f"command: {' '.join(sys.argv)}")
            logger.log(f"config path: {config_path}")
        code = app.run()
        if logger:
            logger.log("tui session ended")
        return code
    except Exception as exc:  # noqa: BLE001
        print(f"ERROR {exc}", file=sys.stderr)
        traceback.print_exc()
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
