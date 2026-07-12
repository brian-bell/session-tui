#!/usr/bin/env python3
"""End-to-end driver for session-tui.

Spawns the release binary in a real PTY with a fake `claude` on PATH and a
hermetic $HOME (fixture transcript, empty codex store), then drives
list -> picker -> launch -> input passthrough -> auto-hide resize ->
focus toggle -> auto-hide toggle -> kill -> quit
and asserts on the rendered frames.

This exercises the layers unit tests can't reach — real key encoding through
crossterm (e.g. the Ctrl+\\ legacy 0x1C quirk), PTY lifecycle, and rendering —
and has caught bugs the unit suite missed (Ctrl+backslash encoding, the picker
substring-match trap).

Usage:
    python3 scripts/e2e.py

Builds target/release/session-tui first; set SESSION_TUI_BIN to skip the
build and test a specific binary. Unix only (uses pty), like the test suite.
Exit code 0 means all checks passed.
"""

import fcntl
import json
import os
import pty
import re
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
FIXTURE_TITLE = "e2e fixture session"
FIXTURE_ID = "5a794577-05ea-4308-b6c9-6a0c46a2c845"

# The child TUI's escape sequences, stripped before asserting: CSI, OSC,
# charset selection, and keypad mode.
ANSI = re.compile(r"\x1b\[[0-9;?]*[a-zA-Z]|\x1b\][^\x07]*\x07|\x1b[()][B0]|\x1b[=>]")

acc = ""  # cumulative de-ANSIfied output; asserts run against this
failures = []


def build_binary() -> Path:
    override = os.environ.get("SESSION_TUI_BIN")
    if override:
        return Path(override)
    subprocess.run(["cargo", "build", "--release"], cwd=REPO, check=True)
    # Ask Cargo where artifacts land: CARGO_TARGET_DIR / build.target-dir
    # redirect them away from <repo>/target.
    meta = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=REPO,
        check=True,
        capture_output=True,
        text=True,
    )
    target_dir = Path(json.loads(meta.stdout)["target_directory"])
    return target_dir / "release" / "session-tui"


def drain(fd: int, secs: float) -> None:
    global acc
    out = b""
    end = time.time() + secs
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            out += chunk
    acc += ANSI.sub("", out.decode("utf-8", "replace"))


def flat(s: str) -> str:
    """Collapse whitespace: the renderer draws blank cells as cursor moves
    (stripped as ANSI) or spaces depending on the frame diff, and long lines
    wrap, so matching must ignore spacing entirely."""
    return re.sub(r"\s+", "", s)


def wait_for(fd: int, fragment: str, timeout: float = 6.0) -> bool:
    """Drain until fragment appears (whitespace-insensitively) in the
    cumulative output."""
    needle = flat(fragment)
    end = time.time() + timeout
    while needle not in flat(acc) and time.time() < end:
        drain(fd, 0.2)
    return needle in flat(acc)


def check(name: str, cond: bool) -> None:
    print(("PASS " if cond else "FAIL ") + name)
    if not cond:
        failures.append(name)


def write_fixture_transcript(home: Path, cwd: Path) -> None:
    """Minimal Claude transcript, same shape as tests/fixtures/mod.rs."""
    project = home / ".claude" / "projects" / str(cwd).replace("/", "-")
    project.mkdir(parents=True)
    line = {
        "parentUuid": None,
        "isSidechain": False,
        "type": "user",
        "message": {"role": "user", "content": FIXTURE_TITLE},
        "uuid": "fefde5ee-8225-4fb2-a95b-4a67a19f69ae",
        "timestamp": "2026-06-26T02:13:40.902Z",
        "cwd": str(cwd),
        "sessionId": FIXTURE_ID,
    }
    (project / f"{FIXTURE_ID}.jsonl").write_text(json.dumps(line) + "\n")


