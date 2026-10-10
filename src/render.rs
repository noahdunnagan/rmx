//! A frame is the whole screen as styled cells. rmx composes one per redraw,
//! diffs it against the previous one, and writes only the changed runs.
//! (Lifted from Lasso.)
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
}

impl Default for Style {
    fn default() -> Self {
        Style { fg: Color::Named(NamedColor::Foreground), bg: Color::Named(NamedColor::Background), flags: Flags::empty() }
    }
}

impl Style {
    pub fn color(color: Color) -> Self {
        Style { fg: color, ..Default::default() }
    }
    pub fn fg(color: NamedColor) -> Self {
        Style { fg: Color::Named(color), ..Default::default() }
    }
    pub fn bold(mut self) -> Self {
        self.flags |= Flags::BOLD;
        self
    }
    pub fn dim(mut self) -> Self {
        self.flags |= Flags::DIM;
        self
    }
    pub fn inverse(mut self) -> Self {
        self.flags |= Flags::INVERSE;
        self
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cell {
    pub c: char,
    /// Combining characters that ride on this cell. Rare, so boxed.
    pub zw: Option<Box<[char]>>,
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { c: ' ', zw: None, style: Style::default() }
    }
}

/// The chrome (rail, footer, panels) sits on a tint of the terminal's own
/// background, so it reads as one surface instead of text floating on black.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Chrome {
    pub surface: Color,
    pub hover: Color,
    pub selected: Color,
}

impl Chrome {
    /// Tint the terminal's own background: lighter on dark themes, darker on light.
    pub fn from(bg: Option<Rgb>) -> Option<Chrome> {
        let base = bg?;
        let dark = (base.r as u32 * 299 + base.g as u32 * 587 + base.b as u32 * 114) / 1000 < 128;
        let mix = |amt: f32| {
            let ch = |c: u8| (if dark { c as f32 + (255.0 - c as f32) * amt } else { c as f32 - c as f32 * amt }) as u8;
            Color::Spec(Rgb { r: ch(base.r), g: ch(base.g), b: ch(base.b) })
        };
        Some(Chrome { surface: mix(0.06), hover: mix(0.11), selected: mix(0.17) })
    }
}

pub struct Frame {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    pub cursor: Option<(usize, usize)>,
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
    /// Surface colours for the chrome, when the terminal told us its background.
    pub chrome: Option<Chrome>,
}

impl Frame {
    pub fn new(cols: usize, rows: usize) -> Self {
        Frame {
            cols,
            rows,
            cells: vec![Cell::default(); cols * rows],
            cursor: None,
            cursor_shape: CursorShape::Block,
            cursor_blink: false,
            chrome: None,
        }
    }

    pub fn same(&self, other: &Frame) -> bool {
        self.cols == other.cols && self.rows == other.rows && self.cursor == other.cursor && self.cursor_shape == other.cursor_shape && self.cells == other.cells
    }

    /// Give every still-default background in the rect this colour. Specific
    /// tints (selected, hover) go first; the surface tint last fills the rest.
    pub fn tint(&mut self, x: usize, y: usize, w: usize, h: usize, bg: Color) {
        for row in y..(y + h).min(self.rows) {
            for col in x..(x + w).min(self.cols) {
                let cell = &mut self.cells[row * self.cols + col];
                if cell.style.bg == Color::Named(NamedColor::Background) {
                    cell.style.bg = bg;
                }
            }
        }
    }

    pub fn put(&mut self, x: usize, y: usize, cell: Cell) {
        if x < self.cols && y < self.rows {
            self.cells[y * self.cols + x] = cell;
        }
    }

    /// Write text clipped to `max_w` columns. Returns the width used.
    pub fn text(&mut self, x: usize, y: usize, max_w: usize, s: &str, style: Style) -> usize {
        let mut used = 0;
        for c in s.chars() {
            let w = c.width().unwrap_or(0);
            if w == 0 {
                continue;
            }
            if used + w > max_w {
                break;
            }
            let mut flags = style.flags;
            if w == 2 {
                flags |= Flags::WIDE_CHAR;
            }
            self.put(x + used, y, Cell { c, zw: None, style: Style { flags, ..style } });
            if w == 2 {
                self.put(x + used + 1, y, Cell { c: ' ', zw: None, style: Style { flags: style.flags | Flags::WIDE_CHAR_SPACER, ..style } });
            }
            used += w;
        }
        used
    }

