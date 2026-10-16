//! A pty-backed terminal session: child shell + reader thread feeding a `vt100` parser.
//! Everything here is headless; the GUI paints `ScreenSnapshot`s.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

pub const SCROLLBACK_LINES: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub rows: u16,
    pub cols: u16,
}

impl Default for TermSize {
    fn default() -> Self {
        TermSize { rows: 24, cols: 80 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    Idx(u8),
    Rgb(u8, u8, u8),
}

impl From<vt100::Color> for Color {
    fn from(c: vt100::Color) -> Self {
        match c {
            vt100::Color::Default => Color::Default,
            vt100::Color::Idx(i) => Color::Idx(i),
            vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cell {
    /// Grapheme text; empty for a blank cell.
    pub text: String,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    /// Double-width glyph (occupies this cell and the next).
    pub wide: bool,
    /// Right half of a wide glyph: do not paint.
    pub continuation: bool,
}

/// A copy of the visible grid (honouring the scrollback offset) plus the modes the GUI needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenSnapshot {
    pub rows: u16,
    pub cols: u16,
    /// Row-major, `rows * cols` cells.
    pub cells: Vec<Cell>,
    /// (row, col) of the cursor.
    pub cursor: (u16, u16),
    pub cursor_visible: bool,
    pub title: String,
    /// Lines scrolled back from the live screen (0 = live).
    pub scrollback: usize,
    pub alt_screen: bool,
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    /// The program asked for mouse reports.
    pub mouse_reporting: bool,
}

impl ScreenSnapshot {
    pub fn cell(&self, row: u16, col: u16) -> Option<&Cell> {
        if row < self.rows && col < self.cols {
            self.cells
                .get(row as usize * self.cols as usize + col as usize)
        } else {
            None
        }
    }

    pub fn row_text(&self, row: u16) -> String {
        let mut s = String::new();
        for c in 0..self.cols {
            if let Some(cell) = self.cell(row, c) {
                if cell.continuation {
                    continue;
                }
                if cell.text.is_empty() {
                    s.push(' ');
                } else {
                    s.push_str(&cell.text);
                }
            }
        }
        s.trim_end().to_string()
    }

    /// Whole visible grid as text, one line per row.
    pub fn text(&self) -> String {
        (0..self.rows)
            .map(|r| self.row_text(r))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Text between two (row, col) positions inclusive, in reading order (either order given).
    pub fn text_range(&self, a: (u16, u16), b: (u16, u16)) -> String {
        let (s, e) = if a <= b { (a, b) } else { (b, a) };
        let mut out = String::new();
        for r in s.0..=e.0.min(self.rows.saturating_sub(1)) {
            let c0 = if r == s.0 { s.1 } else { 0 };
            let c1 = if r == e.0 {
                e.1
            } else {
                self.cols.saturating_sub(1)
            };
            let mut line = String::new();
            for c in c0..=c1.min(self.cols.saturating_sub(1)) {
                if let Some(cell) = self.cell(r, c) {
                    if cell.continuation {
                        continue;
                    }
                    if cell.text.is_empty() {
                        line.push(' ');
                    } else {
                        line.push_str(&cell.text);
                    }
                }
            }
            out.push_str(line.trim_end());
            if r != e.0 {
                out.push('\n');
            }
        }
        out
    }

    /// True when anything other than blanks is on screen.
    pub fn has_content(&self) -> bool {
        self.cells.iter().any(|c| !c.text.is_empty())
    }
}

#[derive(Default)]
struct Callbacks {
    title: String,
}

impl vt100::Callbacks for Callbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title).into_owned();
    }
}

type Repaint = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;

pub struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    parser: Arc<Mutex<vt100::Parser<Callbacks>>>,
    alive: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    repaint: Repaint,
    size: TermSize,
    cwd: std::path::PathBuf,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The user's login shell: `shell` if given, else `$SHELL`, else `/bin/sh` (`COMSPEC` on Windows).
pub fn default_shell(shell: Option<&str>) -> String {
    if let Some(s) = shell.filter(|s| !s.trim().is_empty()) {
        return s.to_string();
    }
    if cfg!(windows) {
        return std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
    }
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into())
}

impl TerminalSession {
    /// Interactive login shell in `cwd`.
    pub fn spawn(cwd: &Path, shell: Option<String>, size: TermSize) -> Result<Self, String> {
        let program = default_shell(shell.as_deref());
        let args: Vec<String> = if cfg!(windows) {
            vec![]
        } else {
            vec!["-l".into()]
        };
        Self::spawn_command(cwd, &program, &args, size)
    }

