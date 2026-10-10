//! rmx: drive Rex sessions from any terminal, built for a phone over ssh.
//! It is a Rex client like the app: it reads sessions and tabs from the Rex
//! server, mirrors one block at a time, and types into it.
mod app;
mod base64;
mod input;
mod model;
mod render;
mod rex;
mod ui;
mod view;
mod ws;

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::app::{App, Mode};
use crate::model::{Model, State};
use crate::render::Frame;

pub enum Msg {
    Input(Vec<u8>),
    InputGone,
    Event(serde_json::Value),
    ControlGone(String),
    Data(u64, Vec<u8>),
    DataGone(u64),
}

const USAGE: &str = "rmx — drive Rex from any terminal

usage: rmx [--server PATH]

  ^B s   tabs        ^B c   new tab       ^B S   new session
  ^B n/p next/prev   ^B o   needs you     ^B [   scroll back
  ^B r   rename tab  ^B x   close tab     ^B d   detach
  ^B ^B  send ^B     tap the footer for buttons and the full menu

The server is REX_SERVER (unix://...) or Rex's default socket.";

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn main() {
    let mut server = std::env::var("REX_SERVER").ok();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-S" | "--server" => server = args.next(),
            "-V" | "--version" => return println!("rmx {}", env!("CARGO_PKG_VERSION")),
            _ => return println!("{USAGE}"),
        }
    }
    if let Err(e) = run(socket_path(server)) {
        eprintln!("rmx: {e}");
        std::process::exit(1);
    }
}

fn socket_path(server: Option<String>) -> PathBuf {
    match server {
        Some(s) => PathBuf::from(percent_decode(s.strip_prefix("unix://").unwrap_or(&s))),
        None => home().join("Library/Application Support/rex/server.sock"),
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

/// REX_SERVER is a URL, so "Application Support" arrives as "Application%20Support".
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], b.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok())) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn last_path() -> PathBuf {
    home().join(".local/state/rmx/last")
}

/// Remember what was on screen, so the next `rmx` reopens it.
pub fn save_last(session: &str, block: &str) {
    let path = last_path();
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(path, format!("{session} {block}\n"));
}

fn load_last() -> Option<(String, String)> {
    let text = std::fs::read_to_string(last_path()).ok()?;
    let (s, b) = text.trim().split_once(' ')?;
    Some((s.to_string(), b.to_string()))
}

fn window_size() -> Option<(usize, usize)> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } != 0 || ws.ws_col == 0 {
        return None;
    }
    Some((ws.ws_col as usize, ws.ws_row as usize))
}

/// The terminal in raw mode on the alternate screen, with mouse reports on.
struct Screen(libc::termios);

const ENTER: &str = "\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b]11;?\x1b\\";
const LEAVE: &str = "\x1b[?1000l\x1b[?1002l\x1b[?1006l\x1b[?2004l\x1b[?1004l\x1b[?1l\x1b>\x1b[=0;1u\x1b[0m\x1b[0 q\x1b[?25h\x1b[?1049l";

impl Screen {
    fn enter() -> std::io::Result<Screen> {
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut saved) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut raw = saved;
        unsafe {
            libc::cfmakeraw(&mut raw);
            libc::tcsetattr(0, libc::TCSANOW, &raw);
        }
        let saved_for_panic = saved;
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore(&saved_for_panic);
            default_hook(info);
        }));
        write_out(ENTER.as_bytes());
        Ok(Screen(saved))
    }
}

fn restore(saved: &libc::termios) {
    write_out(LEAVE.as_bytes());
    unsafe { libc::tcsetattr(0, libc::TCSANOW, saved) };
}

impl Drop for Screen {
    fn drop(&mut self) {
        restore(&self.0);
    }
}

fn write_out(bytes: &[u8]) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

fn run(socket: PathBuf) -> Result<(), String> {
    let (cols, rows) = window_size().ok_or("rmx needs a terminal")?;
    let (tx, rx) = mpsc::channel();
    let rex = rex::Control::connect(&socket, tx.clone()).map_err(|e| format!("cannot reach the Rex server at {} ({e}). Is Rex running?", socket.display()))?;
    let model = Model::load(&rex)?;
    unsafe {
        libc::signal(libc::SIGHUP, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let screen = Screen::enter().map_err(|e| e.to_string())?;
    let input_tx = tx.clone();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                let _ = input_tx.send(Msg::InputGone);
                return;
            }
            if input_tx.send(Msg::Input(buf[..n as usize].to_vec())).is_err() {
                return;
            }
        }
    });
    let mut app = App::new(rex, socket, tx, model, cols, rows);
    app.open_initial(load_last());
    let mut prev: Option<Frame> = None;
    let mut spun = Instant::now();
    let mut farewell = None;
    while !app.quit && !STOP.load(Ordering::SeqCst) {
        let working = app.model.blocks.values().flat_map(|b| b.status.values()).any(|s| s.state == State::Working);
        let wait = if working { Duration::from_millis(100) } else { Duration::from_millis(250) };
        let mut next = match rx.recv_timeout(wait) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        // Take everything already queued, but draw at least every 16 ms.
        let started = Instant::now();
        let mut shape_changed = false;
        while let Some(msg) = next.take() {
            match msg {
                Msg::Input(bytes) => app.input(&bytes),
                Msg::InputGone => app.quit = true,
                Msg::Event(ev) => {
                    app.event(&ev);
                    shape_changed = true;
                }
                Msg::ControlGone(e) => {
                    farewell = Some(format!("lost the Rex server ({e})"));
                    app.quit = true;
                }
                Msg::Data(generation, bytes) => app.data(generation, &bytes),
                Msg::DataGone(generation) => {
                    app.data_gone(generation);
                    shape_changed = true;
                }
            }
            if started.elapsed() < Duration::from_millis(16) {
                next = rx.try_recv().ok();
            }
        }
        if shape_changed {
            app.settle();
        }
        app.mirror_modes();
        if let Some((c, r)) = window_size().filter(|s| *s != (app.cols, app.rows)) {
            app.resized(c, r);
        }
        if working && spun.elapsed() >= Duration::from_millis(100) {
            app.tick += 1;
            spun = Instant::now();
        }
        let list_height = app.rows.saturating_sub(1);
        if app.wide() || matches!(app.mode, Mode::Switch { .. }) {
            app.fit_list(list_height);
        }
        let frame = ui::compose(&app);
        let mut out = std::mem::take(&mut app.out);
        if prev.as_ref().is_none_or(|p| !p.same(&frame)) {
            frame.diff(prev.as_ref(), &mut out);
            prev = Some(frame);
        }
        if !out.is_empty() {
            write_out(&out);
        }
    }
    app.leave();
    drop(screen);
    if let Some(f) = farewell {
        eprintln!("rmx: {f}");
    }
    Ok(())
}