    pub fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, style: Style) {
        for yy in y..(y + h).min(self.rows) {
            for xx in x..(x + w).min(self.cols) {
                self.cells[yy * self.cols + xx] = Cell { c: ' ', zw: None, style };
            }
        }
    }

    /// Emit the ANSI that turns `prev` into `self` on the client's terminal.
    pub fn diff(&self, prev: Option<&Frame>, out: &mut Vec<u8>) {
        // No previous frame: paint every cell. Only a size change also clears,
        // since a full paint covers the screen and clearing first would flash.
        let resized = prev.is_some_and(|p| p.cols != self.cols || p.rows != self.rows);
        let full = prev.is_none() || resized;
        out.extend_from_slice(b"\x1b[?25l\x1b[0m");
        if resized {
            out.extend_from_slice(b"\x1b[H\x1b[2J");
        }
        let mut pen = Style::default();
        let mut pos: Option<(usize, usize)> = None;
        for y in 0..self.rows {
            let mut x = 0;
            while x < self.cols {
                let cell = &self.cells[y * self.cols + x];
                if cell.style.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    x += 1;
                    continue;
                }
                let wide = cell.style.flags.contains(Flags::WIDE_CHAR);
                let w = if wide { 2 } else { 1 };
                if !full {
                    let p = prev.unwrap();
                    let same = p.cells[y * self.cols + x] == *cell
                        && (!wide || x + 1 >= self.cols || p.cells[y * self.cols + x + 1] == self.cells[y * self.cols + x + 1]);
                    if same {
                        x += w;
                        continue;
                    }
                }
                if pos != Some((x, y)) {
                    out.extend_from_slice(format!("\x1b[{};{}H", y + 1, x + 1).as_bytes());
                }
                if cell.style != pen {
                    write_sgr(&cell.style, out);
                    pen = cell.style;
                }
                let mut buf = [0u8; 4];
                out.extend_from_slice(cell.c.encode_utf8(&mut buf).as_bytes());
                if let Some(zw) = &cell.zw {
                    for c in zw.iter() {
                        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    }
                }
                x += w;
                // A cell with combining marks (an emoji + VS16, say) may be
                // one column or two depending on the terminal; place the next
                // cell explicitly so the rest of the line cannot drift.
                pos = if x < self.cols && cell.zw.is_none() { Some((x, y)) } else { None };
            }
        }
        out.extend_from_slice(b"\x1b[0m");
        match self.cursor {
            Some((cx, cy)) if self.cursor_shape != CursorShape::Hidden => {
                let code = match (self.cursor_shape, self.cursor_blink) {
                    (CursorShape::Underline, true) => 3,
                    (CursorShape::Underline, false) => 4,
                    (CursorShape::Beam, true) => 5,
                    (CursorShape::Beam, false) => 6,
                    (_, true) => 1,
                    (_, false) => 2,
                };
                out.extend_from_slice(format!("\x1b[{};{}H\x1b[{} q\x1b[?25h", cy + 1, cx + 1, code).as_bytes());
            }
            _ => {}
        }
    }
}

fn write_sgr(style: &Style, out: &mut Vec<u8>) {
    let mut s = String::from("\x1b[0");
    let f = style.flags;
    if f.contains(Flags::BOLD) {
        s.push_str(";1");
    }
    if f.contains(Flags::DIM) {
        s.push_str(";2");
    }
    if f.contains(Flags::ITALIC) {
        s.push_str(";3");
    }
    if f.contains(Flags::UNDERLINE) {
        s.push_str(";4");
    } else if f.contains(Flags::DOUBLE_UNDERLINE) {
        s.push_str(";4:2");
    } else if f.contains(Flags::UNDERCURL) {
        s.push_str(";4:3");
    } else if f.contains(Flags::DOTTED_UNDERLINE) {
        s.push_str(";4:4");
    } else if f.contains(Flags::DASHED_UNDERLINE) {
        s.push_str(";4:5");
    }
    if f.contains(Flags::INVERSE) {
        s.push_str(";7");
    }
    if f.contains(Flags::HIDDEN) {
        s.push_str(";8");
    }
    if f.contains(Flags::STRIKEOUT) {
        s.push_str(";9");
    }
    color_sgr(style.fg, true, &mut s);
    color_sgr(style.bg, false, &mut s);
    s.push('m');
    out.extend_from_slice(s.as_bytes());
}

fn color_sgr(color: Color, fg: bool, s: &mut String) {
    use std::fmt::Write;
    let base = if fg { 30 } else { 40 };
    let bright = if fg { 90 } else { 100 };
    let ext = if fg { 38 } else { 48 };
    match color {
        Color::Named(n) => {
            let i = n as usize;
            match i {
                0..=7 => write!(s, ";{}", base + i).unwrap(),
                8..=15 => write!(s, ";{}", bright + i - 8).unwrap(),
                259..=266 => write!(s, ";2;{}", base + i - 259).unwrap(),
                268 => write!(s, ";2;{}", base + 9).unwrap(),
                _ => write!(s, ";{}", base + 9).unwrap(),
            }
        }
        Color::Indexed(i) => write!(s, ";{ext};5;{i}").unwrap(),
        Color::Spec(rgb) => write!(s, ";{ext};2;{};{};{}", rgb.r, rgb.g, rgb.b).unwrap(),
    }
}
