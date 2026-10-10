//! The block on screen: a local mirror terminal fed by Rex's data stream.
//! Rex's own terminal is the real one; the mirror only draws, so anything it
//! would answer back (DSR, colour queries) is dropped.
use std::path::Path;
use std::sync::mpsc::Sender;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use serde_json::{json, Value};

use crate::rex::{Control, DataStream};
use crate::Msg;

const SCROLLBACK: usize = 10_000;
const APC: &[u8] = b"\x1b_rex1;";

#[derive(Clone)]
pub struct Sink;

impl EventListener for Sink {
    fn send_event(&self, _: Event) {}
}

pub struct Size {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct View {
    pub session: String,
    pub tab: String,
    pub block: String,
    pub generation: u64,
    pub term: Term<Sink>,
    parser: Processor,
    stream: DataStream,
    /// An in-band APC split across frames, held until it is whole.
    held: Vec<u8>,
    /// The size rmx asked Rex for. The mirror itself follows Rex's in-band
    /// resizes, so it changes size at the exact byte the real terminal did.
    pub want: (usize, usize),
    /// Who sized the block last, when it was not us.
    pub sized_by_other: bool,
}

impl View {
    #[allow(clippy::too_many_arguments)]
    pub fn open(rex: &Control, socket: &Path, session: &str, tab: &str, block: &str, size: (usize, usize), generation: u64, tx: Sender<Msg>) -> Result<View, String> {
        // Seed from Rex's own rendering (screen + scrollback + modes) at the
        // block's current size, then follow the live stream. The binary
        // snapshot on the stream would be gap-free, but it is Ghostty's
        // private format.
        let now = rex.block(session, block, "size", json!({}))?;
        let current = Size { cols: now["columns"].as_u64().unwrap_or(80) as usize, rows: now["rows"].as_u64().unwrap_or(24) as usize };
        let seed = rex.block(
            session,
            block,
            "format",
            json!({"format": "vt", "extra_modes": true, "extra_cursor": true, "extra_style": true, "extra_scrolling_region": true,
                   "extra_tabstops": true, "extra_charsets": true, "extra_keyboard": true, "extra_kitty_keyboard": true}),
        )?;
        let ticket = rex.call("data.connect", json!({"session_id": session, "block_id": block, "data_ref_id": "terminal", "subprotocol": "rex.data.terminal.v1"}))?;
        let ticket = ticket["ticket"].as_str().and_then(crate::base64::decode).ok_or("rex sent a bad data ticket")?;
        let stream = DataStream::open(socket, &ticket, generation, tx).map_err(|e| e.to_string())?;
        let config = Config { scrolling_history: SCROLLBACK, kitty_keyboard: true, ..Config::default() };
        let mut term = Term::new(config, &current, Sink);
        let mut parser = Processor::new();
        parser.advance(&mut term, seed["content"].as_str().unwrap_or_default().as_bytes());
        // Only now take the size: Rex marks the resize in the stream and the
        // program's redraw follows it, so neither can fall between the seed
        // and the stream.
        rex.post_block(session, block, "resize", json!({"columns": size.0, "rows": size.1, "mode": "take"}));
        Ok(View {
            session: session.into(),
            tab: tab.into(),
            block: block.into(),
            generation,
            term,
            parser,
            stream,
            held: Vec::new(),
            want: size,
            sized_by_other: false,
        })
    }

    /// Live output, with Rex's in-band messages applied where they sit.
    pub fn feed(&mut self, bytes: &[u8], me: &str) {
        let mut buf = std::mem::take(&mut self.held);
        buf.extend_from_slice(bytes);
        let mut rest = &buf[..];
        loop {
            let Some(start) = find(rest, APC) else {
                let keep = partial_suffix(rest, APC);
                self.parser.advance(&mut self.term, &rest[..rest.len() - keep]);
                self.held = rest[rest.len() - keep..].to_vec();
                return;
            };
            self.parser.advance(&mut self.term, &rest[..start]);
            let body = &rest[start + APC.len()..];
            let Some(end) = find(body, b"\x1b\\") else {
                // Rex's messages are small; anything this long is not one.
                if body.len() < 4096 {
                    self.held = rest[start..].to_vec();
                } else {
                    self.parser.advance(&mut self.term, &rest[start..]);
                }
                return;
            };
            self.message(&body[..end], me);
            rest = &body[end + 2..];
        }
    }

    /// `<token>;{json}`. Only `resize` matters to a viewer.
    fn message(&mut self, body: &[u8], me: &str) {
        let Some((token, payload)) = std::str::from_utf8(body).ok().and_then(|t| t.split_once(';')) else { return };
        if token.parse() != Ok(self.stream.token) {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else { return };
        if v["type"] != "resize" {
            return;
        }
        let (Some(cols), Some(rows)) = (v["cols"].as_u64(), v["rows"].as_u64()) else { return };
        self.term.resize(Size { cols: cols as usize, rows: rows as usize });
        self.sized_by_other = v["owner"].as_str().is_some_and(|o| o != me);
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// How many trailing bytes of `hay` could be the start of `needle`.
fn partial_suffix(hay: &[u8], needle: &[u8]) -> usize {
    (1..needle.len().min(hay.len() + 1)).rev().find(|&n| hay.ends_with(&needle[..n])).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_suffix_holds_only_possible_starts() {
        assert_eq!(partial_suffix(b"abc\x1b_re", APC), 4);
        assert_eq!(partial_suffix(b"abc\x1b", APC), 1);
        assert_eq!(partial_suffix(b"abc", APC), 0);
        assert_eq!(partial_suffix(b"", APC), 0);
    }
}
