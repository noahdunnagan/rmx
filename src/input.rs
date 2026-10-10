//! Split raw terminal input into tokens. Most are forwarded to the block
//! verbatim (every token keeps its original bytes); only the prefix key,
//! mouse reports, terminal replies and keys pressed inside an rmx mode are
//! interpreted. (Lifted from Lasso.)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Tab,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Insert,
    Delete,
    FocusIn,
    FocusOut,
    /// Something rmx has no name for (function keys, media keys).
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPress {
    pub key: Key,
    pub mods: Mods,
}

impl KeyPress {
    fn plain(key: Key) -> Self {
        KeyPress { key, mods: Mods::default() }
    }
    pub fn ctrl(&self, c: char) -> bool {
        self.key == Key::Char(c) && self.mods == Mods { ctrl: true, shift: false, alt: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mouse {
    /// Raw SGR button code: 0/1/2 buttons, +4 shift, +8 meta, +16 ctrl,
    /// +32 motion, 64/65 wheel up/down.
    pub cb: u16,
    pub x: u16,
    pub y: u16,
    pub press: bool,
}

impl Mouse {
    pub fn wheel(&self) -> Option<i32> {
        match self.cb & !28 {
            64 => Some(-1),
            65 => Some(1),
            _ => None,
        }
    }
    pub fn is_motion(&self) -> bool {
        self.cb & 32 != 0
    }
    /// A left-button press. Not motion with the button held: a finger that
    /// drifts during a tap sends those, and each would count as a tap.
    pub fn is_left_click(&self) -> bool {
        self.press && self.cb & 0b1110_0011 == 0
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Token<'a> {
    Key(KeyPress, &'a [u8]),
    Mouse(Mouse, &'a [u8]),
    PasteStart(&'a [u8]),
    PasteEnd(&'a [u8]),
    /// A terminal reply (OSC), never forwarded.
    Osc(&'a [u8]),
}

impl Token<'_> {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Token::Key(_, b) | Token::Mouse(_, b) | Token::PasteStart(b) | Token::PasteEnd(b) | Token::Osc(b) => b,
        }
    }
}

pub fn tokenize(input: &[u8]) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < input.len() {
        let b = input[i];
        if b == 0x1b {
            let (token, len) = escape(&input[i..]);
            out.push(token);
            i += len;
            continue;
        }
        let (kp, len) = match b {
            0x0d => (KeyPress::plain(Key::Enter), 1),
            0x09 => (KeyPress::plain(Key::Tab), 1),
            0x7f | 0x08 => (KeyPress::plain(Key::Backspace), 1),
            0x01..=0x1a => (KeyPress { key: Key::Char((b + 0x60) as char), mods: Mods { ctrl: true, shift: false, alt: false } }, 1),
            0x00..=0x1f => (KeyPress::plain(Key::Other), 1),
            _ => {
                let len = utf8_len(b).min(input.len() - i);
                match std::str::from_utf8(&input[i..i + len]).ok().and_then(|s| s.chars().next()) {
                    Some(c) => (KeyPress::plain(Key::Char(c)), len),
                    None => (KeyPress::plain(Key::Other), 1),
                }
            }
        };
        out.push(Token::Key(kp, &input[i..i + len]));
        i += len;
    }
    out
}

fn utf8_len(first: u8) -> usize {
    match first {
        0xf0..=0xf7 => 4,
        0xe0..=0xef => 3,
        0xc0..=0xdf => 2,
        _ => 1,
    }
}

/// `input[0]` is ESC. Returns the token and how many bytes it consumed.
fn escape(input: &[u8]) -> (Token<'_>, usize) {
    if input.len() == 1 {
        return (Token::Key(KeyPress::plain(Key::Esc), input), 1);
    }
    match input[1] {
        b'[' => {
            let mut j = 2;
            while j < input.len() && (0x20..=0x3f).contains(&input[j]) {
                j += 1;
            }
            if j >= input.len() || !(0x40..=0x7e).contains(&input[j]) {
                // Truncated: hand the rest through untouched.
                return (Token::Key(KeyPress::plain(Key::Other), input), input.len());
            }
            let seq = &input[..=j];
            let body = &input[2..j];
            let fin = input[j];
            if body.first() == Some(&b'<') && (fin == b'M' || fin == b'm') {
                if let Some(m) = parse_mouse(&body[1..], fin == b'M') {
                    return (Token::Mouse(m, seq), j + 1);
                }
            }
            if fin == b'u' {
                return (Token::Key(csi_u(body), seq), j + 1);
            }
            if fin == b'~' {
                match body {
                    b"200" => return (Token::PasteStart(seq), j + 1),
                    b"201" => return (Token::PasteEnd(seq), j + 1),
                    _ => {}
                }
            }
            (Token::Key(csi_key(body, fin), seq), j + 1)
        }
        b']' => {
            // OSC reply from the terminal: runs to BEL or ST.
            let mut j = 2;
            while j < input.len() {
                if input[j] == 0x07 {
                    return (Token::Osc(&input[..=j]), j + 1);
                }
                if input[j] == 0x1b && input.get(j + 1) == Some(&b'\\') {
                    return (Token::Osc(&input[..j + 2]), j + 2);
                }
                j += 1;
            }
            (Token::Osc(input), input.len())
        }
        b'O' if input.len() >= 3 => {
            let key = match input[2] {
                b'A' => Key::Up,
                b'B' => Key::Down,
                b'C' => Key::Right,
                b'D' => Key::Left,
                b'H' => Key::Home,
                b'F' => Key::End,
                _ => Key::Other,
            };
            (Token::Key(KeyPress::plain(key), &input[..3]), 3)
        }
        b'\x1b' => (Token::Key(KeyPress::plain(Key::Esc), &input[..1]), 1),
        _ => {
            // Legacy alt chord: ESC + one (possibly multibyte) char.
            let len = 1 + utf8_len(input[1]).min(input.len() - 1);
            let key = std::str::from_utf8(&input[1..len]).ok().and_then(|s| s.chars().next()).map_or(Key::Other, Key::Char);
            (Token::Key(KeyPress { key, mods: Mods { alt: true, shift: false, ctrl: false } }, &input[..len]), len)
        }
    }
}

/// xterm-style modifier parameter: 1 + (shift 1, alt 2, ctrl 4).
fn mods_from(param: Option<&str>) -> Mods {
    let m = param.and_then(|p| p.split(':').next()).and_then(|p| p.parse::<u16>().ok()).unwrap_or(1).saturating_sub(1);
    Mods { shift: m & 1 != 0, alt: m & 2 != 0, ctrl: m & 4 != 0 }
}

/// Legacy CSI keys: arrows, home/end, and the `n;m~` family.
fn csi_key(body: &[u8], fin: u8) -> KeyPress {
    let text = std::str::from_utf8(body).unwrap_or("");
    let mut params = text.split(';');
    let first = params.next().unwrap_or("");
    let mods = mods_from(params.next());
    let key = match fin {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        b'I' if body.is_empty() => Key::FocusIn,
        b'O' if body.is_empty() => Key::FocusOut,
        b'Z' => return KeyPress { key: Key::Tab, mods: Mods { shift: true, alt: false, ctrl: false } },
        b'~' => match first {
            "1" | "7" => Key::Home,
            "4" | "8" => Key::End,
            "2" => Key::Insert,
            "3" => Key::Delete,
            "5" => Key::PageUp,
            "6" => Key::PageDown,
            _ => Key::Other,
        },
        _ => Key::Other,
    };
    KeyPress { key, mods }
}

/// Kitty `CSI code[:alt[:base]] ; mods[:event] u`.
fn csi_u(body: &[u8]) -> KeyPress {
    let text = std::str::from_utf8(body).unwrap_or("");
    let mut params = text.split(';');
    let code = params.next().and_then(|p| p.split(':').next()).and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
    let modp = params.next();
    let event = modp.and_then(|p| p.split(':').nth(1)).and_then(|e| e.parse::<u8>().ok()).unwrap_or(1);
    let mods = mods_from(modp);
    let key = if event == 3 {
        Key::Other // release: never wanted
    } else {
        match code {
            27 => Key::Esc,
            13 | 57414 => Key::Enter,
            9 => Key::Tab,
            127 => Key::Backspace,
            57417 => Key::Left,
            57418 => Key::Right,
            57419 => Key::Up,
            57420 => Key::Down,
            57421 => Key::PageUp,
            57422 => Key::PageDown,
            57423 => Key::Home,
            57424 => Key::End,
            57425 => Key::Insert,
            57426 => Key::Delete,
            57399..=57408 => Key::Char((b'0' + (code - 57399) as u8) as char),
            57409 => Key::Char('.'),
            57410 => Key::Char('/'),
            57411 => Key::Char('*'),
            57412 => Key::Char('-'),
            57413 => Key::Char('+'),
            57415 => Key::Char('='),
            57344..=63743 => Key::Other, // private use: modifiers, media, F13+
            _ => char::from_u32(code).map_or(Key::Other, Key::Char),
        }
    };
    KeyPress { key, mods }
}

fn parse_mouse(body: &[u8], press: bool) -> Option<Mouse> {
    let text = std::str::from_utf8(body).ok()?;
    let mut parts = text.split(';');
    let cb = parts.next()?.parse().ok()?;
    let x = parts.next()?.parse().ok()?;
    let y = parts.next()?.parse().ok()?;
    Some(Mouse { cb, x, y, press })
}

/// Encode a mouse event for a pane at the given cell position.
pub fn encode_mouse(m: Mouse, x: u16, y: u16, sgr: bool) -> Vec<u8> {
    if sgr {
        return format!("\x1b[<{};{};{}{}", m.cb, x, y, if m.press { 'M' } else { 'm' }).into_bytes();
    }
    let cb = if m.press { m.cb } else { 3 } as u32 + 32;
    let (x, y) = ((x as u32 + 32).min(255), (y as u32 + 32).min(255));
    vec![0x1b, b'[', b'M', cb as u8, x as u8, y as u8]
}
