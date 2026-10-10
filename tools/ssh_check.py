"""The phone path for real: rmx through `ssh -tt` to this Mac's Tailscale
address at phone size, against a private Rex server.

    python3 tools/ssh_check.py [user@host]     # default: $USER@<tailscale ip>
"""
import json, os, signal, subprocess, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rig import Rig
from check import Server, check, passed, failed


def tailscale_ip():
    out = subprocess.run(["/Applications/Tailscale.app/Contents/MacOS/Tailscale", "ip", "-4"], capture_output=True, text=True).stdout
    return out.split()[0]


def main():
    host = sys.argv[1] if len(sys.argv) > 1 else f"{os.environ['USER']}@{tailscale_ip()}"
    s = Server()
    try:
        s.rex("new", "work", "--shell", "none", "--", "/bin/bash", "--norc", "--noprofile", "-i")
        time.sleep(0.4)
        argv = ["ssh", "-tt", "-o", "BatchMode=yes", host, "rmx", "--server", s.sock]
        r = Rig(s.sock, 46, 28, argv=argv)
        r.pump(1.5)
        t = r.text()
        check(f"ssh {host}: rmx comes up on the tab list", "work" in t and "+ New window" in t, t)
        x, y = r.find("work")
        r.tap(6, y + 1, 1.2)
        check("tapping the tab opens it over ssh", "‹ Tabs" in "".join(r.screen.display[-3:]), r.text())
        r.send("echo over-ssh-$((2+3))\r", 1.0)
        check("typing over ssh reaches the block", "over-ssh-5" in s.rex("capture", "-s", "work"), s.rex("capture", "-s", "work")[-200:])
        block = s.block("work")
        size = s.size("work", block)
        check("block sized to the phone pane", (size["columns"], size["rows"]) == (46, 26), size)
        r.send(b"\x02c", 1.5)
        check("^B c over ssh makes a tab", s.rex("window", "ls", "-s", "work").count("\n") == 3, s.rex("window", "ls", "-s", "work"))
        r.resize(110, 30).pump(1.0)
        t = r.text()
        check("rotating to landscape brings the rail back", "+ New window" in t, t)
        # Drop the connection the way a phone losing signal does: kill ssh.
        rmx_pids = subprocess.run(["pgrep", "-f", f"rmx --server {s.sock}"], capture_output=True, text=True).stdout.split()
        os.kill(r.pid, signal.SIGKILL)
        os.waitpid(r.pid, 0)
        time.sleep(1.5)
        alive = [p for p in rmx_pids if subprocess.run(["kill", "-0", p], capture_output=True).returncode == 0]
        check("dropped ssh: rmx exits", rmx_pids and not alive, f"pids {rmx_pids} alive {alive}")
        sizes = [s.size("work", b["block_id"]) for b in json.loads(s.rex("block", "ls", "-s", "work", "--json"))["blocks"]]
        check("dropped ssh: size handed back", all(z["owner"] is None for z in sizes), sizes)
    finally:
        s.stop()
    print(f"\n{len(passed)} passed, {len(failed)} failed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
