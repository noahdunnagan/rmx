"""End-to-end checks: a private Rex server, rmx in a pty, assertions on both
the screen and what Rex itself reports.

    python3 tools/check.py          # uses target/debug/rmx (RMX_BIN overrides)
"""
import base64, json, os, re, shutil, subprocess, sys, tempfile, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rig import Rig

REX = os.environ.get("REX_BIN", "/Applications/Rex Beta.app/Contents/Helpers/Rex Server.app/Contents/MacOS/rex")


class Server:
    """A Rex server of our own. `stale=True` runs it from a copy of rex that
    is then deleted, like a server left running across a Rex update: it can
    no longer start shells."""

    def __init__(self, stale=False):
        self.dir = tempfile.mkdtemp(prefix="rmx-check-")
        self.sock = os.path.join(self.dir, "server.sock")
        os.makedirs(os.path.join(self.dir, "state"))
        open(os.path.join(self.dir, "init.lua"), "w").close()
        self.binary = REX
        if stale:
            # Copy the whole helper app: rex's signature is tied to its bundle.
            bundle = REX.split("/Contents/MacOS/")[0]
            old = os.path.join(self.dir, "Old Rex Server.app")
            subprocess.run(["cp", "-Rp", bundle, old], check=True)
            self.binary = os.path.join(old, "Contents/MacOS/rex")
        self.start()
        if stale:
            shutil.rmtree(old)

    def start(self):
        self.proc = subprocess.Popen(
            [self.binary, "server", "run", "--listen", "unix://" + self.sock, "--state-dir", os.path.join(self.dir, "state"), "--config", os.path.join(self.dir, "init.lua")],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(50):
            if os.path.exists(self.sock):
                break
            time.sleep(0.1)

    def kill(self):
        self.proc.terminate()
        self.proc.wait(5)
        if os.path.exists(self.sock):
            os.unlink(self.sock)

    def rex(self, *args):
        out = subprocess.run([REX, "-S", "unix://" + self.sock, "--autostart=false", *args], capture_output=True, text=True)
        return out.stdout

    def api(self, method, session, args=None):
        cmd = ["api", "call", "-s", session, method] + ([json.dumps(args)] if args is not None else [])
        out = self.rex(*cmd)
        return json.loads(out) if out.strip() else None

    def block(self, session):
        return json.loads(self.rex("block", "ls", "-s", session, "--json"))["blocks"][0]["block_id"]

    def size(self, session, block):
        return self.api("block.method", session, {"block_id": block, "method": "size"})

    def stop(self):
        self.kill()
        shutil.rmtree(self.dir, ignore_errors=True)


passed, failed = [], []


def check(name, cond, detail=""):
    (passed if cond else failed).append(name)
    print(("ok   " if cond else "FAIL ") + name + ("" if cond else f"\n     {detail}"))


def footer(r):
    """The bottom bar (up to three rows on a phone)."""
    return "\n".join(r.screen.display[-3:])


def tap_text(r, text, settle=1.0):
    pos = r.find(text)
    if pos is None:
        raise AssertionError(f"no {text!r} on screen:\n{r.text()}")
    return r.tap(pos[0] + 1, pos[1], settle)


def tap_bar(r, text, settle=1.0):
    """Tap a bar button by its label."""
    for y in range(r.rows - 1, r.rows - 4, -1):
        x = r.screen.display[y].find(text)
        if x >= 0:
            return r.tap(x + 1, y, settle)
    raise AssertionError(f"no {text!r} in the bar:\n{footer(r)}")


def main():
    s = Server()
    home = os.path.join(s.dir, "home")
    try:
        bash = ["--shell", "none", "--", "/bin/bash", "--norc", "--noprofile", "-i"]
        s.rex("new", "alpha", *bash)
        s.rex("window", "new", "-s", "alpha", "two", *bash)
        s.rex("new", "beta", *bash)
        time.sleep(0.5)

        # ---- wide -----------------------------------------------------
        r = Rig(s.sock, 140, 30, home=home)
        t = r.text()
        check("rail lists every session", all(n in t for n in ("alpha", "beta", "+ New window")), t)
        r.send("echo typed-$((6*7))\r", 0.8)
        r.wait_for("typed-42")
        captured = s.rex("capture", "-s", "alpha", "-w", "1") + s.rex("capture", "-s", "alpha", "-w", "2") + s.rex("capture", "-s", "beta")
        check("typing reaches the block", "typed-42" in captured, captured[-300:])

        active_before = [l for l in s.rex("window", "ls", "-s", "alpha").splitlines() if l.rstrip().endswith("*")]
        r.send(b"\x02c", 1.2)
        windows = s.rex("window", "ls", "-s", "alpha")
        active_after = [l for l in windows.splitlines() if l.rstrip().endswith("*")]
        check("^B c makes a tab", windows.count("\n") == 4, windows)
        check("new tab leaves the Rex app's active tab alone", active_before == active_after, windows)

        r.send(b"\x02r", 0.3).send("renamed-tab\r", 0.8)
        check("^B r renames the tab in Rex", "renamed-tab" in s.rex("window", "ls", "-s", "alpha"), s.rex("window", "ls", "-s", "alpha"))
        check("rename shows in the rail", "renamed-tab" in r.pump(0.3).text(), r.text())

        r.send(b"\x02x", 0.3)
        check("^B x asks first", "Close “renamed-tab”?" in r.text() and "Close tab" in footer(r), r.text())
        r.send(b"y", 1.0)
        check("confirm closes the tab", "renamed-tab" not in s.rex("window", "ls", "-s", "alpha"), s.rex("window", "ls", "-s", "alpha"))
        check("rmx survives closing the tab it showed", r.alive() and "alpha" in r.pump(0.5).text(), r.text())

        # status: a background tab goes blocked
        msg = base64.b64encode(b"approve the plan").decode()
        s.rex("send", "-s", "beta", f"printf '\\033]7501;state=blocked:kind=question:app=ck:msg={msg}\\033\\\\'\r")
        r.pump(1.0)
        t = r.text()
        check("blocked status shows in the rail", "approve the plan" in t, t)
        check("footer counts tabs that need you", "● 1" in footer(r), footer(r))
        r.send(b"\x02o", 1.0)
        check("^B o jumps to it", "beta ›" in footer(r), footer(r))

        # scrollback
        r.send("for i in $(seq 1 200); do echo row-$i; done\r", 1.2)
        r.wheel(60, 10, up=True, n=10, settle=0.4)
        t = r.text()
        check("wheel scrolls back", "lines up" in footer(r) and "row-150" in t, t)
        r.send(b"q", 0.3)
        check("q returns to the bottom", "lines up" not in footer(r), footer(r))

        # program modes reach the outer terminal
        r.raw = b""
        r.send("printf '\\033[?1h\\033[?2004h'\r", 0.8)
        check("app-cursor and bracketed paste are mirrored", b"\x1b[?1h" in r.raw and b"\x1b[?2004h" in r.raw, repr(r.raw[-200:]))
        r.send("printf '\\033[?1l\\033[?2004l'\r", 0.6)

        # size ownership
        block = s.block("beta")
        size = s.size("beta", block)
        check("viewing takes the block's size", (size["columns"], size["rows"]) == (110, 29) and size["owner"], size)
        r.resize(120, 26).pump(0.8)
        size = s.size("beta", block)
        check("a terminal resize follows through", (size["columns"], size["rows"]) == (90, 25), size)
        s.api("block.method", "beta", {"block_id": block, "method": "resize", "args": {"columns": 130, "rows": 40, "mode": "take"}})
        r.pump(0.8)
        check("another client taking the size is flagged", "type to take it" in footer(r), footer(r))
        r.send("echo mine\r", 0.8)
        size = s.size("beta", block)
        check("typing takes the size back", (size["columns"], size["rows"]) == (90, 25), size)
        r.quit()
        time.sleep(0.3)
        check("quitting releases the size", s.size("beta", block)["owner"] is None, s.size("beta", block))

        # ---- phone ----------------------------------------------------
        r = Rig(s.sock, 46, 32, home=home)
        t = r.text()
        check("phone: starts on the tab list", "+ New window" in t and "Rex on" in t and "‹ Tabs" not in footer(r), t)
        check("phone: viewing nothing until a tab is picked", s.size("beta", block)["owner"] is None, s.size("beta", block))
        x, y = r.find("alpha")
        r.tap(6, y + 3, 1.0)
        check("phone: tapping a tab card (even its blank line) opens it", "‹ Tabs" in footer(r) and "+ New window" not in r.text(), r.text())
        check("phone: the bar is three rows tall", s.size("alpha", s.block("alpha"))["rows"] == 29, s.size("alpha", s.block("alpha")))
        tap_bar(r, "‹ Tabs", 0.5)
        check("phone: ‹ Tabs goes back to the list", "+ New window" in r.text() and "Exit" in footer(r), r.text())
        tap_bar(r, "‹ ", 0.5)
        check("phone: the back button returns to the tab", "‹ Tabs" in footer(r), footer(r))
        tap_bar(r, "⋯", 0.5)
        check("phone: ⋯ opens the menu", "New window" in r.text() and "its own window on the Mac" in r.text() and "Cancel" in footer(r), r.text())
        tap_bar(r, "Cancel", 0.5)
        check("phone: Cancel closes the menu", "its own window" not in r.text() and "‹ Tabs" in footer(r), r.text())
        tap_bar(r, "⋯", 0.5)
        before = s.rex("ls")
        x, y = r.find("New window")
        r.tap(x, y + 1, 1.5)
        after = s.rex("ls")
        check("phone: tapping a menu slot (even its blank line) runs it", after.count("\n") == before.count("\n") + 1, before + "\n" + after)
        r.send("echo from-phone\r", 0.8)
        check("phone: typing in the new window", "from-phone" in r.text(), r.text())
        tap_bar(r, "+ Tab", 1.5)
        check("phone: + Tab adds a tab next to this one", "‹ Tabs" in footer(r) and r.alive(), r.text())
        r.send("for i in $(seq 1 120); do echo row-$i; done\r", 1.2)
        tap_bar(r, "⋯", 0.5)
        tap_text(r, "Scroll back", 0.5)
        lines_up = lambda: int(re.search(r"(\d+) lines up", footer(r)).group(1))
        first = lines_up()
        tap_bar(r, "▲ Up", 0.5)
        check("phone: Scroll back, then ▲ Up goes further", lines_up() > first > 0, footer(r))
        tap_bar(r, "Done", 0.5)
        check("phone: Done returns to the bottom", "lines up" not in footer(r) and "‹ Tabs" in footer(r), footer(r))
        r.resize(46, 16).pump(0.8)
        check("phone: keyboard up shrinks the bar to one row", "‹ Tabs" in r.screen.display[-1] and "‹ Tabs" not in r.screen.display[-2], footer(r))
        r.resize(46, 32).pump(0.5)

        # The server goes away and comes back (a restart, a Rex update).
        s.kill()
        r.pump(1.0)
        check("server gone: rmx says so and waits", "Reconnecting" in footer(r) and r.alive(), footer(r))
        s.start()
        s.rex("new", "gamma", *bash)
        r.pump(2.5)
        check("server back: rmx reconnects to the list (a restart ends the old tabs)", "Reconnecting" not in footer(r) and "gamma" in r.text(), r.text())
        status = r.quit()
        check("phone: ^B d exits cleanly", status == 0, status)
    finally:
        s.stop()

    # A server left behind by a Rex update can't start shells; say why.
    s = Server(stale=True)
    try:
        r = Rig(s.sock, 46, 30, home=os.path.join(s.dir, "home"))
        tap_text(r, "+ New tab", 1.5)
        t = r.text()
        check("stale server: the reason is spelled out", "older than the Rex app" in t and "Old Rex Server" not in t, t)
        r.send(b"x", 0.3)
        check("stale server: the message clears on the next key", "older than" not in r.text(), r.text())
        r.quit()
    finally:
        s.stop()
    print(f"\n{len(passed)} passed, {len(failed)} failed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
