//! What rmx draws: the list of sessions and tabs (a rail when the terminal
//! is wide, the whole screen on a phone), the tab itself, a footer of word
//! buttons, a message banner, and the menu.
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::point_to_viewport;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use unicode_width::UnicodeWidthStr;

use crate::app::{Action, App, Confirm, MenuItem, Mode, Prompt, Row, MENU, RAIL_W, WIDE};
use crate::model::{Model, State, Status};
use crate::render::{Cell, Chrome, Frame, Style};

const PAD: usize = 2;
const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// What a tap on a footer button does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tap {
    Run(Action),
    Menu,
    Back,
    Bottom,
    Yes,
    No,
    Quit,
}

fn accent() -> Color {
    Color::Named(NamedColor::Cyan)
}

fn dim() -> Style {
    Style::default().dim()
}

pub fn compose(app: &App) -> Frame {
    let mut f = Frame::new(app.cols, app.rows);
    f.chrome = Chrome::from(app.bg);
    let body = app.rows.saturating_sub(1);
    if app.wide() {
        draw_list(app, &mut f, RAIL_W, body);
        draw_pane(app, &mut f);
    } else if matches!(app.mode, Mode::Switch { .. }) || app.view.is_none() {
        draw_list(app, &mut f, app.cols, body);
    } else {
        draw_pane(app, &mut f);
    }
    if matches!(app.mode, Mode::Prefix) {
        draw_menu(app, &mut f);
    } else {
        draw_banner(app, &mut f, body);
    }
    draw_footer(app, &mut f, body);
    f
}

// ---- the list ---------------------------------------------------------

fn glyph(app: &App, status: Option<&Status>) -> (String, Style) {
    match status.map(|s| s.state) {
        Some(State::Working) => (SPIN[app.tick % SPIN.len()].into(), Style::fg(NamedColor::Blue)),
        Some(State::Blocked) => ("●".into(), Style::fg(NamedColor::Yellow).bold()),
        Some(State::Done) => ("●".into(), Style::fg(NamedColor::Green)),
        Some(State::Error) => ("●".into(), Style::fg(NamedColor::Red)),
        Some(State::Idle) | None => ("○".into(), dim()),
    }
}

fn short_cwd(cwd: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let cwd = match cwd.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => cwd.to_string(),
    };
    let parts: Vec<&str> = cwd.split('/').collect();
    if parts.len() > 3 {
        format!("…/{}", parts[parts.len() - 2..].join("/"))
    } else {
        cwd
    }
}

/// The tab's second line: what its program says, else what is running where.
fn detail(model: &Model, session: &str, tab: &str) -> (String, Style) {
    let Some(t) = model.tab(session, tab) else { return (String::new(), dim()) };
    if let Some(st) = model.tab_status(t) {
        let (fallback, style) = match st.state {
            State::Blocked => ("needs you", Style::fg(NamedColor::Yellow)),
            State::Done => ("done", Style::fg(NamedColor::Green).dim()),
            State::Error => ("failed", Style::fg(NamedColor::Red)),
            State::Working => ("working", dim()),
            State::Idle => ("idle", dim()),
        };
        let text = if st.msg.is_empty() { fallback.to_string() } else { st.msg.clone() };
        return (text, style);
    }
    let b = Model::tab_block(t).and_then(|b| model.blocks.get(&b));
    match b {
        Some(b) if !b.cwd.is_empty() => (format!("{} · {}", b.process, short_cwd(&b.cwd)), dim()),
        Some(b) => (b.process.clone(), dim()),
        None => (String::new(), dim()),
    }
}

