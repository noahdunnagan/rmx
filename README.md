# rmx

Drive Rex from any terminal, made for a phone over ssh. rmx is a Rex client like the app: it lists every session and tab on the Mac's Rex server, shows one tab at a time, and types into it.

## From the phone (Termius)

1. Host: the Mac's Tailscale address (`tailscale ip -4`), your user and ssh key.
2. Optional: set the host's startup command to `rmx` so connecting drops you straight in.
3. Connect and run `rmx`.

On a phone rmx opens on the tab list. Tap a tab to open it, `‹ Tabs` to go back, `+ Tab` for a new tab next to the current one, `⋯` for the rest (scroll back, rename, close, new window, exit). Closing Termius detaches; the tabs keep running.

A window is what Rex shows on the Mac: one window of tabs (Rex calls it a session). Most of the time you only need `+ Tab`.

## Keys

`^B` then: `s` tabs, `c` new tab, `S` new window, `n`/`p` next/previous, `o` the next tab waiting on you, `[` scroll back, `r` rename tab, `x` close tab, `g` redraw, `d` detach. `^B ^B` sends a literal `^B`.

## Snapshot and restore

Rex loses every window and tab when its server restarts (an app update, a crash). `rmx --snapshot` writes down what is open to `~/.local/state/rmx/snapshot.json`: each window's name, each tab's name and folder, and the ck session name of every tab running ck. `rmx --watch` does it every 30 s; a launchd agent (`~/Library/LaunchAgents/rmx.snapshot.plist`, KeepAlive) runs that. Neither starts Rex's server. (launchd's `StartInterval` never fired on this Mac, hence the loop.)

After a restart, `rmx --restore` rebuilds them: a window per window, a shell in each folder with `ck --resume <name>` typed in. Running it again is safe, since open windows and running ck sessions are skipped. A restarted server comes back nearly empty, so the snapshot is not replaced until the new server has as many tabs as the old picture.

A split tab comes back as separate tabs.

## Notes

- While a tab is open in rmx it takes the phone's size; the Mac shows "Sized by another client" until you leave it. Typing in rmx takes the size back if the Mac grabbed it.
- If Rex's server isn't running, rmx starts it. If Rex updated and its old server can't start shells any more, rmx restarts it (asking first when tabs are open, since they close).

## Build and check

```sh
cargo build --release && rm -f ~/.cargo/bin/rmx && cp target/release/rmx ~/.cargo/bin/
python3 tools/check.py          # private Rex server, rmx in a pty
python3 tools/fidelity.py       # rmx's screen vs Rex's for shell, claude, top, vim, less
python3 tools/ssh_check.py      # through real ssh over Tailscale
python3 tools/m6_server_check.py  # server start/restart paths, on a spare Mac
```

The rig's taps include the pointer drift a real finger makes. On a real Mac, cua-driver's clicks carry no pointer motion, so Ghostty reports them at the last cell the pointer moved over; drag the right button onto the target first, then click.
