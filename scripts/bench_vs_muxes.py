#!/usr/bin/env python3
"""Benchmark gwae/gwae against tmux, Zellij, and a bare PTY.

Everything runs headless in a real PTY, driving the real binaries the way a
terminal would. Measures, per multiplexer:

  1. startup:    exec -> first drawn byte, and exec -> first keystroke echoed
                 (the moment you can actually type)
  2. echo RTT:   write one char -> see it echoed back through the mux,
                 median/p90/p99 over N samples (the honest input-latency number:
                 the mux sits on this path twice)
  3. memory:     RSS of the mux's own processes (server+client for tmux/zellij),
                 shells excluded, at 1 pane and 4 panes
  4. idle CPU:   cputime delta of mux processes over an idle window, 4 panes
  5. binary:     size of the executable(s) involved

Usage: python3 scripts/bench_vs_muxes.py [--samples 150] [--idle 10]
"""
import argparse, fcntl, json, os, pty, re, select, shutil, signal, struct, subprocess, sys, tempfile, termios, time

COLS, ROWS = 120, 30
GWAE = os.path.expanduser("~/.cargo/bin/gwae")
TMUX = shutil.which("tmux") or "/opt/homebrew/bin/tmux"
ZELLIJ = shutil.which("zellij") or os.path.expanduser("~/.local/bin/zellij")


def set_winsz(fd):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))


def read_avail(fd, timeout=0.05):
    r, _, _ = select.select([fd], [], [], timeout)
    if not r:
        return b""
    try:
        return os.read(fd, 65536)
    except OSError:
        return b""


def drain(fd, dur):
    end = time.monotonic() + dur
    buf = b""
    while time.monotonic() < end:
        buf += read_avail(fd, 0.05)
    return buf


