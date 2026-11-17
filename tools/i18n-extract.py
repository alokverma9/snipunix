#!/usr/bin/env python3
"""Extract translatable messages from the Snipunix Rust sources.

Snipunix marks translatable strings with the `tr!` and `tr_fmt!` macros from
the `snipunix-i18n` crate. The first string literal argument of such a call is
the message id: the English text that is shown when no translation exists.

The output is a gettext template (`po/snipunix.pot`) that translators work
with, in the same format `xgettext` produces for other projects.

Usage:
    tools/i18n-extract.py [output.pot]
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPOSITORY = Path(__file__).resolve().parent.parent
SOURCE_ROOTS = [REPOSITORY / "crates"]
DEFAULT_OUTPUT = REPOSITORY / "po" / "snipunix.pot"

# `tr!("...")` and `tr_fmt!("...")`, with optional whitespace. The macro path
# is optional so `snipunix_i18n::tr!("...")` is matched as well. A literal may
# span lines with a trailing backslash, so the pattern is dot-all.
MESSAGE_PATTERN = re.compile(
    r"""\b(?:snipunix_i18n::)?\btr(?:_fmt)?!\s*\(\s*"""
    r"""(?P<quote>")"""
    r"""(?P<text>(?:[^"\\]|\\.)*)"""
    r"""(?P=quote)""",
    re.DOTALL,
)

# Rust string escapes that can appear in a message id.
ESCAPES = {
    "n": "\n",
    "t": "\t",
    "r": "\r",
    "0": "\0",
    "\\": "\\",
    '"': '"',
    "'": "'",
}


def decode(text: str) -> str:
    """Decodes the Rust string literal escapes used in message ids."""
    output = []
    index = 0
    while index < len(text):
        character = text[index]
        if character != "\\":
            output.append(character)
            index += 1
            continue
        index += 1
        if index >= len(text):
            break
        escape = text[index]
        if escape == "\n":
            # A line continuation: Rust drops the newline and the whitespace
            # that starts the next line.
            index += 1
            while index < len(text) and text[index] in " \t":
                index += 1
            continue
        if escape in ESCAPES:
            output.append(ESCAPES[escape])
        elif escape == "u":
            # \u{...} is the only unicode form Rust accepts.
            end = text.find("}", index)
            if end != -1:
                try:
                    output.append(chr(int(text[index + 2 : end], 16)))
                    index = end
                except ValueError:
                    output.append(escape)
            else:
                output.append(escape)
        else:
            output.append(escape)
        index += 1
    return "".join(output)


def escape(text: str) -> str:
    """Escapes a message for a gettext catalogue."""
    return (
        text.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("\n", "\\n")
        .replace("\t", "\\t")
    )


def collect() -> dict[str, list[str]]:
    """Returns every message id with the source locations that use it."""
    messages: dict[str, list[str]] = {}
    for root in SOURCE_ROOTS:
        for path in sorted(root.rglob("*.rs")):
            if "target" in path.parts:
                continue
            relative = path.relative_to(REPOSITORY)
            contents = path.read_text(encoding="utf-8")
            for match in MESSAGE_PATTERN.finditer(contents):
                message = decode(match.group("text"))
                if not message.strip():
                    continue
                number = contents.count("\n", 0, match.start()) + 1
                messages.setdefault(message, []).append(f"{relative}:{number}")
    return messages


def render(messages: dict[str, list[str]]) -> str:
    """Renders the messages as a gettext template."""
    header = (
        "# Snipunix translation template.\n"
        "# Copyright (C) 2026 Snipunix contributors\n"
        "# This file is distributed under the same license as the Snipunix package.\n"
        "#\n"
        "#, fuzzy\n"
        'msgid ""\n'
        'msgstr ""\n'
        '"Project-Id-Version: snipunix 0.6.0\\n"\n'
        '"Report-Msgid-Bugs-To: \\n"\n'
        '"MIME-Version: 1.0\\n"\n'
        '"Content-Type: text/plain; charset=UTF-8\\n"\n'
        '"Content-Transfer-Encoding: 8bit\\n"\n'
        '"Plural-Forms: nplurals=2; plural=(n != 1);\\n"\n'
    )
    chunks = [header]
    for message in sorted(messages):
        for location in messages[message]:
            chunks.append(f"#: {location}\n")
        chunks.append(f'msgid "{escape(message)}"\n')
        chunks.append('msgstr ""\n')
        chunks.append("\n")
    return "".join(chunks)


def main() -> int:
    output = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_OUTPUT
    messages = collect()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(render(messages), encoding="utf-8")
    print(f"{output}: {len(messages)} messages")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
