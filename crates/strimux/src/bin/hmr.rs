//! `strimux-hmr`: a minimal hot-reload host for developing strimux in-place.
//!
//! The host owns the session (raw mode, alternate screen, input loop, frame
//! buffer) and the loaded `strimux-core` cdylib. When the dylib changes on
//! disk it re-loads it and swaps the core in while the session keeps
//! running, so state (focus, layout) is never lost. This is the scaffold to
//! iterate on: put your real verb/render logic in `strimux-core`, keep the
//! PTY/grid ownership in the host.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode as TermCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size as term_size, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use libloading::Library;
use strimux_core_api::{Cell, Cmd, Frame, Key, KeyCode, SessionState, StrimuxCore, FACTORY};

fn dylib_path() -> PathBuf {
    let dir = env!("CARGO_MANIFEST_DIR");
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let name = if cfg!(target_os = "macos") {
        "libstrimux_core.dylib"
    } else if cfg!(target_os = "windows") {
        "strimux_core.dll"
    } else {
        "libstrimux_core.so"
    };
    Path::new(dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target")
        .join(profile)
        .join(name)
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Owns the loaded dylib and the live core. Reloads transparently when the
/// dylib's mtime changes, without touching host-owned session state.
struct Reloader {
    path: PathBuf,
    last: Option<SystemTime>,
    // Each load copies the dylib to a fresh unique file so macOS dlopen sees a
    // new inode and gives fresh code; a same-inode rewrite is otherwise cached.
    cache_dir: PathBuf,
    gen: u64,
    lib: Option<Library>,
    // The factory returns `Box<Box<dyn StrimuxCore>>` (double-boxed so the
    // thin pointer preserves the fat vtable). We keep the whole handle so we
    // never move the inner box across the boundary; access via refs.
    core: Option<Box<Box<dyn StrimuxCore>>>,
}

impl Reloader {
    fn new() -> Self {
        let path = dylib_path();
        let cache_dir = path.with_file_name("hmr");
        let _ = std::fs::create_dir_all(&cache_dir);
        let mut s = Reloader {
            path,
            last: None,
            cache_dir,
            gen: 0,
            lib: None,
            core: None,
        };
        s.load();
        s
    }

    fn load(&mut self) {
        match self.load_inner() {
            Ok(()) => {
                self.last = mtime(&self.path);
                tracing::info!(
                    label = self.core.as_ref().map(|c| c.label()).unwrap_or("?"),
                    "core loaded"
                );
            }
            Err(e) => {
                eprintln!("strimux-hmr: load core failed: {e}");
                eprintln!("  (run `cargo build -p strimux-core` first)");
            }
        }
    }

    fn load_inner(&mut self) -> Result<(), String> {
        unsafe {
            // Copy the freshly built dylib to a new unique file and dlopen the
            // copy. dlopen of an already-loaded same-inode path returns the
            // cached (old) image, so a fresh file guarantees fresh code.
            self.gen += 1;
            let copy = self.cache_dir.join(format!("core-{}.dylib", self.gen));
            std::fs::copy(&self.path, &copy).map_err(|e| e.to_string())?;
            let lib = Library::new(&copy).map_err(|e| e.to_string())?;
            let create: libloading::Symbol<unsafe extern "C" fn() -> *mut std::ffi::c_void> =
                lib.get(FACTORY).map_err(|e| e.to_string())?;
            let ptr = create();
            if ptr.is_null() {
                return Err("factory returned null".into());
            }
            // Recover the outer `Box<Box<dyn StrimuxCore>>` the factory stored.
            let holder: Box<Box<dyn StrimuxCore>> = Box::from_raw(ptr as *mut Box<dyn StrimuxCore>);
            self.lib = Some(lib);
            self.core = Some(holder);
        }
        Ok(())
    }

    fn core_mut(&mut self) -> &mut dyn StrimuxCore {
        self.core.as_mut().expect("core loaded").as_mut().as_mut()
    }

    /// If the dylib changed on disk, swap it in. Returns true if reloaded.
    /// The old library is kept alive until the old core box is dropped, so
    /// its vtable stays valid during teardown.
    fn poll_reload(&mut self) -> bool {
        let now = mtime(&self.path);
        if now.is_none() || now == self.last {
            return false;
        }
        eprintln!(
            "strimux-hmr: dylib changed {:?} -> {:?}, reloading",
            self.last, now
        );
        self.last = now;
        // Drop the old core while its library is still loaded, then drop the
        // library, then load the fresh one.
        let old_lib = self.lib.take();
        self.core.take();
        drop(old_lib);
        self.load();
        true
    }
}

fn decode(ke: &KeyEvent) -> Option<Key> {
    let code = match ke.code {
        TermCode::Char(c) => KeyCode::Char(c),
        TermCode::Enter => KeyCode::Enter,
        TermCode::Backspace => KeyCode::Backspace,
        TermCode::Tab => KeyCode::Tab,
        TermCode::Esc => KeyCode::Esc,
        TermCode::Left => KeyCode::Left,
        TermCode::Right => KeyCode::Right,
        TermCode::Up => KeyCode::Up,
        TermCode::Down => KeyCode::Down,
        TermCode::Home => KeyCode::Home,
        TermCode::End => KeyCode::End,
        TermCode::PageUp => KeyCode::PageUp,
        TermCode::PageDown => KeyCode::PageDown,
        TermCode::Delete => KeyCode::Delete,
        TermCode::Insert => KeyCode::Insert,
        _ => KeyCode::Other,
    };
    Some(Key {
        code,
        ctrl: ke.modifiers.contains(KeyModifiers::CONTROL),
        alt: ke.modifiers.contains(KeyModifiers::ALT),
        shift: ke.modifiers.contains(KeyModifiers::SHIFT),
    })
}

/// Paint a frame, diffing against the previous to avoid flicker.
fn paint(out: &mut Vec<u8>, frame: &Frame, last: &[Cell], cols: u16, rows: u16) -> bool {
    use crossterm::queue;
    use crossterm::style::{
        Attribute, Print, SetAttribute, SetBackgroundColor, SetForegroundColor,
    };
    use crossterm::terminal::Clear;
    use crossterm::terminal::ClearType;
    let cc = cols as usize;
    let mut dirty = false;
    for y in 0..rows as usize {
        let sl = &frame.cells[y * cc..(y + 1) * cc];
        let prev = last.get(y * cc..(y + 1) * cc);
        if prev == Some(sl) {
            continue;
        }
        dirty = true;
        let _ = queue!(
            out,
            cursor::MoveTo(0, y as u16),
            SetAttribute(Attribute::Reset)
        );
        let mut x = 0usize;
        while x < cc {
            let cell = sl[x];
            let mut run = String::new();
            run.push(cell.ch);
            let mut end = x + 1;
            while end < cc
                && sl[end].fg == cell.fg
                && sl[end].bg == cell.bg
                && sl[end].bold == cell.bold
                && sl[end].inverse == cell.inverse
            {
                run.push(sl[end].ch);
                end += 1;
            }
            let _ = queue!(
                out,
                SetForegroundColor(crossterm::style::Color::AnsiValue(cell.fg)),
                SetBackgroundColor(crossterm::style::Color::AnsiValue(cell.bg)),
            );
            if cell.bold {
                let _ = queue!(out, SetAttribute(Attribute::Bold));
            }
            if cell.inverse {
                let _ = queue!(out, SetAttribute(Attribute::Reverse));
            }
            let _ = queue!(out, Print(run));
            x = end;
        }
        let _ = queue!(out, Clear(ClearType::UntilNewLine));
    }
    dirty
}

fn main() -> std::io::Result<()> {
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(io::Error::other)?;
    if let Err(e) = execute!(stdout, EnterAlternateScreen, cursor::Hide) {
        let _ = disable_raw_mode();
        return Err(e);
    }

    let (cols, rows) = term_size().map_err(io::Error::other)?;
    let mut state = SessionState {
        focus: 0,
        cols: cols.max(1),
        rows: rows.max(2),
        frames: 0,
        panes: vec![
            "pane-1".into(),
            "pane-2".into(),
            "pane-3".into(),
            "pane-4".into(),
        ],
    };

    let mut reloader = Reloader::new();
    let mut frame: Frame;
    let mut last: Vec<Cell> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut dirty = true;

    'main: loop {
        // Hot reload check first each tick.
        if reloader.poll_reload() {
            dirty = true;
        }

        if event::poll(Duration::from_millis(7)).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(ke)) if ke.kind == KeyEventKind::Press => {
                    if let Some(key) = decode(&ke) {
                        match reloader.core_mut().handle_key(&mut state, key) {
                            Cmd::Quit => break 'main,
                            Cmd::Reload => {
                                reloader.poll_reload();
                                dirty = true;
                            }
                            Cmd::Input(_) | Cmd::Scroll(_) | Cmd::None => {}
                            Cmd::Repaint => {
                                dirty = true;
                            }
                        }
                    }
                }
                Ok(Event::Resize(c, r)) => {
                    state.cols = c.max(1);
                    state.rows = r.max(2);
                    dirty = true;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("input: {e}");
                }
            }
        }

        if dirty {
            frame = Frame::new(state.cols, state.rows);
            reloader.core_mut().render(&mut state, &mut frame);
            buf.clear();
            if paint(&mut buf, &frame, &last, state.cols, state.rows) {
                let _ = stdout.write_all(&buf);
                let _ = stdout.flush();
                last = frame.cells.clone();
            }
            dirty = false;
        }
    }

    // Teardown: drop the core while its library is loaded, then clean screen.
    let lib = reloader.lib.take();
    reloader.core.take();
    drop(lib);
    let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
    let _ = disable_raw_mode();
    Ok(())
}
