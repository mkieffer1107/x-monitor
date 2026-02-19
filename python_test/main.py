from __future__ import annotations

import argparse
import json
import os
import sys
import traceback
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Iterable

from dotenv import load_dotenv
from xdk import Client
from xdk.stream.models import UpdateRulesRequest


def timestamp() -> str:
    return datetime.now().strftime("%Y-%m-%d %H:%M:%S")


@dataclass
class SessionLogger:
    path: Path

    def __post_init__(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)

    def log(self, message: str) -> None:
        with self.path.open("a", encoding="utf-8") as fh:
            fh.write(f"[{timestamp()}] {message}\n")


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


def iter_pages(value: Any) -> Iterable[Any]:
    if value is None:
        return
    if isinstance(value, (dict, str, bytes)):
        yield value
        return
    try:
        iterator = iter(value)
    except TypeError:
        yield value
        return
    for item in iterator:
        yield item


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


def configure_logging(args: argparse.Namespace) -> SessionLogger | None:
    if args.log_session and args.log_file:
        raise ValueError("use only one of --log-session or --log-file")
    if args.log_file:
        return SessionLogger(Path(args.log_file).expanduser().resolve())
    if args.log_session:
        filename = f"session-{datetime.now().strftime('%Y%m%d-%H%M%S')}.log"
        return SessionLogger((Path(__file__).parent / "logs" / filename).resolve())
    return None


def load_env() -> None:
    load_dotenv()
    root_env = Path(__file__).resolve().parents[1] / ".env"
    if root_env.exists():
        load_dotenv(root_env)


def resolve_bearer_token(cli_value: str | None) -> str:
    if cli_value:
        return cli_value
    token = os.getenv("X_BEARER_TOKEN") or os.getenv("x_bearer_token")
    if not token:
        raise ValueError("missing X bearer token. Set X_BEARER_TOKEN or pass --bearer-token")
    return token


def list_rules(client: Client) -> list[dict[str, Any]]:
    all_rules: list[dict[str, Any]] = []
    pages = client.stream.get_rules()
    for page in iter_pages(pages):
        payload = to_dict(page)
        rules = payload.get("data") or []
        if isinstance(rules, list):
            all_rules.extend(rules)
    return all_rules


def cmd_rules_list(client: Client, logger: SessionLogger | None) -> int:
    rules = list_rules(client)
    if not rules:
        print("No stream rules configured.")
        if logger:
            logger.log("No stream rules configured.")
        return 0

    print(f"Configured rules ({len(rules)}):")
    for rule in rules:
        rule_id = rule.get("id", "<unknown>")
        value = rule.get("value", "<unknown>")
        tag = rule.get("tag", "")
        print(f"- id={rule_id} tag={tag} value={value}")
    if logger:
        logger.log(f"Listed {len(rules)} rule(s)")
    return 0


def cmd_rules_add(client: Client, args: argparse.Namespace, logger: SessionLogger | None) -> int:
    if args.account:
        handle = normalize_handle(args.account)
        value = f"from:{handle}"
        tag = args.tag or f"acct:{handle}"
    else:
        value = quote_phrase(args.phrase)
        tag = args.tag or f"phrase:{args.phrase[:24]}"

    body = UpdateRulesRequest(add=[{"value": value, "tag": tag}])
    response = client.stream.update_rules(body=body)
    payload = to_dict(response)
    print(json.dumps(payload, indent=2, ensure_ascii=False))
    if logger:
        logger.log(f"Added rule value={value} tag={tag}")
    return 0


def cmd_rules_delete(
    client: Client, args: argparse.Namespace, logger: SessionLogger | None
) -> int:
    body = UpdateRulesRequest(delete={"ids": args.ids})
    response = client.stream.update_rules(body=body)
    payload = to_dict(response)
    print(json.dumps(payload, indent=2, ensure_ascii=False))
    if logger:
        logger.log(f"Deleted rule ids={','.join(args.ids)}")
    return 0


def cmd_rules_clear(client: Client, logger: SessionLogger | None) -> int:
    rules = list_rules(client)
    ids = [rule.get("id") for rule in rules if rule.get("id")]
    if not ids:
        print("No rules to delete.")
        if logger:
            logger.log("No rules to delete.")
        return 0

    body = UpdateRulesRequest(delete={"ids": ids})
    response = client.stream.update_rules(body=body)
    payload = to_dict(response)
    print(json.dumps(payload, indent=2, ensure_ascii=False))
    if logger:
        logger.log(f"Cleared {len(ids)} rule(s)")
    return 0


