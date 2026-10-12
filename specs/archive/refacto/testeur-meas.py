#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (C) 2026 Ronan Lamour
# Reference measure of the dashboard / journal / server cost on a team of fakes (copy of the release binary).
# usage: meas.py <members> <working|idle> <seconds> [grid|focus] [client]
import fcntl, glob, json, os, pty, re, signal, struct, subprocess, sys, termios, threading, time

B = "/tmp/rtbench.Nggu"
n, st, secs = int(sys.argv[1]), sys.argv[2], int(sys.argv[3])
layout = sys.argv[4] if len(sys.argv) > 4 else "grid"
with_client = len(sys.argv) > 5 and sys.argv[5] == "client"
STEP = 5

R = subprocess.check_output(["mktemp", "-d", "/tmp/rtm.XXXX"], text=True).strip()
assert R.startswith("/tmp/rtm.")
for d in ["proj/.recruit", "profile", "config", "cache", "home", "run"]:
    os.makedirs(f"{R}/{d}")
SHIM = f"""#!/bin/sh
D="${{CLAUDE_CONFIG_DIR:-/nonexistent}}/fake"
case "$1" in
  --version) echo "2.1.295 (Claude Code)"; exit 0;;
  agents)
    mkdir -p "$D"
    printf '['; sep=''
    for f in "$D"/run/*.json; do
      [ -f "$f" ] || continue
      p=${{f##*/}}; p=${{p%.json}}
      kill -0 "$p" 2>/dev/null || continue
      printf '%s' "$sep"; cat "$f"; sep=','
    done
    printf ']\\n'; exit 0;;
esac
exec env RECRUIT_TEST_CHILD=claude '{B}/mux_bench' child --exact --ignored --nocapture --test-threads=1 -q '{R}' -- "$@"
"""
open(f"{R}/claude", "w").write(SHIM)
os.chmod(f"{R}/claude", 0o755)
members = ["lead"] + [f"dev{i}" for i in range(1, n)]
toml = f'[claude]\ncommand = "{R}/claude"\nconfig_dir = "{R}/profile"\n\n[teams.m]\ndescription = "bench"\n'
if layout == "focus":
    toml += 'layout = "focus"\n'
toml += '[teams.m.members.lead]\nrole = "Contact"\ncontact = true\n'
for m in members[1:]:
    toml += f'[teams.m.members.{m}]\nrole = "Agent"\n'
open(f"{R}/proj/.recruit/settings.toml", "w").write(toml)
ENV = {
    "PATH": os.environ["PATH"], "HOME": f"{R}/home", "SHELL": "/bin/sh", "TERM": "xterm-256color",
    "XDG_CONFIG_HOME": f"{R}/config", "XDG_CACHE_HOME": f"{R}/cache", "RECRUIT_TMPDIR": f"{R}/run",
    "RECRUIT_LANG": "fr", "LANG": "en_US.UTF-8",
}
REC = f"{B}/recruit"


def run(*args, cwd=None):
    return subprocess.run([REC, *args], env=ENV, cwd=cwd, capture_output=True, text=True, stdin=subprocess.DEVNULL)


