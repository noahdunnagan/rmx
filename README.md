# rmx

Drive Rex from any terminal, made for a phone over ssh. rmx is a Rex client like the app: it lists every session and tab on the Mac's Rex server, shows one tab at a time, and types into it.

## From the phone (Termius)

1. Host: the Mac's Tailscale address (`tailscale ip -4`), your user and ssh key.
2. Optional: set the host's startup command to `rmx` so connecting drops you straight in.
3. Connect and run `rmx`.

On a phone rmx opens on the tab list. Tap a tab to open it, `‹ Tabs` to go back, `+ Tab` for a new tab next to the current one, `⋯` for the rest (scroll back, rename, close, new window, exit). Closing Termius detaches; the tabs keep running.

A window is what Rex shows on the Mac: one window of tabs (Rex calls it a session). Most of the time you only need `+ Tab`.

## Keys

`^B` then: `s` tabs, `c` new tab, `S` new session, `n`/`p` next/previous, `o` the next tab waiting on you, `[` scroll back, `r` rename tab, `x` close tab, `g` redraw, `d` detach. `^B ^B` sends a literal `^B`.

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

```sh
```