def summarize_post(payload: dict[str, Any]) -> str:
    data = payload.get("data") or {}
    includes = payload.get("includes") or {}
    users = includes.get("users") or []
    id_to_username = {user.get("id"): user.get("username") for user in users if isinstance(user, dict)}

    post_id = data.get("id", "<unknown>")
    author_id = data.get("author_id")
    author = id_to_username.get(author_id, author_id or "unknown")
    text = (data.get("text") or "").replace("\n", " ").strip()
    text = text if text else "<no text>"

    return f"POST @{author} id={post_id} text={text}"


def cmd_stream(client: Client, args: argparse.Namespace, logger: SessionLogger | None) -> int:
    count = 0
    print("Connecting to filtered stream via official XDK...")
    if logger:
        logger.log("Connecting to filtered stream via official XDK")

    try:
        for event in client.stream.posts():
            payload = to_dict(event)
            if args.raw:
                raw = json.dumps(payload, ensure_ascii=False)
                print(raw)
                if logger:
                    logger.log(raw)
            elif payload.get("data"):
                line = summarize_post(payload)
                print(f"[{timestamp()}] {line}")
                if logger:
                    logger.log(line)
            else:
                line = json.dumps(payload, ensure_ascii=False)
                print(f"[{timestamp()}] EVENT {line}")
                if logger:
                    logger.log(f"EVENT {line}")

            if payload.get("data"):
                count += 1
                if args.max_posts and count >= args.max_posts:
                    print(f"Reached --max-posts={args.max_posts}; exiting stream.")
                    if logger:
                        logger.log(f"Reached --max-posts={args.max_posts}; exiting stream.")
                    break
    except KeyboardInterrupt:
        print("Stream stopped by user.")
        if logger:
            logger.log("Stream stopped by user.")
        return 0
    except Exception as exc:  # noqa: BLE001
        message = f"Stream error: {exc}"
        print(message, file=sys.stderr)
        traceback_str = traceback.format_exc()
        print(traceback_str, file=sys.stderr)
        if logger:
            logger.log(message)
            logger.log(traceback_str.rstrip())
        return 1

    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Official XDK filtered-stream test harness (uv-friendly)"
    )
    parser.add_argument(
        "--bearer-token",
        help="X bearer token (falls back to X_BEARER_TOKEN or x_bearer_token env vars)",
    )
    parser.add_argument(
        "--log-session",
        action="store_true",
        help="Write session log to python_test/logs/session-YYYYMMDD-HHMMSS.log",
    )
    parser.add_argument("--log-file", help="Write session log to a custom file path")

    subparsers = parser.add_subparsers(dest="command", required=True)

    rules = subparsers.add_parser("rules", help="Manage filtered stream rules")
    rules_sub = rules.add_subparsers(dest="rules_command", required=True)

    rules_sub.add_parser("list", help="List existing stream rules")

    rules_add = rules_sub.add_parser("add", help="Add a rule")
    group = rules_add.add_mutually_exclusive_group(required=True)
    group.add_argument("--account", help="Account handle, with or without leading @")
    group.add_argument("--phrase", help="Phrase to monitor")
    rules_add.add_argument("--tag", help="Optional custom rule tag")

    rules_delete = rules_sub.add_parser("delete", help="Delete rules by id")
    rules_delete.add_argument("--ids", nargs="+", required=True, help="One or more rule ids")

    rules_sub.add_parser("clear", help="Delete all existing rules")

    stream = subparsers.add_parser("stream", help="Consume filtered stream")
    stream.add_argument(
        "--max-posts",
        type=int,
        default=0,
        help="Stop after N posts (0 means unlimited)",
    )
    stream.add_argument(
        "--raw",
        action="store_true",
        help="Print and log raw event JSON instead of a compact summary",
    )

    return parser


def main() -> int:
    load_env()
    parser = build_parser()
    args = parser.parse_args()

    try:
        logger = configure_logging(args)
        token = resolve_bearer_token(args.bearer_token)
        client = Client(bearer_token=token)

        if logger:
            logger.log("Session started")
            logger.log(f"Command: {' '.join(sys.argv)}")

        if args.command == "rules":
            if args.rules_command == "list":
                return cmd_rules_list(client, logger)
            if args.rules_command == "add":
                return cmd_rules_add(client, args, logger)
            if args.rules_command == "delete":
                return cmd_rules_delete(client, args, logger)
            if args.rules_command == "clear":
                return cmd_rules_clear(client, logger)
            parser.error(f"unknown rules command: {args.rules_command}")

        if args.command == "stream":
            return cmd_stream(client, args, logger)

        parser.error(f"unknown command: {args.command}")
    except Exception as exc:  # noqa: BLE001
        print(f"ERROR {exc}", file=sys.stderr)
        traceback.print_exc()
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
