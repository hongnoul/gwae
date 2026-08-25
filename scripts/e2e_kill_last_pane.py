#!/usr/bin/env python3
"""E2E: closing every pane with the kill-pane verb must quit strimux.

Scenario: start with 2 panes, press `Ctrl-b x` twice. After the second kill
there are no panes left, so the process must terminate instead of resurrecting
a fresh default layout.
"""
import os, pty, select, struct, signal, sys, tempfile, termios, fcntl, time

COLS, ROWS = 120, 30
BIN = os.path.join(os.path.dirname(__file__), "..", "target", "debug", "strimux")


def spawn(panes=2):
    cfg = tempfile.mkdtemp(prefix="strimux-e2e-")
    os.makedirs(os.path.join(cfg, "strimux"), exist_ok=True)
    with open(os.path.join(cfg, "strimux", "strimux.toml"), "w") as f:
        f.write(f"startup_panes = {panes}\n")
    env = dict(os.environ, XDG_CONFIG_HOME=cfg, SHELL="/bin/sh", TERM="xterm-256color")
    pid, fd = pty.fork()
    if pid == 0:
        os.execve(BIN, [BIN], env)
        os._exit(127)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    return pid, fd


def drain(fd, dur):
    end = time.time() + dur
    out = b""
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


def main():
    pid, fd = spawn(2)
    ok = True

    def check(name, cond, detail=""):
        nonlocal ok
        print(("PASS" if cond else "FAIL"), name, detail)
        ok = ok and cond

    drain(fd, 1.5)
    # First kill: one pane left, strimux keeps running.
    os.write(fd, b"\x02x")
    drain(fd, 1.0)
    check("strimux survives killing a non-last pane",
          os.waitpid(pid, os.WNOHANG) == (0, 0))

    # Second kill: no panes left, must exit.
    os.write(fd, b"\x02x")
    deadline = time.time() + 5
    quit_ok = False
    while time.time() < deadline:
        drain(fd, 0.2)
        if os.waitpid(pid, os.WNOHANG) != (0, 0):
            quit_ok = True
            break
    check("killing the last pane quits strimux", quit_ok)
    if not quit_ok:
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
