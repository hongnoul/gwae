#!/usr/bin/env python3
"""Did the Mac sleep while gwae keep-awake (option+w) was on?

Reads `pmset -g log` and finds every gwae keep-awake window: gwae's
`caffeinate -i -m -s -w <pid>` is the only caffeinate here that holds
PreventDiskIdle (`-m`), and its ClientDied/Released line carries the
assertion's age, so start = end - age. Then it reports any real sleep
(Clamshell or Idle, not dark-wake Maintenance cycles) inside a window.

Exit 0 when no window contains a sleep, 1 otherwise. Usage:

    scripts/check-keep-awake-sleeps.py [--since YYYY-MM-DD[THH:MM]]

To verify the lid fix: press option+w in gwae, unplug, close the lid for a
few minutes, open it, and run this with --since set to just before.
"""

import re
import subprocess
import sys
from datetime import datetime, timedelta

TS = r"(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d)"
END = re.compile(
    TS + r" [-+]\d{4} Assertions\s+PID (\d+)\(caffeinate\) (?:ClientDied|Released) "
    r'PreventDiskIdle "caffeinate command-line tool" (\d+):(\d\d):(\d\d)'
)
HELD = re.compile(
    TS + r" [-+]\d{4} Assertions\s+PID (\d+)\(caffeinate\) Summary "
    r'PreventDiskIdle "caffeinate command-line tool" (\d+):(\d\d):(\d\d)'
)
SLEEP = re.compile(
    TS + r" [-+]\d{4} Sleep\s+Entering Sleep state due to '(Clamshell Sleep|Idle Sleep)'"
    r".*Using (AC|Batt)"
)


def parse(ts):
    return datetime.strptime(ts, "%Y-%m-%d %H:%M:%S")


def main(argv):
    since = None
    if len(argv) == 3 and argv[1] == "--since":
        since = datetime.fromisoformat(argv[2].replace(" ", "T"))
    elif len(argv) != 1:
        print(__doc__)
        return 2
    log = subprocess.run(
        ["pmset", "-g", "log"], capture_output=True, text=True, check=False
    ).stdout
    windows = {}  # pid -> (start, end); end None while still held
    sleeps = []
    for line in log.splitlines():
        m = END.match(line) or HELD.match(line)
        if m:
            at, pid = parse(m.group(1)), m.group(2)
            age = timedelta(
                hours=int(m.group(3)), minutes=int(m.group(4)), seconds=int(m.group(5))
            )
            start = at - age
            ended = END.match(line) is not None
            prev = windows.get(pid)
            windows[pid] = (min(start, prev[0]) if prev else start, at if ended else None)
            continue
        m = SLEEP.match(line)
        if m:
            sleeps.append((parse(m.group(1)), m.group(2), m.group(3)))
    now = datetime.now()
    bad = 0
    for pid, (start, end) in sorted(windows.items(), key=lambda kv: kv[1][0]):
        end_eff = end or now
        if since and end_eff < since:
            continue
        inside = [s for s in sleeps if start <= s[0] <= end_eff]
        state = "held" if end is None else f"until {end:%m-%d %H:%M}"
        print(f"keep-awake window pid {pid}: {start:%m-%d %H:%M} {state}")
        for at, why, power in inside:
            bad += 1
            print(f"  SLEPT {at:%m-%d %H:%M:%S}  {why} on {power}")
    if not windows:
        print("no gwae keep-awake windows in the pmset log")
    print("FAIL: slept while keep-awake was on" if bad else "OK: no sleep while keep-awake was on")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