fn draw_list(app: &App, f: &mut Frame, width: usize, height: usize) {
    let inner = if app.wide() { width - 1 } else { width };
    let chrome = f.chrome;
    if chrome.is_none() && app.wide() {
        for y in 0..height {
            f.put(inner, y, Cell { c: '│', zw: None, style: dim() });
        }
    }
    let rows = app.rows();
    let sel = match app.mode {
        Mode::Switch { sel } => Some(sel),
        _ => None,
    };
    let current = app.view.as_ref().map(|v| (v.session.clone(), v.tab.clone(), v.block.clone()));
    let mut tint_selected = Vec::new();
    let mut tint_current = Vec::new();
    let add = Style::color(accent());
    for (idx, row) in rows.iter().enumerate().skip(app.list_offset).take(height) {
        let y = idx - app.list_offset;
        if sel == Some(idx) {
            tint_selected.push(y);
            f.text(0, y, 1, "›", Style::color(accent()).bold());
        }
        let max = inner.saturating_sub(PAD + 1);
        match row {
            Row::Session(s) => {
                let Some(session) = app.model.session(s) else { continue };
                let here = current.as_ref().is_some_and(|c| c.0 == *s);
                let style = if here { Style::color(accent()).bold() } else { Style::default().bold() };
                let w = f.text(PAD, y, max, &session.label, style);
                let n = session.tabs.len();
                let count = format!(" · {n} tab{}", if n == 1 { "" } else { "s" });
                f.text(PAD + w, y, max.saturating_sub(w), &count, dim());
            }
            Row::Tab(s, t) => {
                let Some(tab) = app.model.tab(s, t) else { continue };
                let here = current.as_ref().is_some_and(|c| c.0 == *s && c.1 == *t);
                if here {
                    f.text(PAD, y, 1, "▎", Style::color(accent()));
                    tint_current.push(y);
                }
                let status = app.model.tab_status(tab);
                let (g, gs) = glyph(app, status);
                f.text(PAD + 2, y, 1, &g, gs);
                let chip = status.map(|s| s.app.clone()).unwrap_or_default();
                let cw = chip.width().min(12);
                let name_w = inner.saturating_sub(PAD + 5 + if cw > 0 { cw + 1 } else { 0 });
                let style = if here { Style::color(accent()).bold() } else { Style::default() };
                f.text(PAD + 4, y, name_w, &app.model.tab_name(tab), style);
                f.text(inner.saturating_sub(cw + 1), y, cw, &chip, dim());
            }
            Row::Detail(s, t) => {
                if current.as_ref().is_some_and(|c| c.0 == *s && c.1 == *t) {
                    tint_current.push(y);
                }
                let (text, style) = detail(&app.model, s, t);
                f.text(PAD + 4, y, inner.saturating_sub(PAD + 5), &text, style);
            }
            Row::Block(_, _, b) => {
                let here = current.as_ref().is_some_and(|c| c.2 == *b);
                let info = app.model.blocks.get(b);
                let name = info.map(|i| if i.title.trim().is_empty() { i.process.clone() } else { i.title.trim().to_string() }).unwrap_or_default();
                let style = if here { Style::color(accent()) } else { dim() };
                f.text(PAD + 4, y, 2, "├ ", dim());
                f.text(PAD + 6, y, inner.saturating_sub(PAD + 7), &name, style);
            }
            Row::AddTab(_) => {
                f.text(PAD + 4, y, max, "+ new tab", add);
            }
            Row::AddSession => {
                let label = if app.model.sessions.is_empty() { "+ start a session" } else { "+ new session" };
                f.text(PAD, y, max, label, add.bold());
            }
            Row::Empty => {
                f.text(PAD, y, max, "No Rex sessions yet.", Style::default().bold());
            }
            Row::Blank => {}
        }
    }
    match chrome {
        Some(ch) => {
            for y in &tint_selected {
                f.tint(0, *y, width, 1, ch.selected);
            }
            for y in &tint_current {
                f.tint(0, *y, width, 1, ch.hover);
            }
            f.tint(0, 0, width, height, ch.surface);
        }
        None => {
            for y in tint_selected {
                for x in 0..inner {
                    let i = y * f.cols + x;
                    f.cells[i].style.flags ^= Flags::INVERSE;
                }
            }
        }
    }
}

// ---- the block --------------------------------------------------------

