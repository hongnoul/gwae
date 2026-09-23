"""Real-PTY latency benchmark: gwae vs other multiplexers vs bare shell.

Usage:
  python3 scripts/latency_bench.py zsh    # bare-shell baseline
  python3 scripts/latency_bench.py gwae   # GWAE_BIN overrides the binary
  python3 scripts/latency_bench.py herdr  # needs herdr installed

Each target runs in an isolated HOME under /tmp/muxbench (create
home_{zsh,gwae,herdr} with a minimal .zshrc: PS1="%% ").

Measures, inside a real PTY (120x40):
  startup:    exec -> first output byte, and -> output quiesce (300ms silence)
  echo:       keystroke byte -> first output byte, N samples, quiet-gated
  spawn:      split-pane keychord -> first byte / quiesce
  throughput: `seq 1 100000` wall time until DONE marker renders
"""
import os, pty, sys, time, select, fcntl, termios, struct, signal, statistics, json

def now(): return time.monotonic()

class Mux:
    def __init__(self, cmd, home):
        env = dict(os.environ)
        env["HOME"] = home
        env["TERM"] = "xterm-256color"
        env["SHELL"] = "/bin/zsh"
        for k in ("XDG_CONFIG_HOME","XDG_STATE_HOME","ZDOTDIR"):
            env.pop(k, None)
        self.t_exec = now()
        pid, fd = pty.fork()
        if pid == 0:
            os.execvpe(cmd[0], cmd, env)
        self.pid, self.fd = pid, fd
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        self.alive = True

    def read_avail(self, timeout):
        """Return (bytes, t_first_byte or None) within timeout."""
        end = now() + timeout
        buf = b""; t_first = None
        while True:
            remaining = end - now()
            if remaining <= 0: break
            r,_,_ = select.select([self.fd],[],[],remaining)
            if not r: break
            try: d = os.read(self.fd, 65536)
            except OSError: self.alive=False; break
            if not d: self.alive=False; break
            if t_first is None: t_first = now()
            buf += d
        return buf, t_first

    def drain_until_quiet(self, quiet=0.3, cap=10.0):
        """Read until `quiet` seconds of silence. Returns (all bytes, t_last_byte)."""
        start = now(); buf = b""; t_last = None
        while now() - start < cap:
            r,_,_ = select.select([self.fd],[],[],quiet)
            if not r: break
            try: d = os.read(self.fd, 65536)
            except OSError: self.alive=False; break
            if not d: self.alive=False; break
            buf += d; t_last = now()
        return buf, t_last

    def write(self, data):
        os.write(self.fd, data)

    def kill(self):
        try:
            os.kill(self.pid, signal.SIGKILL)
            os.waitpid(self.pid, 0)
        except (ProcessLookupError, ChildProcessError): pass
        try: os.close(self.fd)
        except OSError: pass

def ensure_quiet(m, quiet=0.15, cap=5.0):
    m.drain_until_quiet(quiet=quiet, cap=cap)

def bench_echo(m, samples=60):
    lat = []
    for i in range(samples):
        ensure_quiet(m, quiet=0.12, cap=2.0)
        ch = b"abcdefghij"[i % 10:i % 10 + 1]
        t0 = now()
        m.write(ch)
        _, t1 = m.read_avail(1.0)
        if t1 is not None:
            lat.append((t1 - t0) * 1000)
        if i % 10 == 9:
            m.write(b"\x15")  # ctrl-u clear line
            ensure_quiet(m, quiet=0.12, cap=2.0)
    m.write(b"\x15"); ensure_quiet(m)
    return lat

def bench_throughput(m, n=100000):
    ensure_quiet(m)
    cmd = f'seq 1 {n}; printf "DO%s\\n" "NEMARK"\n'.encode()
    t0 = now()
    m.write(cmd)
    buf = b""; deadline = now() + 60
    while now() < deadline:
        d, tf = m.read_avail(0.5)
        buf += d
        if b"DONEMARK" in buf:
            return (now() - t0), len(buf)
        if not m.alive: break
    return None, len(buf)

