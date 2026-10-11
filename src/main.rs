//! rmx: drive Rex sessions from any terminal, built for a phone over ssh.
//! It is a Rex client like the app: it reads sessions and tabs from the Rex
//! server, mirrors one block at a time, and types into it.
mod app;
mod base64;
mod input;
mod model;
mod render;
mod rex;
mod snapshot;
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
    /// From control connection N.
    Event(u64, serde_json::Value),
    ControlGone(u64, String),
    Data(u64, Vec<u8>),
    DataGone(u64),
}

const USAGE: &str = "rmx — drive Rex from any terminal

usage: rmx [--server PATH] [--snapshot | --restore]

  --snapshot   write down every open window and tab (a launchd timer runs this)
  --restore    reopen them after Rex restarted, resuming each ck session

  ^B s   tabs        ^B c   new tab       ^B S   new session
  ^B n/p next/prev   ^B o   needs you     ^B [   scroll back
  ^B r   rename tab  ^B x   close tab     ^B d   detach
  ^B ^B  send ^B     tap the footer for buttons and the full menu

The server is REX_SERVER (unix://...) or Rex's own, which rmx starts if
it is not running.";

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn main() {
    let mut server = std::env::var("REX_SERVER").ok();
    let mut job = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-S" | "--server" => server = args.next(),
            "--snapshot" => job = Some(false),
            "--restore" => job = Some(true),
            "-V" | "--version" => return println!("rmx {}", env!("CARGO_PKG_VERSION")),
            _ => return println!("{USAGE}"),
        }
    }
    let manage_server = server.is_none();
    let socket = socket_path(server);
    let done = match job {
        Some(restore) => snapshot::run(restore, &socket, manage_server).map(|line| println!("{line}")),
        None => run(socket, manage_server),
    };
    if let Err(e) = done {
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

/// Connect to Rex. When rmx manages Rex's own server and it is not running
/// (the Rex app is closed, say), start it first.
pub fn connect(socket: &std::path::Path, manage_server: bool, generation: u64, tx: mpsc::Sender<Msg>) -> Result<rex::Control, String> {
    let first = rex::Control::connect(socket, generation, tx.clone());
    let down = |e: &std::io::Error| matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused);
    match first {
        Ok(c) => return Ok(c),
        Err(e) if !(manage_server && down(&e)) => return Err(format!("can't reach the Rex server at {} ({e})", socket.display())),
        Err(_) => {}
    }
    let bin = rex::find_rex().ok_or("Rex isn't running and rmx can't find the Rex app to start it")?;
    rex::start_server(&bin).map_err(|e| format!("couldn't start Rex's server: {e}"))?;
    let mut last = String::new();
    for _ in 0..50 {
        match rex::Control::connect(socket, generation, tx.clone()) {
            Ok(c) => return Ok(c),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!("started Rex's server but can't reach it ({last})"))
}

/// This Mac's short name, for the phone's header.
pub fn host() -> &'static str {
    static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HOST.get_or_init(|| {
        let mut buf = [0u8; 256];
        unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
        let name = String::from_utf8_lossy(&buf[..buf.iter().position(|b| *b == 0).unwrap_or(0)]).into_owned();
        name.split('.').next().unwrap_or("this Mac").to_string()
    })
}

pub fn home() -> PathBuf {
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

fn run(socket: PathBuf, manage_server: bool) -> Result<(), String> {
    let (cols, rows) = window_size().ok_or("rmx needs a terminal")?;
    let (tx, rx) = mpsc::channel();
    let rex = connect(&socket, manage_server, 0, tx.clone())?;
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
    let mut app = App::new(rex, socket, manage_server, tx, model, cols, rows);
    app.home(load_last());
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
                Msg::Event(generation, ev) if generation == app.control_gen => {
                    app.event(&ev);
                    shape_changed = true;
                }
                Msg::Event(..) => {}
                Msg::ControlGone(generation, e) if generation == app.control_gen => {
                    farewell = Some(format!("lost the Rex server ({e})"));
                    app.lost.get_or_insert_with(Instant::now);
                    app.view = None;
                }
                Msg::ControlGone(..) => {}
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
        // The server went away (restart, Rex update, Rex quit): try to get
        // it back for a while before giving up.
        if let Some(since) = app.lost {
            if app.reconnect().is_ok() {
                farewell = None;
            } else if since.elapsed() > Duration::from_secs(30) {
                break;
            }
        } else if shape_changed {
            app.settle();
        }
        if let Some(v) = app.view.as_mut() {
            v.resync(&app.rex);
        }
        app.mirror_modes();
        if let Some((c, r)) = window_size().filter(|s| *s != (app.cols, app.rows)) {
            app.resized(c, r);
        }
        if working && spun.elapsed() >= Duration::from_millis(100) {
            app.tick += 1;
            spun = Instant::now();
        }
        let list_height = app.rows.saturating_sub(app.bar_h());
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