fn draw_pane(app: &App, f: &mut Frame) {
    let (x0, width, height) = app.pane_rect();
    let Some(v) = &app.view else {
        let text = if app.model.sessions.is_empty() { "No Rex sessions yet. Tap + tab to start one." } else { "No tab open. Pick one on the left." };
        f.text(x0 + PAD, 1, width.saturating_sub(PAD), text, dim());
        return;
    };
    let term = &v.term;
    let content = term.renderable_content();
    let colors = content.colors;
    let offset = content.display_offset;
    // A block taller than the pane shows its bottom, where prompts live.
    let shift = term.screen_lines().saturating_sub(height);
    let cols = term.columns();
    for indexed in content.display_iter {
        let Some(vp) = point_to_viewport(offset, indexed.point) else { continue };
        let (Some(row), col) = (vp.line.checked_sub(shift), vp.column.0) else { continue };
        if row >= height || col >= width.min(cols) {
            continue;
        }
        let cell = indexed.cell;
        let flags = cell.flags & (Flags::INVERSE | Flags::BOLD | Flags::ITALIC | Flags::ALL_UNDERLINES | Flags::DIM | Flags::HIDDEN | Flags::STRIKEOUT | Flags::WIDE_CHAR | Flags::WIDE_CHAR_SPACER);
        let resolve = |c: Color| match c {
            Color::Named(n) if (n as usize) < 16 => colors[n as usize].map(Color::Spec).unwrap_or(c),
            Color::Named(n @ (NamedColor::Foreground | NamedColor::Background)) => colors[n].map(Color::Spec).unwrap_or(c),
            Color::Indexed(i) => colors[i as usize].map(Color::Spec).unwrap_or(c),
            c => c,
        };
        let c = if cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) { ' ' } else { cell.c };
        let zw = cell.zerowidth().filter(|z| !z.is_empty()).map(|z| z.to_vec().into_boxed_slice());
        f.put(x0 + col, row, Cell { c, zw, style: Style { fg: resolve(cell.fg), bg: resolve(cell.bg), flags } });
    }
    let cursor = content.cursor;
    if cursor.shape != CursorShape::Hidden && matches!(app.mode, Mode::Normal | Mode::Prefix) {
        if let Some(vp) = point_to_viewport(offset, cursor.point) {
            if let Some(row) = vp.line.checked_sub(shift).filter(|r| *r < height && vp.column.0 < width) {
                f.cursor = Some((x0 + vp.column.0, row));
                f.cursor_shape = cursor.shape;
                f.cursor_blink = term.cursor_style().blinking;
            }
        }
    }
}

// ---- the banner -------------------------------------------------------

/// Word-wrap `text` to `width`.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let line = lines.last_mut().unwrap();
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            lines.push(word.to_string());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    lines
}

/// The latest message, right above the footer, until the next key or tap.
fn draw_banner(app: &App, f: &mut Frame, footer: usize) {
    let Some(note) = &app.note else { return };
    let lines: Vec<String> = wrap(&note.text, app.cols.saturating_sub(PAD * 2)).into_iter().take(5).collect();
    let top = footer.saturating_sub(lines.len() + 2);
    let h = footer - top;
    f.fill(0, top, app.cols, h, Style::default());
    let style = if note.error { Style::fg(NamedColor::Red).bold() } else { Style::default() };
    for (i, line) in lines.iter().enumerate() {
        f.text(PAD, top + 1 + i, app.cols.saturating_sub(PAD), line, style);
    }
    if let Some(ch) = f.chrome {
        f.tint(0, top, app.cols, h, ch.selected);
    }
    f.cursor = None;
}

// ---- the menu ---------------------------------------------------------

fn menu_items(app: &App) -> Vec<&'static MenuItem> {
    MENU.iter().filter(|m| (!m.needs_tab || app.view.is_some()) && (m.action != Action::ToggleRail || app.cols >= WIDE)).collect()
}

/// (x, y, w, h, row height) of the menu sheet: bottom of the screen, above
/// the footer. Rows get a blank line under them when there is room, so they
/// are easy to tap.
fn menu_rect(app: &App) -> (usize, usize, usize, usize, usize) {
    let (px, pcols, prows) = app.pane_rect();
    let n = menu_items(app).len();
    let row_h = if n * 2 + 2 <= prows { 2 } else { 1 };
    let h = (n * row_h + 2).min(prows);
    let (x, w) = if app.wide() { (px, pcols.min(52)) } else { (0, app.cols) };
    (x, prows - h, w, h, row_h)
}