class Mux:
    """One mux session running in a PTY."""

    def __init__(self, name, argv, env_extra=None, panes=1):
        self.name = name
        self.cfgdir = tempfile.mkdtemp(prefix=f"bench-{name}-")
        env = dict(os.environ, TERM="xterm-256color", SHELL="/bin/sh")
        env.pop("TMUX", None)
        env.pop("ZELLIJ", None)
        if env_extra:
            env.update(env_extra(self.cfgdir, panes))
        self.t_exec = time.monotonic_ns()
        pid, fd = pty.fork()
        if pid == 0:
            os.execve(argv[0], argv(self.cfgdir, panes) if callable(argv) else argv, env)
            os._exit(127)
        self.pid, self.fd = pid, fd
        set_winsz(fd)
        self.t_first_out = None
        self.t_echo_ready = None

    def wait_ready(self, deadline=15.0):
        """Record first output; then probe with a char until it echoes."""
        end = time.monotonic() + deadline
        probe_sent = None
        while time.monotonic() < end:
            chunk = read_avail(self.fd, 0.01)
            now = time.monotonic_ns()
            if chunk and self.t_first_out is None:
                self.t_first_out = now
            if probe_sent and chunk and b"q" in chunk:
                self.t_echo_ready = now
                try:
                    os.write(self.fd, b"\x15")  # ctrl-u: clear the line
                except OSError:
                    return False
                drain(self.fd, 0.3)
                return True
            if self.t_first_out and not probe_sent:
                time.sleep(0.05)
                try:
                    os.write(self.fd, b"q")
                except OSError:
                    return False
                probe_sent = time.monotonic_ns()
        return False

    def echo_rtt(self, samples, warmup=20):
        lat = []
        chars = b"abcdefghij"
        for i in range(samples + warmup):
            c = chars[i % len(chars):][:1]
            drain(self.fd, 0.005)
            t0 = time.monotonic_ns()
            try:
                os.write(self.fd, c)
            except OSError:
                break
            got = b""
            end = time.monotonic() + 2.0
            while time.monotonic() < end:
                got += read_avail(self.fd, 0.001)
                if c in got:
                    if i >= warmup:
                        lat.append((time.monotonic_ns() - t0) / 1e6)
                    break
            if i % 20 == 19:
                try:
                    os.write(self.fd, b"\x15")
                except OSError:
                    break
                drain(self.fd, 0.05)
        return lat

    def stop(self):
        try:
            os.kill(self.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            os.close(self.fd)
        except OSError:
            pass


def proc_rows():
    out = subprocess.run(["ps", "-axo", "pid,ppid,rss,cputime,comm"],
                         capture_output=True, text=True).stdout
    rows = []
    for line in out.splitlines()[1:]:
        parts = line.split(None, 4)
        if len(parts) == 5:
            pid, ppid, rss, cput, comm = parts
            rows.append((int(pid), int(ppid), int(rss), cput, comm))
    return rows


def descendants(root_pid, rows):
    """root_pid and all its live descendants."""
    kids = {}
    for pid, ppid, *_ in rows:
        kids.setdefault(ppid, []).append(pid)
    seen, stack = set(), [root_pid]
    while stack:
        p = stack.pop()
        if p in seen:
            continue
        seen.add(p)
        stack.extend(kids.get(p, []))
    return seen


def cputime_secs(s):
    # mm:ss.cc or hh:mm:ss
    parts = s.split(":")
    sec = 0.0
    for p in parts:
        sec = sec * 60 + float(p)
    return sec


def mux_stats(root_pid, name_filter):
    """RSS/cputime of *this session's* mux processes: descendants of root_pid
    (and, for client-server muxes, any matching-name process parented to init
    that appeared for this session) whose command matches name_filter."""
    rows = proc_rows()
    desc = descendants(root_pid, rows)
    ours = [r for r in rows if r[0] in desc and name_filter in r[4]]
    # client-server muxes daemonize: the server reparents to launchd/init.
    # Catch it by name, but only if it is NOT a descendant of some other
    # session (best effort: include orphaned name matches).
    orphan = [r for r in rows if r[0] not in desc and name_filter in r[4]
              and r[1] == 1]
    ours += orphan
    rss_kb = sum(r[2] for r in ours)
    cpu = sum(cputime_secs(r[3]) for r in ours)
    return rss_kb, cpu, len(ours)


def pctl(v, p):
    if not v:
        return float("nan")
    s = sorted(v)
    return s[min(len(s) - 1, int(round(p / 100 * (len(s) - 1))))]


def bench_one(label, spawn_fn, name_filter, samples, idle, panes4_fn):
    res = {"name": label}
    # -- startup + latency + 1-pane memory
    m = spawn_fn(1)
    ok = m.wait_ready()
    if not ok:
        m.stop()
        res["error"] = "never became interactive"
        return res
    res["start_first_out_ms"] = (m.t_first_out - m.t_exec) / 1e6
    res["start_echo_ready_ms"] = (m.t_echo_ready - m.t_exec) / 1e6
    lat = m.echo_rtt(samples)
    res["rtt_ms"] = {"median": pctl(lat, 50), "p90": pctl(lat, 90),
                     "p99": pctl(lat, 99), "n": len(lat)}
    time.sleep(0.5)
    rss, _, nproc = mux_stats(m.pid, name_filter)
    res["rss_1pane_kb"] = rss
    res["procs_1pane"] = nproc
    m.stop()
    time.sleep(0.5)
    time.sleep(0.5)
    # -- 4-pane memory + idle CPU
    try:
        m4 = panes4_fn()
        if m4 and m4.wait_ready():
            drain(m4.fd, 1.5)
            rss4, cpu_a, nproc4 = mux_stats(m4.pid, name_filter)
            t0 = time.monotonic()
            drain(m4.fd, idle)
            _, cpu_b, _ = mux_stats(m4.pid, name_filter)
            wall = time.monotonic() - t0
            res["rss_4pane_kb"] = rss4
            res["procs_4pane"] = nproc4
            res["idle_cpu_pct_4pane"] = 100.0 * (cpu_b - cpu_a) / wall
            m4.stop()
        elif m4:
            m4.stop()
            res["warn_4pane"] = "4-pane session did not become interactive"
    except OSError as e:
        res["warn_4pane"] = f"4-pane failed: {e}"
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--samples", type=int, default=150)
    ap.add_argument("--idle", type=float, default=10.0)
    args = ap.parse_args()

    results = []
    only = os.environ.get("BENCH_ONLY")

    def want(n):
        return only is None or only == n

    # ---- bare PTY /bin/sh (the floor) ----
    def bare(_p):
        return Mux("bare", ["/bin/sh", "-i"])
    if want("bare"):
        results.append(bench_one("bare /bin/sh (floor)", bare, "NOMATCH-bare",
                                 args.samples, args.idle, lambda: None))

    # ---- gwae ----
    def gwae_env(cfg, panes):
        os.makedirs(os.path.join(cfg, "gwae"), exist_ok=True)
        with open(os.path.join(cfg, "gwae", "gwae.toml"), "w") as f:
            f.write(f"startup_panes = {panes}\ninput_poll_ms = 1\n")
        return {"XDG_CONFIG_HOME": cfg}
    def gwae_spawn(panes):
        m = Mux.__new__(Mux)
        m.name = "gwae"
        m.cfgdir = tempfile.mkdtemp(prefix="bench-gwae-")
        env = dict(os.environ, TERM="xterm-256color", SHELL="/bin/sh")
        env.update(gwae_env(m.cfgdir, panes))
        m.t_exec = time.monotonic_ns()
        pid, fd = pty.fork()
        if pid == 0:
            os.execve(GWAE, [GWAE, "run"], env)
            os._exit(127)
        m.pid, m.fd = pid, fd
        set_winsz(fd)
        m.t_first_out = None
        m.t_echo_ready = None
        return m
    if want("gwae"):
        results.append(bench_one("gwae (gwae)", gwae_spawn, "gwae",
                                 args.samples, args.idle, lambda: gwae_spawn(4)))

    # ---- tmux ----
    def tmux_spawn(panes):
        m = Mux.__new__(Mux)
        m.name = "tmux"
        m.cfgdir = tempfile.mkdtemp(prefix="bench-tmux-")
        sock = os.path.join(m.cfgdir, "sock")
        env = dict(os.environ, TERM="xterm-256color", SHELL="/bin/sh")
        env.pop("TMUX", None)
        argv = [TMUX, "-S", sock, "-f", "/dev/null", "new-session"]
        for _ in range(panes - 1):
            argv += [";", "split-window"]
        m.t_exec = time.monotonic_ns()
        pid, fd = pty.fork()
        if pid == 0:
            os.execve(TMUX, argv, env)
            os._exit(127)
        m.pid, m.fd = pid, fd
        set_winsz(fd)
        m.t_first_out = None
        m.t_echo_ready = None
        return m
    if want("tmux"):
        results.append(bench_one("tmux", tmux_spawn, "tmux",
                                 args.samples, args.idle, lambda: tmux_spawn(4)))

    # ---- zellij ----
    def zellij_spawn(panes):
        m = Mux.__new__(Mux)
        m.name = "zellij"
        m.cfgdir = tempfile.mkdtemp(prefix="bench-zellij-")
        env = dict(os.environ, TERM="xterm-256color", SHELL="/bin/sh",
                   ZELLIJ_CONFIG_DIR=m.cfgdir)
        env.pop("ZELLIJ", None)
        argv = [ZELLIJ, "--session", f"bench{os.getpid()}{panes}"]
        if panes > 1:
            layout = os.path.join(m.cfgdir, "four.kdl")
            with open(layout, "w") as f:
                f.write("layout {\n" + "  pane\n" * panes + "}\n")
            argv += ["--layout", layout]
        # skip the startup tips popup
        with open(os.path.join(m.cfgdir, "config.kdl"), "w") as f:
            f.write('show_startup_tips false\nshow_release_notes false\n')
        m.t_exec = time.monotonic_ns()
        pid, fd = pty.fork()
        if pid == 0:
            os.execve(ZELLIJ, argv, env)
            os._exit(127)
        m.pid, m.fd = pid, fd
        set_winsz(fd)
        m.t_first_out = None
        m.t_echo_ready = None
        return m
    if want("zellij"):
        results.append(bench_one("zellij", zellij_spawn, "zellij",
                                 args.samples, args.idle, lambda: zellij_spawn(4)))

    # ---- binary sizes ----
    sizes = {}
    for label, path in [("gwae", GWAE), ("tmux", TMUX), ("zellij", ZELLIJ)]:
        try:
            sizes[label] = os.path.getsize(os.path.realpath(path))
        except OSError:
            sizes[label] = None

    print(json.dumps({"results": results, "binary_bytes": sizes,
                      "host": os.uname().machine, "samples": args.samples},
                     indent=2))


if __name__ == "__main__":
    main()