def cpu(pid):
    out = subprocess.run(["ps", "-o", "time=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    if not out:
        return None
    parts = out.split(":")
    sec = 0.0
    for p in parts:
        sec = sec * 60 + float(p)
    return sec


def pids():
    out = subprocess.run(["ps", "-axo", "pid=,args="], capture_output=True, text=True).stdout
    found = {}
    for line in out.splitlines():
        line = line.strip()
        pid, _, args = line.partition(" ")
        if R not in args:
            continue
        for key, pat in [("server", "_server"), ("dashboard", "_panel dashboard"), ("journal", "_panel journal"),
                         ("client", " attach")]:
            if pat in args and "recruit" in args.split()[0] + args:
                found.setdefault(key, int(pid))
    return found


out = run("--detach", cwd=f"{R}/proj")
assert out.returncode == 0, out.stdout + out.stderr
state = glob.glob(f"{R}/cache/recruit/teams/*")[0]


def ctl(*a):
    return run("_ctl", state, *a)


def stats():
    return json.loads(ctl("stats", "--json").stdout)


deadline = time.time() + 30
while time.time() < deadline:
    panes = json.loads(ctl("panes", "--json").stdout or "[]")
    if len([p for p in panes if p.get("role") == ""]) >= n:
        break
    time.sleep(0.5)
time.sleep(3)
for m in members:
    ctl("send", "--pane", m, f"osc {st}\\r")
    ctl("send", "--pane", m, f"agent {'busy' if st == 'working' else 'idle'}\\r")

client = {"bytes": 0}
if with_client:
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(f"{R}/proj")
        os.execve(REC, [REC, "attach", "m"], ENV)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 50, 200, 0, 0))

    def drain():
        while True:
            try:
                data = os.read(fd, 65536)
            except OSError:
                return
            if not data:
                return
            client["bytes"] += len(data)
            if b"\x1b[c" in data or b"\x1b[0c" in data:
                pass

    threading.Thread(target=drain, daemon=True).start()
    time.sleep(1)
    os.write(fd, b"\x1b[?62;c")  # a DA1 answer, so the probe ends at once
time.sleep(4)  # settle
first = stats()
names = [p["member"] for p in first["panes"]]
dash_name = "Tableau de bord"
pp = pids()


def read_of(s, name):
    for p in s["panes"]:
        if p["member"] == name:
            return p["read"]
    return 0


samples = []
prev_t, prev_dash, prev_jour = time.time(), read_of(first, dash_name), read_of(first, "Journal")
prev_cpu = {k: cpu(v) for k, v in pp.items()}
prev_client = client["bytes"]
t0, c0 = prev_t, dict(prev_cpu)
dash0, jour0, cl0 = prev_dash, prev_jour, prev_client
end = t0 + secs
while time.time() < end:
    time.sleep(STEP)
    s = stats()
    t = time.time()
    row = {"dt": t - prev_t, "dash": read_of(s, dash_name) - prev_dash, "journal": read_of(s, "Journal") - prev_jour,
           "client": client["bytes"] - prev_client}
    cur = {k: cpu(v) for k, v in pp.items()}
    for k in pp:
        row[k] = (cur[k] - prev_cpu[k]) if cur[k] is not None and prev_cpu[k] is not None else None
    samples.append(row)
    prev_t, prev_dash, prev_jour, prev_cpu, prev_client = t, read_of(s, dash_name), read_of(s, "Journal"), cur, client["bytes"]

total = prev_t - t0
print(f"# {n} members, {st}, layout {layout}, client {'yes' if with_client else 'no'}, {total:.0f} s, pids {pp}")
print(f"load: {os.getloadavg()}")
print("interval  dash B  journal B  client B  cpu dash/journal/server/client (s)")
for r in samples:
    print(f"{r['dt']:6.1f}  {int(r["dash"]):7d}  {int(r["journal"]):8d}  {int(r["client"]):8d}  "
          + " / ".join(f"{(r.get(k) or 0):.2f}" for k in ("dashboard", "journal", "server", "client")))
tot = lambda k: sum((r.get(k) or 0) for r in samples)
print(f"TOTAL dashboard {read_of(s, dash_name) - dash0} B = {(read_of(s, dash_name) - dash0) / total:.0f} B/s, "
      f"max interval {max(r['dash'] for r in samples)} B; journal {read_of(s, 'Journal') - jour0} B; "
      f"client {client['bytes'] - cl0} B = {(client['bytes'] - cl0) / total:.0f} B/s")
print("CPU % of one core: " + ", ".join(f"{k} {tot(k) / total * 100:.2f}" for k in ("dashboard", "journal", "server", "client") if k in pp))
if with_client:
    try:
        os.kill(pid, signal.SIGHUP)
    except ProcessLookupError:
        pass
run("stop", "m", cwd=f"{R}/proj")
time.sleep(2)
left = subprocess.run(["pgrep", "-f", R], capture_output=True, text=True).stdout.split()
print("left after stop:", left)
subprocess.run(["rm", "-rf", "--", R])