def write_claude_shim(shim_dir: Path) -> None:
    """Fake `claude`: announces its cwd, then echoes stdin line by line.
    `size` reports the PTY dimensions the child actually has — the only
    ground truth for auto-hide resizing frames can't show."""
    fake = shim_dir / "claude"
    fake.write_text(
        '#!/bin/sh\n'
        'echo "FAKE-CLAUDE started in $(pwd) args:$*"\n'
        'while IFS= read -r line; do\n'
        '  echo "echo:$line"\n'
        '  [ "$line" = "size" ] && echo "SIZE:$(stty size)"\n'
        '  [ "$line" = "exit" ] && break\n'
        'done\n'
    )
    fake.chmod(0o755)


def main() -> int:
    binary = build_binary()
    if not binary.is_file():
        print(f"binary not found: {binary}", file=sys.stderr)
        return 2

    with tempfile.TemporaryDirectory(prefix="session-tui-e2e-") as tmp:
        root = Path(tmp)
        home = root / "home"
        # The fixture transcript's cwd lives *inside* the launch dir, so the
        # typed launch path substring-matches a known picker row and
        # chosen_dir's typed-path-over-match precedence is genuinely on the
        # line (the picker trap this harness originally caught). The cwds
        # are still distinct, so the launched session can't be confused
        # with (or adopted by) the fixture transcript's row.
        launch_dir = root / "work" / "e2e-target"
        fixture_cwd = launch_dir / "fixture-cwd"
        fixture_cwd.mkdir(parents=True)
        write_fixture_transcript(home, fixture_cwd)
        shim = root / "shim"
        shim.mkdir()
        write_claude_shim(shim)

        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        env = dict(
            os.environ,
            HOME=str(home),
            PATH=f"{shim}:{os.environ['PATH']}",
            TERM="xterm-256color",
        )
        proc = subprocess.Popen(
            [str(binary)], stdin=slave, stdout=slave, stderr=slave, env=env
        )

        def send(s, wait=0.3):
            os.write(master, s.encode() if isinstance(s, str) else s)
            drain(master, wait)

        try:
            check("list renders", wait_for(master, " Sessions ") and " Terminal " in acc)
            check("fixture transcript listed", wait_for(master, FIXTURE_TITLE))

            send("n")
            check("picker opens", wait_for(master, " New session "))
            # Type the path key by key; a full absolute path must win over
            # the fixture row it substring-matches (the picker trap).
            for c in str(launch_dir):
                send(c, 0.02)
            send("\r")
            check("fake claude launched", wait_for(master, "FAKE-CLAUDE started in"))
            # The shim's pwd ends with the picked dir, right before " args:".
            check("launched in picked dir", flat("e2e-target args:") in flat(acc))

            send("hello\r")
            check("input passthrough", wait_for(master, "echo:hello"))

            # Auto-hide is on by default: the launch focused the terminal,
            # so the list pane is hidden and the child's PTY spans the full
            # 120-col frame (minus 2 border cols; 40 rows - help - borders).
            send("size\r")
            check("auto-hide grows the pty to full width", wait_for(master, "SIZE: 37 118"))

            # Ctrl+\ (legacy Ctrl+4 encoding): focus back to list. The
            # provisional row's title only ever renders while the list is
            # visible, so seeing it proves the panel came back.
            send("\x1c")
            check("focus toggle back to list", wait_for(master, "(new session)"))

            send("h")
            check("auto-hide toggles off", wait_for(master, "auto-hide off"))
            send("\x1c")  # into the terminal; the list stays visible now
            send("size\r")
            check("visible list narrows the pty", wait_for(master, "SIZE: 37 88"))

            send("\x1c")  # back to list for the kill flow
            send("\x0b")  # Ctrl+K: kill the selected session
            check("kill confirm shown", wait_for(master, "Kill session?"))
            send("y", 1.0)

            send("q")
            deadline = time.time() + 5
            while proc.poll() is None and time.time() < deadline:
                drain(master, 0.2)
            check("quit exits cleanly", proc.poll() == 0)
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()
            os.close(master)
            os.close(slave)

    if failures:
        print("\n--- cumulative output tail ---")
        print(acc[-2500:])
        return 1
    print("ALL OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
