# rmx

Drive Rex from any terminal, made for a phone over ssh: it lists every Rex window and tab on the Mac, shows one, and types into it.

## Install

```sh
cargo install --path .
```

From the phone (Termius etc.), ssh into the Mac (Tailscale works) and run `rmx`, or make it the host's startup command.

`^B` then `s` tabs, `c` new tab, `S` new window, `n`/`p` next/previous, `o` next tab waiting on you, `[` scroll back, `r` rename, `x` close, `d` detach.

## Survive Rex restarts (optional)

A Rex server restart drops every tab. This keeps a snapshot every 30 s; afterwards `rmx --restore` reopens each window and folder and resumes its ck sessions.

```sh
cat > ~/Library/LaunchAgents/rmx.snapshot.plist <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>Label</key><string>rmx.snapshot</string>
  <key>ProgramArguments</key><array><string>$HOME/.cargo/bin/rmx</string><string>--watch</string></array>
  <key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
</dict></plist>
EOF
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/rmx.snapshot.plist
```

Checks: `python3 tools/check.py` (private Rex server, rmx in a pty).