fn draw_menu(app: &App, f: &mut Frame) {
    let (x, y, w, h, row_h) = menu_rect(app);
    f.fill(x, y, w, h, Style::default());
    f.text(x + PAD, y, w.saturating_sub(PAD), "menu · or ^B then the key", dim());
    let label_w = 15;
    for (i, item) in menu_items(app).iter().enumerate() {
        let row = y + 2 + i * row_h - (row_h - 1).min(1);
        if row >= y + h {
            break;
        }
        f.text(x + PAD, row, label_w, item.label, Style::default().bold());
        let hint_w = w.saturating_sub(PAD + label_w + 5);
        f.text(x + PAD + label_w, row, hint_w, item.hint, dim());
        f.text(x + w.saturating_sub(3), row, 1, item.key, Style::color(accent()));
    }
    match f.chrome {
        Some(ch) => f.tint(x, y, w, h, ch.selected),
        None => {
            for col in x..x + w {
                f.put(col, y, Cell { c: '─', zw: None, style: dim() });
            }
            f.text(x + PAD, y, w.saturating_sub(PAD), " menu · or ^B then the key ", dim());
        }
    }
    f.cursor = None;
}

pub fn menu_hit(app: &App, x: usize, y: usize) -> Option<Action> {
    let (mx, my, w, h, row_h) = menu_rect(app);
    if x < mx || x >= mx + w || y <= my || y >= my + h {
        return None;
    }
    let first = my + 2 - (row_h - 1).min(1);
    let i = y.checked_sub(first.saturating_sub(row_h - 1))? / row_h;
    menu_items(app).get(i).map(|m| m.action)
}

// ---- the footer -------------------------------------------------------

struct Button {
    text: String,
    tap: Tap,
    style: Style,
}

fn button(text: impl Into<String>, tap: Tap) -> Button {
    Button { text: text.into(), tap, style: Style::color(accent()).bold() }
}

/// The footer's parts: buttons on the left, a line of text, buttons on the
/// right.
fn footer_parts(app: &App) -> (Vec<Button>, String, Style, Vec<Button>) {
    let narrow = !app.wide();
    if app.lost.is_some() {
        return (vec![], "Lost Rex's server, reconnecting…".into(), Style::fg(NamedColor::Yellow), vec![button("quit", Tap::Quit)]);
    }
    let (status_text, status_style) = breadcrumb(app);
    match &app.mode {
        Mode::Confirm(Confirm::Close { name, .. }) => (
            vec![],
            format!("Close {name}?"),
            Style::default().bold(),
            vec![Button { style: Style::fg(NamedColor::Red).bold(), ..button("close", Tap::Yes) }, button("cancel", Tap::No)],
        ),
        Mode::Confirm(Confirm::Restart { tabs }) => (
            vec![],
            if *tabs == 1 { "Restart Rex? 1 tab closes".into() } else { format!("Restart Rex? {tabs} tabs close") },
            Style::default().bold(),
            vec![Button { style: Style::fg(NamedColor::Red).bold(), ..button("restart", Tap::Yes) }, button("cancel", Tap::No)],
        ),
        Mode::Prompt { kind, .. } => {
            let label = match kind {
                Prompt::RenameTab(..) => "tab name: ",
                Prompt::RenameSession(_) => "session name: ",
            };
            (vec![], label.into(), dim(), vec![button("save", Tap::Yes), button("cancel", Tap::No)])
        }
        Mode::Scroll => {
            let lines = app.view.as_ref().map(|v| v.term.grid().display_offset()).unwrap_or(0);
            (vec![], format!("↑ {lines} lines back"), Style::fg(NamedColor::Yellow), vec![button("bottom", Tap::Bottom)])
        }
        Mode::Switch { .. } if narrow => {
            let left = if app.view.is_some() { vec![button("‹ back", Tap::Back)] } else { vec![] };
            let hint = if app.view.is_some() { "" } else if app.model.sessions.is_empty() { "" } else { "tap a tab to open it" };
            (left, hint.into(), dim(), vec![button("⋯ menu", Tap::Menu)])
        }
        Mode::Switch { .. } => (vec![], "↑↓ enter · c new tab · x close · esc".into(), dim(), vec![button("⋯ menu", Tap::Menu)]),
        Mode::Normal | Mode::Prefix => {
            let left = if narrow { vec![button("‹ tabs", Tap::Run(Action::Switcher))] } else { vec![] };
            let mut right = Vec::new();
            let (waiting, blocked) = app.waiting();
            if waiting > 0 {
                let color = if blocked { NamedColor::Yellow } else { NamedColor::Green };
                right.push(Button { style: Style::fg(color).bold(), ..button(format!("● {waiting}"), Tap::Run(Action::Attention)) });
            }
            right.push(button("+ tab", Tap::Run(Action::NewTab)));
            let menu = if matches!(app.mode, Mode::Prefix) { Button { style: Style::color(accent()).bold().inverse(), ..button("⋯ menu", Tap::Menu) } } else { button("⋯ menu", Tap::Menu) };
            right.push(menu);
            (left, status_text, status_style, right)
        }
    }
}

