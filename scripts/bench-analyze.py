"""Reads runs from scripts/bench-sway.sh and prints, per step, the milliseconds from dispatch (or
process launch, for `cold`) to the UI thread picking the command up, the window being built, and
its first drawn frame.

    python3 scripts/bench-analyze.py target/bench/run-before target/bench/run-after
"""

import glob
import re
import sys
from datetime import datetime

ANSI = re.compile(r"\x1b\[[0-9;]*m")
KINDS = [(": dispatch", "dispatch"), ("ui command", "ui"), ("window built", "built"), ("first frame", "frame")]


def ts(text):
    return datetime.strptime(text[:26], "%Y-%m-%dT%H:%M:%S.%f").timestamp()


def load(run):
    events = []
    for path in glob.glob(f"{run}/home/.local/share/Laqta/logs/*"):
        for line in open(path, errors="replace"):
            line = ANSI.sub("", line)
            if "laqta::perf" not in line:
                continue
            kind = next((k for needle, k in KINDS if needle in line), None)
            if kind:
                events.append((ts(line), kind))
    events.sort()
    marks = [(ts(line), line.split(" ", 1)[1].strip()) for line in open(f"{run}/marks")]
    return events, marks


def steps(run):
    events, marks = load(run)
    out = {}
    for i, (t0, name) in enumerate(marks):
        if name == "seed":
            continue
        t1 = marks[i + 1][0] if i + 1 < len(marks) else float("inf")
        seg = [e for e in events if t0 <= e[0] < t1]
        start = t0 if name == "cold" else next((t for t, k in seg if k == "dispatch"), None)
        if start is None:
            continue
        first = lambda kind: next(((t - start) * 1000 for t, k in seg if k == kind), None)
        out[name] = (first("ui"), first("built"), first("frame"))
    return out


def main(runs):
    results = [steps(r) for r in runs]
    names = list(dict.fromkeys(n for r in results for n in r))
    fmt = lambda v: "-" if v is None else f"{v:.0f}"
    print(f"{'step (ms: ui/built/frame)':<26}" + "".join(f"{r.rstrip('/').split('run-')[-1]:>22}" for r in runs))
    for name in names:
        cells = [" / ".join(fmt(v) for v in r.get(name, (None, None, None))) for r in results]
        print(f"{name:<26}" + "".join(f"{c:>22}" for c in cells))


if __name__ == "__main__":
    main(sys.argv[1:])
