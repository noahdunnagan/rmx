"""Headless rig: run rmx in a pty against a Rex server, read the screen with pyte.

    python3 tools/rig.py SOCKET [COLS ROWS]   # prints the first screen

Import it for scripted checks: Rig(socket, cols, rows).send(b"...").wait_for("text").
"""
import os, pty, select, struct, fcntl, termios, time, sys

import pyte

HERE = os.path.dirname(os.path.abspath(__file__))
RMX = os.environ.get("RMX_BIN", os.path.join(HERE, "..", "target", "debug", "rmx"))


class Rig:
    def __init__(self, socket, cols=160, rows=45, bg="1414/1414/1414", home="/tmp/rmx-rig-home", argv=None):
        os.makedirs(home, exist_ok=True)
        self.cols, self.rows, self.bg = cols, rows, bg
        self.screen = pyte.Screen(cols, rows)
        self.stream = pyte.ByteStream(self.screen)
        self.raw = b""
        pid, fd = pty.fork()
        if pid == 0:
            os.environ["TERM"] = "xterm-256color"
            os.environ.pop("REX_SERVER", None)
            if argv is None:
                os.environ["HOME"] = home  # keep rmx's "last tab" file out of the real one
                argv = [RMX, "--server", socket]
            os.execvp(argv[0], argv)
        self.pid, self.fd = pid, fd
        self.resize(cols, rows)
        self.pump(0.6)

    def resize(self, cols, rows):
        self.cols, self.rows = cols, rows
        self.screen.resize(rows, cols)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        return self

    def pump(self, secs=0.3):
        end = time.time() + secs
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if not r:
                continue
            try:
                data = os.read(self.fd, 1 << 16)
            except OSError:
                break
            self.raw += data
            if self.bg and b"\x1b]11;?" in data:
                os.write(self.fd, b"\x1b]11;rgb:%s\x1b\\" % self.bg.encode())
            self.stream.feed(data)
        return self

    def send(self, data, settle=0.3):
        os.write(self.fd, data if isinstance(data, bytes) else data.encode())
        return self.pump(settle)

    def tap(self, x, y, settle=0.4):
        """Left click at 0-based cell (x, y), the way Termius sends a tap."""
        self.send(f"\x1b[<0;{x + 1};{y + 1}M".encode(), 0.05)
        return self.send(f"\x1b[<0;{x + 1};{y + 1}m".encode(), settle)

    def wheel(self, x, y, up=True, n=1, settle=0.3):
        for _ in range(n):
            self.send(f"\x1b[<{64 if up else 65};{x + 1};{y + 1}M".encode(), 0.02)
        return self.pump(settle)

    def text(self):
        return "\n".join(line.rstrip() for line in self.screen.display)

    def find(self, needle):
        for y, line in enumerate(self.screen.display):
            x = line.find(needle)
            if x >= 0:
                return x, y
        return None

    def wait_for(self, needle, timeout=5.0):
        end = time.time() + timeout
        while time.time() < end:
            if needle in self.text():
                return True
            self.pump(0.1)
        raise AssertionError(f"never saw {needle!r}:\n{self.text()}")

    def alive(self):
        return os.waitpid(self.pid, os.WNOHANG) == (0, 0)

    def quit(self):
        self.send(b"\x02d", 0.6)
        try:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
        except ChildProcessError:
            return 0
        if pid == 0:
            os.kill(self.pid, 9)
            os.waitpid(self.pid, 0)
            return None
        return status


if __name__ == "__main__":
    sock = sys.argv[1]
    cols, rows = (int(sys.argv[2]), int(sys.argv[3])) if len(sys.argv) > 3 else (160, 45)
    rig = Rig(sock, cols, rows)
    print(rig.text())
    rig.quit()
