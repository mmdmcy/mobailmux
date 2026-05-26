from __future__ import annotations

import re
from dataclasses import dataclass


@dataclass(frozen=True)
class CommandMessage:
    text: str
    explicit: bool


def parse_command_message(message: str) -> CommandMessage:
    text = message.strip()
    if text.startswith("!"):
        return CommandMessage(text=text[1:].strip(), explicit=True)
    return CommandMessage(text=text, explicit=False)


def cd_target_arg(text: str) -> str | None:
    match = re.fullmatch(r"cd(?:\s+(.+))?", text, flags=re.IGNORECASE | re.DOTALL)
    if match:
        target = (match.group(1) or "~").strip()
        return target or "~"

    match = re.fullmatch(r"(?:folder|workdir)\s+(.+)", text, flags=re.IGNORECASE | re.DOTALL)
    if match:
        return match.group(1).strip()

    return None


def queue_request_arg(text: str) -> str | None:
    match = re.fullmatch(r"(?:next|queue)\s+(.+)", text, flags=re.IGNORECASE | re.DOTALL)
    if not match:
        return None
    request = match.group(1).strip()
    return request or None


def unknown_command_text(text: str) -> str:
    display = text or "!"
    if len(display) > 100:
        display = display[:80] + "\n...[truncated]"
    return (
        f"Unknown Mobailmux command: `{display}`. "
        "Type `!help` for commands, or send a normal message without `!` to talk to the agent."
    )
