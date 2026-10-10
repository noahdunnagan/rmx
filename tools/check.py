"""End-to-end checks: a private Rex server, rmx in a pty, assertions on both
the screen and what Rex itself reports.

    python3 tools/check.py          # uses target/debug/rmx (RMX_BIN overrides)
"""
import base64, json, os, shutil, subprocess, sys, tempfile, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rig import Rig

REX = os.environ.get("REX_BIN", "/Applications/Rex Beta.app/Contents/Helpers/Rex Server.app/Contents/MacOS/rex")


class Server:
    def __init__(self):
        self.dir = tempfile.mkdtemp(prefix="rmx-check-")
        self.sock = os.path.join(self.dir, "server.sock")
        os.makedirs(os.path.join(self.dir, "state"))
        open(os.path.join(self.dir, "init.lua"), "w").close()
        self.proc = subprocess.Popen(
            [REX, "server", "run", "--listen", "unix://" + self.sock, "--state-dir", os.path.join(self.dir, "state"), "--config", os.path.join(self.dir, "init.lua")],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(50):
            if os.path.exists(self.sock):
                break
            time.sleep(0.1)

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
        self.proc.terminate()
        self.proc.wait(5)
        shutil.rmtree(self.dir, ignore_errors=True)


passed, failed = [], []


def check(name, cond, detail=""):
    (passed if cond else failed).append(name)
    print(("ok   " if cond else "FAIL ") + name + ("" if cond else f"\n     {detail}"))


def main():
    s = Server()
    try:
        bash = ["--shell", "none", "--", "/bin/bash", "--norc", "--noprofile", "-i"]
        s.rex("new", "alpha", *bash)
        s.rex("window", "new", "-s", "alpha", "two", *bash)
        s.rex("new", "beta", *bash)
        time.sleep(0.5)

        # ---- wide -----------------------------------------------------
        home = os.path.join(s.dir, "home")
        r = Rig(s.sock, 140, 30, home=home)
        t = r.text()
        check("rail lists every session", all(n in t for n in ("alpha", "beta", "+ session")), t)
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
        check("^B x asks first", "close renamed-tab?" in r.text(), r.text())
        r.send(b"y", 1.0)
        check("confirm closes the tab", "renamed-tab" not in s.rex("window", "ls", "-s", "alpha"), s.rex("window", "ls", "-s", "alpha"))
        check("rmx survives closing the tab it showed", r.alive() and "alpha" in r.pump(0.5).text(), r.text())

        # status: a background tab goes blocked
        msg = base64.b64encode(b"approve the plan").decode()
        s.rex("send", "-s", "beta", f"printf '\\033]7501;state=blocked:kind=question:app=ck:msg={msg}\\033\\\\'\r")
        r.pump(1.0)
        t = r.text()
        check("blocked status shows in the rail", "approve the plan" in t, t)
        check("footer counts tabs that need you", "● 1" in t.splitlines()[-1], t.splitlines()[-1])
        r.send(b"\x02o", 1.0)
        check("^B o jumps to it", "beta ›" in r.text().splitlines()[-1], r.text().splitlines()[-1])

        # scrollback
        r.send("for i in $(seq 1 200); do echo row-$i; done\r", 1.2)
        r.wheel(60, 10, up=True, n=10, settle=0.4)
        t = r.text()
        check("wheel scrolls back", "scrolled" in t.splitlines()[-1] and "row-150" in t, t)
        r.send(b"q", 0.3)
        check("q returns to the bottom", "scrolled" not in r.text().splitlines()[-1], r.text().splitlines()[-1])

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
        r.quit()
        time.sleep(0.3)
        check("quitting releases the size", s.size("beta", block)["owner"] is None, s.size("beta", block))

        # ---- phone ----------------------------------------------------
        r = Rig(s.sock, 50, 32, home=home)
        t = r.text()
        check("phone: no rail, full-width block", "+ session" not in t and "☰" in t.splitlines()[-1], t)
        r.send(b"\x02s", 0.5)
        t = r.text()
        check("phone: ^B s opens the switcher", "+ session" in t and "alpha" in t, t)
        x, y = r.find("alpha")
        r.tap(6, y + 1, 1.0)
        check("phone: tapping a tab opens it", "alpha ›" in r.text().splitlines()[-1] and "+ session" not in r.text(), r.text())
        fx = r.screen.display[-1].find("^B")
        r.tap(fx, r.rows - 1, 0.5)
        check("phone: tapping ^B opens the menu", "new session" in r.text(), r.text())
        mx, my = r.find("new session")
        before = s.rex("ls")
        r.tap(mx, my, 1.5)
        after = s.rex("ls")
        check("phone: tapping a menu entry runs it", after.count("\n") == before.count("\n") + 1, before + "\n" + after)
        r.send("echo from-phone\r", 0.8)
        check("phone: typing in the new session", "from-phone" in r.text(), r.text())
        status = r.quit()
        check("phone: ^B d exits cleanly", status == 0, status)
    finally:
        s.stop()
    print(f"\n{len(passed)} passed, {len(failed)} failed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
