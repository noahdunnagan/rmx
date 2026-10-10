"""The paths that touch Rex's own server, run on a spare Mac with Rex
installed (the M6): rmx over ssh starts a stopped server, and recovers a
server left behind by a Rex update. Destroys that Mac's Rex sessions.

    python3 tools/m6_server_check.py <ssh-host>
"""
import os, subprocess, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rig import Rig
from check import check, passed, failed, footer, tap_text, tap_bar

HOST = sys.argv[1]
REX = '"$HOME/Applications/Rex Beta.app/Contents/Helpers/Rex Server.app/Contents/MacOS/rex"'
OLD = "$HOME/scratch/Old Rex Server.app"
FLAGS = "--websocket-origin-pattern https://rex.superlogical.com --log-max-size 50MiB --log-format text"


def sh(cmd):
    return subprocess.run(["ssh", HOST, cmd], capture_output=True, text=True).stdout


def status():
    return sh(f"{REX} --autostart=false server status 2>&1")


def rmx():
    r = Rig("", 46, 30, argv=["ssh", "-tt", HOST, "~/.local/bin/rmx"])
    r.pump(3.0)
    return r


def stale_server(sessions):
    """Run the server from a copy of the helper app, then delete the copy."""
    sh(f"{REX} --autostart=false server stop; rm -rf \"{OLD}\"; mkdir -p ~/scratch; cp -Rp \"$HOME/Applications/Rex Beta.app/Contents/Helpers/Rex Server.app\" \"{OLD}\"")
    sh(f"\"{OLD}/Contents/MacOS/rex\" server start {FLAGS}")
    for name in sessions:
        sh(f"\"{OLD}/Contents/MacOS/rex\" --autostart=false new {name}")
    sh(f"rm -rf \"{OLD}\"")
    return status()


def main():
    # 1. Rex's server is not running at all.
    sh(f"{REX} --autostart=false server stop")
    check("setup: server stopped", "pid:" not in status(), status())
    r = rmx()
    t = r.text()
    check("server down: rmx starts it", "pid:" in status() and "No tabs open on the Mac yet." in t, t + "\n" + status())
    r.quit()

    # 2. A stale server with no tabs: restarted without asking.
    st = stale_server([])
    check("setup: stale server running from a deleted copy", "Old Rex Server" not in st and "pid:" in st, st)
    old_pid = [l for l in st.splitlines() if l.startswith("pid:")]
    r = rmx()
    tap_text(r, "+ New tab", 4.0)
    t = r.text()
    new = status()
    check("stale + empty: restarted and the session opened", "‹ Tabs" in footer(r) and [l for l in new.splitlines() if l.startswith("pid:")] != old_pid, t + "\n" + new)
    r.quit()

    # 3. A stale server with a tab open: asks first.
    stale_server(["keepme"])
    r = rmx()
    x, y = r.find("keepme")
    r.tap(6, y + 1, 1.5)
    tap_bar(r, "+ Tab", 1.5)
    t = r.text()
    check("stale + tabs: asks before restarting", "Restart Rex's server?" in t and "closes the 1 open tab." in " ".join(t.split()) and "Restart" in footer(r), t)
    tap_bar(r, "Cancel", 1.0)
    check("stale + tabs: cancel leaves the tab alone", "keepme" in sh(f"{REX} --autostart=false ls"), sh(f"{REX} --autostart=false ls"))
    tap_bar(r, "+ Tab", 1.5)
    tap_bar(r, "Restart", 5.0)
    t = r.text()
    check("stale + tabs: restart, then the new tab opens", "‹ Tabs" in footer(r) and "Restarted Rex's server." in t, t)
    print(sh(f"{REX} --autostart=false ls"))
    r.quit()

    print(f"\n{len(passed)} passed, {len(failed)} failed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
