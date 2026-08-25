#!/usr/bin/env python3
"""E2E: drive the real strimux binary in a PTY and verify the minimap
agent dashboard.

Scenario (the actual end-user workflow):
  1. strimux starts with 2 panes; pane 0 runs a command that speaks OSC 133
     (command start, then done with exit 0) and then sleeps to keep the pane
     open.
  2. The minimap must appear bottom-right (2 panes > 1) with the summary bar
     tallying panes by status, and the OSC 133 pane's tile must flip to the
     done state (a `✓` glyph on screen).
  3. The shell pane emits no output; after the quiet window it must flip to
     wants-attention (`!` in the summary tallies).
  4. Column digits must be painted in the map tiles ('1' and '2').
"""
import os, pty, re, select, signal, struct, sys, tempfile, termios, fcntl, time

COLS, ROWS = 120, 30
BIN = os.path.join(os.path.dirname(__file__), "..", "target", "debug", "strimux")

def spawn():
    cfg = tempfile.mkdtemp(prefix="strimux-e2e-")
    os.makedirs(os.path.join(cfg, "strimux"), exist_ok=True)
    with open(os.path.join(cfg, "strimux", "strimux.toml"), "w") as f:
        f.write("startup_panes = 2\n")
    env = dict(os.environ, XDG_CONFIG_HOME=cfg, SHELL="/bin/sh", TERM="xterm-256color")
    # strimux's shell_split is naive about nested quotes, so put the OSC 133
    # emitter in a script file and run it plainly.
    script = os.path.join(cfg, "osc133.sh")
    with open(script, "w") as f:
        f.write('printf "\\033]133;C\\007"\nsleep 0.3\nprintf "\\033]133;D;0\\007"\nsleep 30\n')
    pid, fd = pty.fork()
    if pid == 0:
        # Pane 0: OSC 133 command-start, command-done (exit 0), then sleep so
        # the pane stays open with a Done status.
        os.execve(BIN, [BIN, "run", "sh " + script], env)
        os._exit(127)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    return pid, fd

def drain(fd, dur):
    out = b""
    end = time.time() + dur
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
    return out

def render(raw):
    """Minimal VT interpreter: apply CUP moves and printable text to a
    COLS x ROWS screen, ignoring SGR/other escapes."""
    screen = [[" "] * COLS for _ in range(ROWS)]
    row = col = 0
    txt = raw.decode("utf-8", "replace")
    i = 0
    esc = re.compile(r"\x1b(\[[0-9;?]*[A-Za-z]|\][^\x07\x1b]*(\x07|\x1b\\)|.)")
    while i < len(txt):
        ch = txt[i]
        if ch == "\x1b":
            m = esc.match(txt, i)
            if not m:
                i += 1
                continue
            seq = m.group(1)
            cup = re.fullmatch(r"\[(\d+);(\d+)H", seq)
            if cup:
                row, col = int(cup.group(1)) - 1, int(cup.group(2)) - 1
            elif seq == "[H":
                row = col = 0
            i = m.end()
            continue
        if ch == "\r":
            col = 0
        elif ch == "\n":
            row += 1
        elif ch >= " ":
            if 0 <= row < ROWS and 0 <= col < COLS:
                screen[row][col] = ch
            col += 1
        i += 1
    return ["".join(r) for r in screen]

def main():
    pid, fd = spawn()
    ok = True
    def check(name, cond, detail=""):
        nonlocal ok
        print(("PASS" if cond else "FAIL"), name, detail)
        ok = ok and cond

    # Wait past the OSC 133 D marker (0.3s) AND the 4s quiet window so the
    # protocol-less shell pane flips to wants-attention.
    raw = drain(fd, 6.0)
    alive = os.waitpid(pid, os.WNOHANG) == (0, 0)
    check("strimux is running", alive)
    screen = render(raw)
    bottom = screen[-8:]  # map + summary live in the last few rows

    # The map paints in the bottom-right corner with per-pane tiles carrying
    # their ⌥+digit column addresses.
    right = [line[COLS - 34:] for line in bottom]
    blob = "\n".join(right)
    check("map tile shows column digit 1", "1" in blob, repr(blob))
    check("map tile shows column digit 2", "2" in blob)

    # Pane 0 spoke OSC 133 D;0 -> Done: a ✓ appears (tile glyph and tally).
    check("done pane shows a ✓ glyph", "✓" in blob, repr(blob))

    # The silent shell pane flips to wants-attention after the quiet window.
    check("quiet pane shows the ! attention glyph", "!" in blob, repr(blob))

    # Summary bar: total pane count '2' followed by status tallies (one
    # attention pane, one done pane) on the line above the map rows.
    summary_ok = any(re.search(r"2(\s+[»!✓✗]\d+)+", line) for line in right)
    check("summary bar tallies panes by status", summary_ok, repr(right))
    tallies_ok = any(re.search(r"!1.*✓1|✓1.*!1", line) for line in right)
    check("summary tallies split 1 attention / 1 done", tallies_ok, repr(right))

    os.kill(pid, signal.SIGKILL)
    os.waitpid(pid, 0)
    sys.exit(0 if ok else 1)

if __name__ == "__main__":
    main()
