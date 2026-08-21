#!/usr/bin/env python3
"""Drive DashboardApp through a real POSIX PTY and return complete screens."""

from __future__ import annotations

import errno
import fcntl
import json
import os
import pty
import re
import select
import struct
import sys
import termios
import time


ANSI = re.compile(rb"\x1b\[[0-?]*[ -/]*[@-~]")


def set_size(fd: int, width: int, height: int) -> None:
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))


def read_available(fd: int, timeout: float, quiet: float = 0.08) -> bytes:
    output = bytearray()
    deadline = time.monotonic() + timeout
    quiet_deadline = time.monotonic() + quiet
    while time.monotonic() < deadline:
        wait = max(0.0, min(deadline, quiet_deadline) - time.monotonic())
        readable, _, _ = select.select([fd], [], [], wait)
        if not readable:
            if time.monotonic() >= quiet_deadline:
                break
            continue
        try:
            chunk = os.read(fd, 65536)
        except OSError as error:
            if error.errno == errno.EIO:
                break
            raise
        if not chunk:
            break
        output.extend(chunk)
        quiet_deadline = time.monotonic() + quiet
    return bytes(output)


def latest_screen(raw: bytes) -> str:
    marker = raw.rfind(b"\x1b[H")
    if marker < 0:
        return ""
    start = marker + len(b"\x1b[H")
    end = raw.find(b"\x1b[J", start)
    if end < 0:
        end = len(raw)
    screen = ANSI.sub(b"", raw[start:end]).replace(b"\r\n", b"\n").replace(b"\r", b"")
    return screen.decode("utf8", errors="replace")


def child_status(pid: int) -> int | None:
    result, status = os.waitpid(pid, os.WNOHANG)
    return None if result == 0 else os.waitstatus_to_exitcode(status)


def main() -> int:
    width = int(sys.argv[1])
    height = int(sys.argv[2])
    settings_path = sys.argv[3]
    count_path = sys.argv[4]
    steps = json.loads(sys.argv[5])
    root = sys.argv[6]
    history_path = sys.argv[7]
    transition_path = sys.argv[8]

    pid, master = pty.fork()
    if pid == 0:
        set_size(1, width, height)
        environment = dict(os.environ)
        environment.update(
            {
                "NO_COLOR": "1",
                "TEST_SETTINGS_PATH": settings_path,
                "TEST_COLLECT_COUNT_PATH": count_path,
                "TEST_HISTORY_PATH": history_path,
                "TEST_TRANSITION_PATH": transition_path,
            }
        )
        os.chdir(root)
        os.execvpe("node", ["node", "test/bin/pty-app.mjs"], environment)

    set_size(master, width, height)
    raw = bytearray()
    initial_deadline = time.monotonic() + 4.0
    while time.monotonic() < initial_deadline:
        raw.extend(read_available(master, 0.5))
        screen = latest_screen(bytes(raw))
        if len(screen.split("\n")) == height:
            break
    screens = [latest_screen(bytes(raw))]
    for step in steps:
        if "hex" in step:
            value = bytes.fromhex(step["hex"])
        else:
            value = step.get("text", "").encode("utf8")
        if value:
            os.write(master, value)
        delay = float(step.get("delay", 0.0))
        if delay:
            time.sleep(delay)
        if step.get("settle", True):
            raw.extend(read_available(master, 2.0))
            screens.append(latest_screen(bytes(raw)))

    status = child_status(pid)
    if status is None:
        try:
            os.write(master, b"q")
        except OSError as error:
            # macOS reports EIO when the slave closes before waitpid observes
            # the child exit. The scripted steps may already have sent "q".
            if error.errno != errno.EIO:
                raise
        raw.extend(read_available(master, 2.0))
        deadline = time.monotonic() + 2.0
        while status is None and time.monotonic() < deadline:
            time.sleep(0.02)
            status = child_status(pid)
    os.close(master)
    print(json.dumps({"screens": screens, "exitCode": status}))
    return 0 if status in (0, None) else status


if __name__ == "__main__":
    raise SystemExit(main())