/// Where each footer piece sits: buttons as (x, width, button), and the
/// text's (x, max width). Buttons carry a space of padding either side.
fn footer_layout(app: &App) -> (Vec<(usize, usize, Button)>, String, Style, (usize, usize)) {
    let (left, text, style, right) = footer_parts(app);
    let mut placed = Vec::new();
    let mut x = 0;
    for b in left {
        let w = b.text.width() + 2;
        placed.push((x, w, b));
        x += w + 1;
    }
    let text_x = x + 1;
    let mut end = app.cols;
    let mut rights = Vec::new();
    for b in right.into_iter().rev() {
        let w = b.text.width() + 2;
        let Some(start) = end.checked_sub(w).filter(|s| *s > text_x + 4) else { break };
        rights.push((start, w, b));
        end = start.saturating_sub(1);
    }
    placed.extend(rights.into_iter().rev());
    (placed, text, style, (text_x, end.saturating_sub(text_x + 1)))
}

pub fn footer_hit(app: &App, x: usize) -> Option<Tap> {
    let (buttons, _, _, (tx, tw)) = footer_layout(app);
    if let Some((.., b)) = buttons.into_iter().find(|(s, w, _)| x >= *s && x < s + w) {
        return Some(b.tap);
    }
    // The text is a button too: on a phone it leads to the list and back.
    let on_text = x >= tx && x < tx + tw;
    match app.mode {
        Mode::Normal if on_text && !app.wide() && app.lost.is_none() => Some(Tap::Run(Action::Switcher)),
        Mode::Switch { .. } if on_text && app.view.is_some() => Some(Tap::Back),
        Mode::Prefix => Some(Tap::Menu),
        _ => None,
    }
}

fn breadcrumb(app: &App) -> (String, Style) {
    let Some(v) = &app.view else { return ("rmx".into(), dim()) };
    let session = app.model.session(&v.session).map(|s| s.label.clone()).unwrap_or_default();
    let tab = app.model.tab(&v.session, &v.tab).map(|t| app.model.tab_name(t)).unwrap_or_default();
    if v.sized_by_other {
        return (format!("{tab} · Mac has the size, type to take it"), Style::fg(NamedColor::Yellow));
    }
    if app.wide() {
        return (format!("{session} › {tab}"), dim());
    }
    (tab, Style::default())
}

fn draw_footer(app: &App, f: &mut Frame, y: usize) {
    let (buttons, text, style, (tx, tw)) = footer_layout(app);
    let mut x = tx;
    if matches!(app.mode, Mode::Normal | Mode::Prefix) && app.lost.is_none() {
        let status = app.view.as_ref().and_then(|v| app.model.tab(&v.session, &v.tab)).and_then(|t| app.model.tab_status(t));
        let (g, gs) = glyph(app, status);
        f.text(x, y, 1, &g, gs);
        x += 2;
    }
    let w = f.text(x, y, (tx + tw).saturating_sub(x), &text, style);
    if let Mode::Prompt { buf, .. } = &app.mode {
        let bw = f.text(x + w, y, (tx + tw).saturating_sub(x + w), buf, Style::color(accent()).bold());
        f.cursor = Some((x + w + bw, y));
        f.cursor_shape = CursorShape::Beam;
    }
    for (bx, bw, b) in &buttons {
        f.text(bx + 1, y, bw - 2, &b.text, b.style);
    }
    if let Some(ch) = f.chrome {
        f.tint(0, y, app.cols, 1, ch.surface);
        for (bx, bw, _) in &buttons {
            f.tint(*bx, y, *bw, 1, ch.selected);
        }
    }
}