def bench_spawn(m, chord, times=5, quiet=0.25, cap=5.0):
    """TTFB = first byte after the chord; quiesce = last byte before
    `quiet` seconds of silence.

    TTFB is the mux metric. Quiesce is dominated by the spawned shell's
    own startup (zsh with system rc files ~180 ms, fish ~40 ms on the
    same host), so only compare quiesce across muxes configured with the
    identical shell. One read loop tracks both ends, so a
    deferred fork (frame first, shell prompt later) is measured honestly
    instead of quiesce collapsing to TTFB."""
    res = []
    for _ in range(times):
        ensure_quiet(m, quiet=0.2, cap=4.0)
        t0 = now()
        m.write(chord)
        t_first = None; t_last = None; total = 0
        deadline = now() + cap
        while now() < deadline:
            r,_,_ = select.select([m.fd],[],[],quiet)
            if not r: break
            try: dd = os.read(m.fd, 65536)
            except OSError: m.alive=False; break
            if not dd: m.alive=False; break
            t_now = now()
            if t_first is None: t_first = t_now
            t_last = t_now; total += len(dd)
        ttfb = (t_first - t0)*1000 if t_first else None
        tq = (t_last - t0)*1000 if t_last else None
        res.append((ttfb, tq, total))
        time.sleep(0.2)
    return res

def stats(xs):
    xs = [x for x in xs if x is not None]
    if not xs: return {}
    xs = sorted(xs)
    return {"n": len(xs), "min": round(xs[0],2), "p50": round(statistics.median(xs),2),
            "p90": round(xs[int(len(xs)*0.9)-1],2), "max": round(xs[-1],2),
            "mean": round(statistics.fmean(xs),2)}

def run(name, cmd, home, chord, settle=4.0):
    m = Mux(cmd, home)
    d, t_first = m.read_avail(settle)
    t_ttfb = (t_first - m.t_exec)*1000 if t_first else None
    _, t_last = m.drain_until_quiet(quiet=0.4, cap=8.0)
    t_quiesce = ((t_last - m.t_exec)*1000) if t_last else t_ttfb
    time.sleep(0.5)
    out = {"name": name, "startup_first_byte_ms": round(t_ttfb,1) if t_ttfb else None,
           "startup_quiesce_ms": round(t_quiesce,1) if t_quiesce else None}
    out["echo_ms"] = stats(bench_echo(m))
    tp, nbytes = bench_throughput(m)
    out["seq100k_s"] = round(tp,2) if tp else None
    out["seq100k_bytes"] = nbytes
    if chord:
        sp = bench_spawn(m, chord)
        out["spawn_ttfb_ms"] = stats([a for a,_,_ in sp])
        out["spawn_quiesce_ms"] = stats([b for _,b,_ in sp])
    # RSS of the direct child (and, for herdr, note server separately outside)
    try:
        rss = int(os.popen(f"ps -o rss= -p {m.pid}").read().strip() or 0)
        out["client_rss_mb"] = round(rss/1024,1)
    except Exception: pass
    m.kill()
    return out

if __name__ == "__main__":
    which = sys.argv[1]
    if which == "zsh":
        r = run("bare-zsh", ["/bin/zsh","-f"], "/tmp/muxbench/home_zsh", None, settle=2.0)
    elif which == "gwae":
        gbin = os.environ.get("GWAE_BIN","/opt/homebrew/bin/gwae")
        r = run("gwae", [gbin,"run","/bin/zsh -f"], "/tmp/muxbench/home_gwae", b"\x1b\r")
    elif which == "herdr":
        r = run("herdr", ["/Users/justinhong/.local/bin/herdr"], "/tmp/muxbench/home_herdr", b"\x02v")
    print(json.dumps(r, indent=1))
