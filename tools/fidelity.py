"""Does rmx's mirror match Rex's own screen? Runs real full-screen programs in a
private Rex server, views each through rmx, and diffs rmx's pane against
`rex capture` once the program settles.

    python3 tools/fidelity.py [program ...]    # default: all of PROGRAMS
"""
import os, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rig import Rig
from check import Server

PROGRAMS = {
    # wrapped lines a resize has to reflow, then an idle prompt
    "shell": ["/bin/bash", "--norc", "--noprofile", "-c",
              "for i in 1 2 3; do printf 'long-%d-' $i; printf 'x%.0s' {1..64}; echo; done; echo short; exec bash --norc --noprofile -i"],
    "claude": ["claude"],
    "top": ["top", "-s", "1"],
    "vim": ["vim", "-u", "NONE", "-N", "+set number", "/etc/services"],
    "less": ["less", "/etc/services"],
}


def pane_lines(rig, x0, rows):
    return [line[x0:].rstrip() for line in rig.screen.display[:rows]]


def screen_lines(server, session, rows):
    text = server.rex("capture", "-s", session)
    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    return [l.rstrip() for l in lines[-rows:]]


def compare(name, server, cols, rows, keys=b"", settle=4.0):
    session = f"fid-{name}"
    server.rex("new", session, "--shell", "none", "--", *PROGRAMS[name])
    time.sleep(0.3)
    home = os.path.join(server.dir, "home-" + name)
    os.makedirs(home, exist_ok=True)
    rig = Rig(server.sock, cols, rows, home=home)
    # open this program's tab: switcher, find it, enter
    rig.send(b"\x02s", 0.4)
    pos = rig.find(session)
    if pos:
        rig.tap(6, pos[1] + 1, 0.8)
    if keys:
        rig.send(keys, 1.0)
    rig.pump(settle)
    wide = cols >= 100
    x0, prows = (30 if wide else 0), rows - 1
    # top redraws every second; compare a frozen moment by capturing right after a pump
    mine = pane_lines(rig, x0, prows)
    theirs = screen_lines(server, session, prows)
    diffs = [(i, a, b) for i, (a, b) in enumerate(zip(mine, theirs)) if a != b]
    rig.quit()
    server.rex("kill", session)
    return mine, theirs, diffs


def main():
    names = sys.argv[1:] or list(PROGRAMS)
    s = Server()
    bad = 0
    try:
        for name in names:
            for cols, rows in [(140, 40), (50, 30)]:
                mine, theirs, diffs = compare(name, s, cols, rows)
                volatile = name == "top"
                ok = not diffs or (volatile and len(diffs) <= 3)
                bad += not ok
                print(f"{'ok  ' if ok else 'DIFF'} {name} {cols}x{rows}: {len(diffs)} differing lines")
                for i, a, b in diffs[:6]:
                    print(f"   line {i}\n     rmx: {a!r}\n     rex: {b!r}")
    finally:
        s.stop()
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
