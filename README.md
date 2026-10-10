# rmx

Drive Rex from any terminal, made for a phone over ssh. rmx is a Rex client like the app: it lists every session and tab on the Mac's Rex server, shows one tab at a time, and types into it.

## From the phone (Termius)

1. Host: the Mac's Tailscale address (`tailscale ip -4`), your user and ssh key.
2. Optional: set the host's startup command to `rmx` so connecting drops you straight in.
3. Connect and run `rmx`.

On a phone rmx opens on the tab list. Tap a tab to open it, `‹ tabs` to go back, `+ tab` for a new tab next to the current one, `⋯ menu` for everything else. Closing Termius detaches; the tabs keep running.

A session is a group of tabs (what one Rex window shows). Most of the time you only need `+ tab`.

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
