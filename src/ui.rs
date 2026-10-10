//! What rmx draws: the list of sessions and tabs (a rail when the terminal
//! is wide, a full-screen switcher when it is phone-narrow), the block, a
//! one-line footer, and the ^B menu.
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::point_to_viewport;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use unicode_width::UnicodeWidthStr;

use crate::app::{Action, App, Mode, Prompt, Row, MENU, RAIL_W};
use crate::model::{Model, State, Status};
use crate::render::{Cell, Chrome, Frame, Style};

const PAD: usize = 2;
const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
    let switching = matches!(app.mode, Mode::Switch { .. });
    if app.wide() {
        draw_list(app, &mut f, RAIL_W, body);
        draw_pane(app, &mut f);
    } else if switching {
        draw_list(app, &mut f, app.cols, body);
    } else {
        draw_pane(app, &mut f);
    }
    if matches!(app.mode, Mode::Prefix) {
        draw_menu(app, &mut f);
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
    for (idx, row) in rows.iter().enumerate().skip(app.list_offset).take(height) {
        let y = idx - app.list_offset;
        let selected = sel == Some(idx);
        if selected {
            tint_selected.push(y);
            f.text(0, y, 1, "›", Style::color(accent()).bold());
        }
        let max = inner.saturating_sub(PAD + 1);
        match row {
            Row::Session(s) => {
                let Some(session) = app.model.session(s) else { continue };
                let here = current.as_ref().is_some_and(|c| c.0 == *s);
                let style = if here { Style::color(accent()).bold() } else { Style::default().bold() };
                f.text(PAD, y, max, &session.label, style);
                let n = format!("{}", session.tabs.len());
                f.text(inner.saturating_sub(n.width() + 1), y, n.width(), &n, dim());
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
                f.text(PAD + 4, y, max, "+ tab", dim());
            }
            Row::AddSession => {
                f.text(PAD, y, max, "+ session", dim());
            }
            Row::Blank => {}
        }
    }
    if app.model.sessions.is_empty() && height > 2 {
        f.text(PAD, 0, inner.saturating_sub(PAD), "no Rex sessions yet", dim());
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
        let text = if app.model.sessions.is_empty() { "no Rex sessions yet · ^B S starts one" } else { "no tab open · ^B s picks one" };
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

// ---- the ^B menu ------------------------------------------------------

/// (x, y, w, h) of the menu panel: bottom of the pane, above the footer.
fn menu_rect(app: &App) -> (usize, usize, usize, usize) {
    let (px, pcols, prows) = app.pane_rect();
    let h = (MENU.len() + 1).min(prows);
    let w = if app.wide() { pcols.min(34) } else { app.cols };
    (if app.wide() { px } else { 0 }, prows - h, w, h)
}

fn draw_menu(app: &App, f: &mut Frame) {
    let (x, y, w, h) = menu_rect(app);
    f.fill(x, y, w, h, Style::default());
    f.text(x + PAD, y, w.saturating_sub(PAD), "rmx · tap or press a key", dim());
    for (i, (key, label, _)) in MENU.iter().take(h - 1).enumerate() {
        let row = y + 1 + i;
        f.text(x + PAD, row, 2, key, Style::color(accent()).bold());
        f.text(x + PAD + 4, row, w.saturating_sub(PAD + 5), label, Style::default());
    }
    match f.chrome {
        Some(ch) => f.tint(x, y, w, h, ch.selected),
        None => {
            for col in x..x + w {
                f.put(col, y, Cell { c: '─', zw: None, style: dim() });
            }
            f.text(x + PAD, y, w.saturating_sub(PAD), " rmx · tap or press a key ", dim());
        }
    }
    f.cursor = None;
}

pub fn menu_hit(app: &App, x: usize, y: usize) -> Option<Action> {
    let (mx, my, w, h) = menu_rect(app);
    if x < mx || x >= mx + w || y <= my || y >= my + h {
        return None;
    }
    MENU.get(y - my - 1).map(|m| m.2)
}

// ---- the footer -------------------------------------------------------

/// Footer buttons right to left: (text, action). None opens the ^B menu.
fn buttons(app: &App) -> Vec<(String, Option<Action>)> {
    let narrow = !app.wide();
    let mut b = vec![(" ^B ".to_string(), None), (if narrow { "+" } else { "+ tab" }.to_string(), Some(Action::NewTab)), (if narrow { "☰" } else { "tabs" }.to_string(), Some(Action::Switcher))];
    let (waiting, _) = app.waiting();
    if waiting > 0 {
        b.push((format!("● {waiting}"), Some(Action::Attention)));
    }
    b
}

/// Each button's span, right-aligned with a gap of two.
fn button_spans(app: &App) -> Vec<(usize, usize, String, Option<Action>)> {
    let mut right = app.cols.saturating_sub(1);
    let mut spans = Vec::new();
    for (text, action) in buttons(app) {
        let w = text.width();
        let Some(start) = right.checked_sub(w) else { break };
        spans.push((start, w, text, action));
        right = start.saturating_sub(2);
    }
    spans
}

/// What a tap on the footer at `x` does in normal mode: a button's action,
/// the switcher for the breadcrumb, None (the menu) for the ^B button.
pub fn footer_hit(app: &App, x: usize) -> Option<Action> {
    let spans = button_spans(app);
    match spans.iter().find(|(s, w, ..)| x >= *s && x < s + w) {
        Some((.., action)) => *action,
        None if spans.last().is_some_and(|(s, ..)| x < *s) => Some(Action::Switcher),
        None => None,
    }
}

fn confirm_spans(app: &App) -> [(usize, usize); 2] {
    let close = "[ close ]".width();
    let cancel = "[ cancel ]".width();
    let cancel_x = app.cols.saturating_sub(cancel + 1);
    [(cancel_x.saturating_sub(close + 2), close), (cancel_x, cancel)]
}

/// Some(true) on [ close ], Some(false) on [ cancel ].
pub fn confirm_hit(app: &App, x: usize) -> Option<bool> {
    let [(cx, cw), (nx, nw)] = confirm_spans(app);
    if x >= cx && x < cx + cw {
        Some(true)
    } else if x >= nx && x < nx + nw {
        Some(false)
    } else {
        None
    }
}

/// "session › tab", or just the tab when that is all that fits in `max`.
fn breadcrumb(app: &App, max: usize) -> (String, Style) {
    let Some(v) = &app.view else { return ("rmx".into(), dim()) };
    let session = app.model.session(&v.session).map(|s| s.label.clone()).unwrap_or_default();
    let tab = app.model.tab(&v.session, &v.tab).map(|t| app.model.tab_name(t)).unwrap_or_default();
    if v.sized_by_other {
        return (format!("{tab} · sized by another Rex client · ^B g takes it"), Style::fg(NamedColor::Yellow));
    }
    let full = format!("{session} › {tab}");
    (if full.width() <= max { full } else { tab }, dim())
}

fn draw_footer(app: &App, f: &mut Frame, y: usize) {
    let cols = app.cols;
    let note = app.note.as_ref().filter(|(_, at)| at.elapsed().as_secs() < 4).map(|(t, _)| t.clone());
    match &app.mode {
        Mode::Confirm { name, .. } => {
            let [(cx, cw), (nx, nw)] = confirm_spans(app);
            f.text(PAD, y, cx.saturating_sub(PAD + 1), &format!("close {name}?"), Style::default().bold());
            f.text(cx, y, cw, "[ close ]", Style::fg(NamedColor::Red).bold());
            f.text(nx, y, nw, "[ cancel ]", Style::default());
        }
        Mode::Prompt { kind, buf } => {
            let label = match kind {
                Prompt::RenameTab(..) => "rename tab › ",
                Prompt::RenameSession(_) => "rename session › ",
            };
            let x = PAD + f.text(PAD, y, cols.saturating_sub(PAD), label, dim());
            let w = f.text(x, y, cols.saturating_sub(x + 1), buf, Style::color(accent()).bold());
            f.cursor = Some((x + w, y));
            f.cursor_shape = CursorShape::Beam;
        }
        Mode::Scroll => {
            let lines = app.view.as_ref().map(|v| v.term.grid().display_offset()).unwrap_or(0);
            let text = if app.wide() { format!("scrolled {lines} lines · ↑↓ pgup pgdn g G · q or tap here returns") } else { format!("↑ {lines} · tap or q returns") };
            f.text(PAD, y, cols.saturating_sub(PAD + 1), &text, Style::fg(NamedColor::Yellow));
        }
        Mode::Switch { .. } => {
            let text = if app.wide() || cols >= 70 { "↑↓ pick · enter opens · c new tab · S new session · x close · r rename · esc" } else { "↑↓ enter · c tab · S session · x · esc" };
            f.text(PAD, y, cols.saturating_sub(PAD + 1), text, dim());
        }
        Mode::Normal | Mode::Prefix => {
            let spans = button_spans(app);
            let left_max = spans.last().map(|(s, ..)| s.saturating_sub(PAD + 2)).unwrap_or(cols);
            let (text, style) = match note {
                Some(n) => (n, Style::fg(NamedColor::Red)),
                None => breadcrumb(app, left_max.saturating_sub(2)),
            };
            let status = app.view.as_ref().and_then(|v| app.model.tab(&v.session, &v.tab)).and_then(|t| app.model.tab_status(t));
            let (g, gs) = glyph(app, status);
            f.text(PAD, y, 1, &g, gs);
            f.text(PAD + 2, y, left_max.saturating_sub(2), &text, style);
            let (_, blocked) = app.waiting();
            for (x, w, text, action) in spans {
                let style = match action {
                    None if matches!(app.mode, Mode::Prefix) => Style::color(accent()).bold().inverse(),
                    None => Style::color(accent()).bold(),
                    Some(Action::Attention) => Style::fg(if blocked { NamedColor::Yellow } else { NamedColor::Green }).bold(),
                    Some(_) => Style::default(),
                };
                f.text(x, y, w, &text, style);
            }
        }
    }
    if let Some(ch) = f.chrome {
        f.tint(0, y, cols, 1, ch.surface);
    }
}
