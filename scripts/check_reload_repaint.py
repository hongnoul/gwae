#!/usr/bin/env python3
"""Acceptance check: a real gwae hot reload must not blank a real jcode pane.

The automated suites use stand-ins for the agent harness, because pulling
ratatui and jcode into gwae's dev-dependencies to test a repaint would be a
heavy and fragile dependency. This script closes that gap by driving the
actual binaries: real gwae, real jcode, real PTY, and a real binary swap of
the kind `make dev` performs.

It exists because of a bug that every cheaper check missed. The reload nudge
used to write `Ctrl-L` (\\x0c) into each adopted pane to ask for a redraw.
That is a redraw only in a shell; jcode binds Ctrl-L to terminal-style clear,
so each reload pushed the transcript off-screen and left the pane black. A
`printf`-on-signal stand-in cannot see that, and neither can "is the process
alive": the session survived, the screen did not.

What this measures, and the numbers it was calibrated against:

                        wordmark   glyphs   transcript
    Ctrl-L (the bug)      absent      829   lost
    observable resize     present    1976   restored

Usage:
    cargo build -p gwae
    python3 scripts/check_reload_repaint.py

Exits non-zero if the reloaded pane does not come back with jcode's own UI
painted in it. Requires `jcode` on PATH; skips (exit 0) when it is missing.
"""
import fcntl
import os
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GWAE = os.path.join(ROOT, "target", "debug", "gwae")

# Calibrated against the table above, with wide margins: the failing case
# produced 829 glyphs and no wordmark, the passing case 1976 and a wordmark.
MIN_GLYPHS = 1200
BOOT_SECONDS = 25
RELOAD_SECONDS = 15
# Box-drawing and whitespace are gwae's own chrome, not pane content.
CHROME = " \u2502\u2500\u250c\u2510\u2514\u2518\u251c\u2524\u252c\u2534\u253c\u256d\u256e\u256f\u2570"


def visible_text(buf: bytes) -> str:
    """Strip escape sequences so the check judges glyphs, not styling."""
    text = buf.decode("utf-8", "replace")
    text = re.sub(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)", "", text)  # OSC
    text = re.sub(r"\x1b[\[\?][0-9;?]*[a-zA-Z]", "", text)  # CSI
    return re.sub(r"\x1b.", "", text)


def install(src: str, dst: str) -> None:
    """Install like a real upgrade: write, sign, rename into place.

    The rename matters. Writing over a running Mach-O can be observed
    half-done, and an unsigned or stale-signed image is SIGKILLed on exec
    rather than rejected cleanly.
    """
    tmp = dst + ".new"
    shutil.copy(src, tmp)
    os.chmod(tmp, 0o755)
    subprocess.run(["codesign", "-f", "-s", "-", tmp], capture_output=True)
    os.replace(tmp, dst)


def main() -> int:
    if not shutil.which("jcode"):
        print("SKIP: jcode is not on PATH, so there is nothing to check")
        return 0
    if not os.path.exists(GWAE):
        print(f"FAIL: {GWAE} is missing; run `cargo build -p gwae` first")
        return 1

    work = tempfile.mkdtemp(prefix="gwae-reload-check-")
    cfg = os.path.join(work, "gwae")
    os.makedirs(cfg, exist_ok=True)
    with open(os.path.join(cfg, "gwae.toml"), "w") as fh:
        fh.write('default_agent = "jcode"\n')
    binary = os.path.join(work, "gwae-bin")
    install(GWAE, binary)

    env = dict(os.environ)
    env.update(
        {
            # Keep the check off the developer's real config and home.
            "XDG_CONFIG_HOME": work,
            "HOME": work,
            "TERM": "xterm-256color",
            "GWAE_DEV_RELOAD": "1",
            "SHELL": "/bin/sh",
        }
    )
    # The session must reload because *this script* swaps the binary, never
    # because a watcher rebuilt one underneath it.
    env.pop("GWAE_DEV_WATCH", None)

    child, master = pty.fork()
    if child == 0:
        os.environ.clear()
        os.environ.update(env)
        os.execv(binary, [binary, "run", "jcode"])

    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 170, 0, 0))
    captured = bytearray()

    def pump(seconds: float) -> None:
        end = time.time() + seconds
        while time.time() < end:
            ready, _, _ = select.select([master], [], [], 0.2)
            if not ready:
                continue
            try:
                chunk = os.read(master, 65536)
            except OSError:
                return
            if not chunk:
                return
            captured.extend(chunk)

    status = 1
    try:
        print(f"booting real jcode in a real gwae pane ({BOOT_SECONDS}s)...", flush=True)
        pump(BOOT_SECONDS)
        if "jcode" not in visible_text(bytes(captured)):
            print("INCONCLUSIVE: jcode never painted before the reload")
            return 0

        mark = len(captured)
        print("swapping the gwae binary to trigger a real hot reload...", flush=True)
        install(GWAE, binary)
        pump(RELOAD_SECONDS)

        after = visible_text(bytes(captured[mark:]))
        glyphs = [c for c in after if c.isprintable() and c not in CHROME]
        wordmark = "jcode" in after

        print()
        print(f"glyphs repainted after reload: {len(glyphs)}")
        print(f"jcode wordmark repainted:      {wordmark}")
        print()
        if wordmark and len(glyphs) >= MIN_GLYPHS:
            print("PASS: the reloaded pane repainted jcode's own UI")
            status = 0
        else:
            print(
                "FAIL: the reloaded pane came back without jcode's UI. The "
                "repaint nudge is not reaching the child, or is clearing it "
                "(see this file's header for the Ctrl-L history)."
            )
    finally:
        os.kill(child, signal.SIGKILL)
        try:
            os.waitpid(child, 0)
        except ChildProcessError:
            pass
        # The pane's own descendants live outside the process group above.
        subprocess.run(["pkill", "-f", work], capture_output=True)
        shutil.rmtree(work, ignore_errors=True)
    return status


if __name__ == "__main__":
    sys.exit(main())
