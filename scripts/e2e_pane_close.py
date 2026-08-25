#!/usr/bin/env python3
"""E2E: drive the real strimux binary in a PTY and verify natural pane close.

Scenario (the actual end-user workflow):
  1. strimux starts with 2 panes; pane 0 runs a command that exits after 1s.
  2. When it exits, the pane must close and the strip collapse leftward:
     the surviving shell pane becomes column 0 and takes focus.
  3. Typed input then lands in the surviving pane (echo MARKER), and the
     repaint places that pane's content at the LEFT edge (fill first left).
  4. `exit` in the last pane quits strimux entirely (process terminates).
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
    pid, fd = pty.fork()
    if pid == 0:
        os.execve(BIN, [BIN, "run", "sh -c 'sleep 1; exit 0'"], env)
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
    COLS x ROWS screen, ignoring SGR/other escapes. Enough to see where
    strimux painted each pane's content."""
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

    # Let both panes spawn and pane 0's command run out (1s) + collapse.
    pre = drain(fd, 2.5)
    alive = os.waitpid(pid, os.WNOHANG) == (0, 0)
    check("strimux survives a non-last pane exit", alive)

    # Type into whatever pane is focused now. If collapse+refocus worked,
    # this lands in the surviving shell.
    os.write(fd, b"echo MARKER_$((6*7))\r")
    out = drain(fd, 1.5)
    # The diff painter may split text across style runs and MoveTos, so
    # assert on the reconstructed screen, not the raw byte stream.
    screen = render(pre + out)
    check("surviving pane is focused and live (echo answered)",
          any("MARKER_42" in line for line in screen))

    # Fill-left: the marker must be painted in the left quarter of the
    # viewport, i.e. the surviving pane collapsed into column 0 rather than
    # staying at its old x offset.
    cols = [line.find("MARKER_42") for line in screen if "MARKER_42" in line]
    check("marker painted at the left edge (collapse filled left)",
          bool(cols) and min(cols) < COLS // 4,
          f"cols={cols}")

    # Last pane: exit must quit strimux.
    os.write(fd, b"exit\r")
    deadline = time.time() + 5
    quit_ok = False
    while time.time() < deadline:
        drain(fd, 0.2)
        if os.waitpid(pid, os.WNOHANG) != (0, 0):
            quit_ok = True
            break
    check("last pane exit quits strimux", quit_ok)
    if not quit_ok:
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
    sys.exit(0 if ok else 1)

if __name__ == "__main__":
    main()