    /// Run an arbitrary program in a pty.
    pub fn spawn_command(
        cwd: &Path,
        program: &str,
        args: &[String],
        size: TermSize,
    ) -> Result<Self, String> {
        let size = TermSize {
            rows: size.rows.max(1),
            cols: size.cols.max(1),
        };
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        if cwd.is_dir() {
            cmd.cwd(cwd);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            size.rows,
            size.cols,
            SCROLLBACK_LINES,
            Callbacks::default(),
        )));
        let alive = Arc::new(AtomicBool::new(true));
        let generation = Arc::new(AtomicU64::new(0));
        let repaint: Repaint = Arc::new(Mutex::new(None));
        {
            let (parser, alive, generation, repaint) = (
                parser.clone(),
                alive.clone(),
                generation.clone(),
                repaint.clone(),
            );
            std::thread::Builder::new()
                .name("rteams-pty-reader".into())
                .spawn(move || {
                    let mut buf = [0u8; 8192];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                lock(&parser).process(&buf[..n]);
                                generation.fetch_add(1, Ordering::Release);
                                if let Some(f) = lock(&repaint).as_ref() {
                                    f();
                                }
                            }
                        }
                    }
                    alive.store(false, Ordering::Release);
                    generation.fetch_add(1, Ordering::Release);
                    if let Some(f) = lock(&repaint).as_ref() {
                        f();
                    }
                })
                .map_err(|e| e.to_string())?;
        }
        Ok(TerminalSession {
            master: pair.master,
            writer,
            child,
            parser,
            alive,
            generation,
            repaint,
            size,
            cwd: cwd.to_path_buf(),
        })
    }

    /// Called from the reader thread whenever new output arrives (e.g. `ctx.request_repaint()`).
    pub fn set_repaint(&self, f: impl Fn() + Send + Sync + 'static) {
        *lock(&self.repaint) = Some(Arc::new(f));
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if self.writer.write_all(bytes).is_ok() {
            let _ = self.writer.flush();
        }
        // Typing returns to the live screen.
        self.scroll_to_bottom();
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let size = TermSize {
            rows: rows.max(1),
            cols: cols.max(1),
        };
        if size == self.size {
            return;
        }
        self.size = size;
        let _ = self.master.resize(PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        lock(&self.parser)
            .screen_mut()
            .set_size(size.rows, size.cols);
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn size(&self) -> TermSize {
        self.size
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Scroll the view: positive = further back into history.
    pub fn scroll(&self, lines: isize) {
        let mut p = lock(&self.parser);
        let cur = p.screen().scrollback() as isize;
        p.screen_mut().set_scrollback((cur + lines).max(0) as usize);
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn scroll_to_bottom(&self) {
        lock(&self.parser).screen_mut().set_scrollback(0);
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Increments whenever the screen changed; lets callers skip re-snapshotting.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn is_alive(&mut self) -> bool {
        if self.alive.load(Ordering::Acquire) {
            return !matches!(self.child.try_wait(), Ok(Some(_)));
        }
        false
    }

    /// Lines of history above the live screen.
    pub fn history_len(&self) -> usize {
        let mut p = lock(&self.parser);
        let cur = p.screen().scrollback();
        p.screen_mut().set_scrollback(usize::MAX);
        let max = p.screen().scrollback();
        p.screen_mut().set_scrollback(cur);
        max
    }

    pub fn title(&self) -> String {
        lock(&self.parser).callbacks().title.clone()
    }

    pub fn screen(&self) -> ScreenSnapshot {
        let p = lock(&self.parser);
        let s = p.screen();
        let (rows, cols) = s.size();
        let mut cells = Vec::with_capacity(rows as usize * cols as usize);
        for r in 0..rows {
            for c in 0..cols {
                cells.push(match s.cell(r, c) {
                    Some(cell) => Cell {
                        text: cell.contents().to_string(),
                        fg: cell.fgcolor().into(),
                        bg: cell.bgcolor().into(),
                        bold: cell.bold(),
                        dim: cell.dim(),
                        italic: cell.italic(),
                        underline: cell.underline(),
                        inverse: cell.inverse(),
                        wide: cell.is_wide(),
                        continuation: cell.is_wide_continuation(),
                    },
                    None => Cell::default(),
                });
            }
        }
        ScreenSnapshot {
            rows,
            cols,
            cells,
            cursor: s.cursor_position(),
            cursor_visible: !s.hide_cursor() && s.scrollback() == 0,
            title: p.callbacks().title.clone(),
            scrollback: s.scrollback(),
            alt_screen: s.alternate_screen(),
            app_cursor: s.application_cursor(),
            bracketed_paste: s.bracketed_paste(),
            mouse_reporting: s.mouse_protocol_mode() != vt100::MouseProtocolMode::None,
        }
    }

    /// Kill the child (and its process group via the pty hangup).
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        self.alive.store(false, Ordering::Release);
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.kill();
        // Reap; the pty closes when `master` drops, hanging up the foreground process group.
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_for(s: &TerminalSession, want: &str) -> ScreenSnapshot {
        let t = Instant::now();
        loop {
            let snap = s.screen();
            if snap.text().contains(want) || t.elapsed() > Duration::from_secs(10) {
                return snap;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn captures_output_into_screen() {
        let args = vec!["-c".to_string(), "printf hello".to_string()];
        let s = TerminalSession::spawn_command(
            &std::env::temp_dir(),
            "/bin/sh",
            &args,
            TermSize { rows: 10, cols: 40 },
        )
        .unwrap();
        let snap = wait_for(&s, "hello");
        assert!(snap.row_text(0).starts_with("hello"), "{:?}", snap.text());
        assert_eq!((snap.rows, snap.cols), (10, 40));
        assert_eq!(snap.cells.len(), 400);
        assert_eq!(snap.cell(0, 0).unwrap().text, "h");
    }

    #[test]
    fn colors_and_cursor() {
        let args = vec!["-c".to_string(), r"printf '\033[1;31mR\033[0m'".to_string()];
        let s = TerminalSession::spawn_command(
            &std::env::temp_dir(),
            "/bin/sh",
            &args,
            TermSize::default(),
        )
        .unwrap();
        let snap = wait_for(&s, "R");
        let c = snap.cell(0, 0).unwrap();
        assert!(c.bold);
        assert_eq!(c.fg, Color::Idx(1));
    }

    #[test]
    fn resize_and_write_roundtrip() {
        let mut s = TerminalSession::spawn_command(
            &std::env::temp_dir(),
            "/bin/sh",
            &[],
            TermSize { rows: 10, cols: 40 },
        )
        .unwrap();
        s.resize(20, 100);
        assert_eq!(
            s.size(),
            TermSize {
                rows: 20,
                cols: 100
            }
        );
        let snap = s.screen();
        assert_eq!((snap.rows, snap.cols), (20, 100));
        s.write(b"echo $((6*7))\n");
        let snap = wait_for(&s, "\n42");
        assert!(snap.text().contains("42"), "{:?}", snap.text());
        assert!(s.is_alive());
        s.write(b"exit\n");
        let t = Instant::now();
        while s.is_alive() && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!s.is_alive());
    }

    #[test]
    fn title_and_range_text() {
        let args = vec![
            "-c".to_string(),
            r"printf '\033]0;mytitle\007abc\r\ndef'".to_string(),
        ];
        let s = TerminalSession::spawn_command(
            &std::env::temp_dir(),
            "/bin/sh",
            &args,
            TermSize::default(),
        )
        .unwrap();
        let snap = wait_for(&s, "def");
        assert_eq!(snap.title, "mytitle");
        assert_eq!(snap.text_range((0, 1), (1, 1)), "bc\nde");
    }

    #[test]
    fn scrollback_scroll() {
        let args = vec!["-c".to_string(), "seq 1 100".to_string()];
        let s = TerminalSession::spawn_command(
            &std::env::temp_dir(),
            "/bin/sh",
            &args,
            TermSize { rows: 10, cols: 20 },
        )
        .unwrap();
        let _ = wait_for(&s, "100");
        assert!(s.history_len() > 80);
        s.scroll(5);
        assert_eq!(s.screen().scrollback, 5);
        s.scroll_to_bottom();
        assert_eq!(s.screen().scrollback, 0);
    }
}
