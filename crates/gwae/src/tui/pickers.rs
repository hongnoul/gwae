//! Overlays: spawn-dir picker, quit confirm.

use gwae_term::{CColor, Cell};

use crate::theme::Palette;

/// Live state of the `⌥+d` spawn-directory picker.
///
/// Adds a typed filter, because the candidate list is dozens of repos.
/// `s` writes the highlighted directory back to the config
/// file, which is the difference between "this session" and "from now on".
pub(crate) struct DirPicker {
    pub(crate) all: Vec<crate::spawndir::Candidate>,
    pub(crate) query: String,
    pub(crate) sel: usize,
    /// Harness this picker is editing (e.g. "jcode"), empty when no harness.
    pub(crate) harness_label: String,
}

impl DirPicker {
    /// When the query itself expands to an existing directory, offer it as the
    /// top candidate (`typed`). This is what makes `~/` + Enter resolve to `$HOME`
    /// instead of to the top fuzzy match, and in general lets the user type any
    /// valid path (including one outside the discovered set) and pick it.
    pub(crate) fn typed_candidate(&self) -> Option<crate::spawndir::Candidate> {
        let q = self.query.trim();
        if q.is_empty() {
            return None;
        }
        let p = crate::spawndir::expand(q);
        if !p.is_dir() {
            return None;
        }
        let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
        Some(crate::spawndir::Candidate {
            label: crate::spawndir::tilde(&canon),
            path: canon,
            origin: "typed",
        })
    }
    pub(crate) fn shown(&self) -> Vec<crate::spawndir::Candidate> {
        let mut base = crate::spawndir::filter(&self.all, &self.query);
        if let Some(typed) = self.typed_candidate() {
            // De-duplicate: if the typed path is already in the filtered list
            // (e.g. query "~" already contains the "home" candidate), move it to
            // the top so a literal path always wins over a fuzzy match.
            let t = &typed.path;
            if let Some(pos) = base.iter().position(|c| &c.path == t) {
                base.remove(pos);
            }
            base.insert(0, typed);
        }
        base
    }
    pub(crate) fn current(&self) -> Option<crate::spawndir::Candidate> {
        self.shown().get(self.sel).cloned()
    }
    /// Move the highlight, clamped to the filtered list, wrapping at either
    /// end so a long repo list is reachable from both directions.
    pub(crate) fn step(&mut self, d: i32) {
        let n = self.shown().len();
        if n == 0 {
            self.sel = 0;
            return;
        }
        let i = self.sel as i32 + d;
        self.sel = i.rem_euclid(n as i32) as usize;
    }
}

/// Draw the spawn-directory picker: the filter line, the matching directories
/// with the selection highlighted, and the key legend.
///
/// A directory does not repaint the screen, so this panel shows the list.
pub(crate) fn draw_dir_picker(
    out: &mut [Cell],
    cols: u16,
    rows: u16,
    pick: &DirPicker,
    pal: &Palette,
) {
    let shown = pick.shown();
    let rows_shown = shown.len().clamp(1, 10);
    // Title names the harness so the user knows which agent the picked
    // directory will apply to. Plain "spawn dir:" when no harness is set.
    let title = if pick.harness_label.is_empty() {
        format!(" spawn dir: {}_ ", pick.query)
    } else {
        format!(" spawn dir [{}]: {}_ ", pick.harness_label, pick.query)
    };
    // The save key is the *chord*, not a bare `s`: every printable key types
    // into the filter, so advertising `s` would tell the user to type a
    // letter that filters instead of saving.
    let help = format!(
        " ↑/↓·⌃/⌥+j/k pick   ⏎ session   {} save to config   esc cancel ",
        crate::keys::chord("s")
    );
    let help = help.as_str();
    let widest = shown
        .iter()
        .take(rows_shown)
        .map(|c| c.label.chars().count() + c.origin.chars().count() + 4)
        .max()
        .unwrap_or(0);
    // A long discovered or typed path must not make the whole picker vanish.
    let bw = (widest.max(title.chars().count()).max(help.chars().count()) + 2)
        .min((cols as usize).saturating_sub(2));
    let bh = rows_shown + 4;
    if bw < 6 || (rows as usize) < bh + 2 {
        return;
    }
    let ox = ((cols as usize) - bw) / 2;
    let oy = ((rows as usize) - bh) / 2;
    for y in 0..bh {
        for x in 0..bw {
            if let Some(c) = out.get_mut((oy + y) * cols as usize + ox + x) {
                *c = Cell {
                    ch: ' ',
                    style: gwae_term::Style {
                        fg: pal.text,
                        bg: pal.surface,
                        ..Default::default()
                    },
                    width: 1,
                    ..Default::default()
                };
            }
        }
    }
    let mut edge = |x: usize, y: usize, ch: char| {
        if let Some(c) = out.get_mut(y * cols as usize + x) {
            c.ch = ch;
            c.style.fg = pal.accent;
            c.style.bg = pal.surface;
            c.width = 1;
        }
    };
    for x in 0..bw {
        edge(ox + x, oy, '─');
        edge(ox + x, oy + bh - 1, '─');
    }
    for y in 0..bh {
        edge(ox, oy + y, '│');
        edge(ox + bw - 1, oy + y, '│');
    }
    edge(ox, oy, '╭');
    edge(ox + bw - 1, oy, '╮');
    edge(ox, oy + bh - 1, '╰');
    edge(ox + bw - 1, oy + bh - 1, '╯');

    // A free function rather than a closure: the selection highlight below
    // also needs `&mut out`, and a capturing closure would hold the borrow
    // for the whole body.
    #[allow(clippy::too_many_arguments)]
    fn text(
        out: &mut [Cell],
        cols: u16,
        limit: usize,
        row: usize,
        col: usize,
        s: &str,
        fg: CColor,
        bg: CColor,
        bold: bool,
    ) {
        for (i, ch) in s.chars().enumerate() {
            if col + i >= limit {
                break;
            }
            if let Some(c) = out.get_mut(row * cols as usize + col + i) {
                c.ch = ch;
                c.style.fg = fg;
                c.style.bg = bg;
                c.style.bold = bold;
                c.width = 1;
            }
        }
    }
    let lim = ox + bw - 1;
    text(
        out,
        cols,
        lim,
        oy + 1,
        ox + 1,
        &title,
        pal.accent,
        pal.surface,
        true,
    );
    if shown.is_empty() {
        text(
            out,
            cols,
            lim,
            oy + 2,
            ox + 2,
            "no match",
            pal.label,
            pal.surface,
            false,
        );
    }
    // Scroll the window so the selection is always on screen, even when the
    // filter leaves more matches than the panel can hold.
    let first = pick.sel.saturating_sub(rows_shown.saturating_sub(1));
    for (i, c) in shown.iter().skip(first).take(rows_shown).enumerate() {
        let y = oy + 2 + i;
        let selected = first + i == pick.sel;
        let (fg, bg) = if selected {
            (pal.base, pal.accent)
        } else {
            (pal.text, pal.surface)
        };
        if selected {
            for x in 1..bw - 1 {
                if let Some(cell) = out.get_mut(y * cols as usize + ox + x) {
                    cell.ch = ' ';
                    cell.style.bg = bg;
                    cell.style.fg = fg;
                    cell.width = 1;
                }
            }
        }
        let ow = c.origin.chars().count();
        let at = ox + bw - 2 - ow.min(bw.saturating_sub(4));
        // Reserve the origin column and keep the end of the path, where the
        // directory name lives. Only the display is shortened, never c.path.
        let label_width = at.saturating_sub(ox + 3);
        let label_len = c.label.chars().count();
        let label = if label_width == 0 {
            String::new()
        } else if label_len > label_width {
            format!(
                "…{}",
                c.label
                    .chars()
                    .skip(label_len - label_width + 1)
                    .collect::<String>()
            )
        } else {
            c.label.clone()
        };
        text(out, cols, lim, y, ox + 2, &label, fg, bg, selected);
        let ofg = if selected { fg } else { pal.label };
        text(out, cols, lim, y, at, c.origin, ofg, bg, false);
    }
    text(
        out,
        cols,
        lim,
        oy + bh - 2,
        ox + 1,
        help,
        pal.label,
        pal.surface,
        false,
    );
}

/// Live state of the `⌥+;` / `⌥+⇧+;` harness picker overlay.
///
/// Unlike the `⌥+d` directory picker there is no save key: a pick is
/// remembered in the state file automatically, and a shell is just another
/// row. `new_row` remembers whether the chord was `⌥+;` or `⌥+Shift+;` (`⌥+⇧+;`
/// always opens the picker, ignoring the fast paths `⌥+;` takes), and
/// `notice` carries a one-line explanation when the overlay opened for a
/// reason (an override that is not installed, a remembered pick that is
/// gone, nothing installed at all).
pub(crate) struct HarnessPicker {
    pub(crate) all: Vec<crate::agent::Found>,
    pub(crate) query: String,
    pub(crate) sel: usize,
    pub(crate) new_row: bool,
    pub(crate) notice: Option<String>,
}

/// One selectable harness-picker row.
#[derive(Debug, Clone)]
pub(crate) enum HarnessChoice {
    /// A detected harness, by value.
    Listed(crate::agent::Found),
    /// A typed command that resolves but was not listed.
    Typed(String),
    /// A plain shell; remembered nothing.
    Shell,
}

impl HarnessPicker {
    fn matches(&self, f: &crate::agent::Found) -> bool {
        let q = self.query.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        f.cmd.to_lowercase().contains(&q) || f.label.to_lowercase().contains(&q)
    }
    /// When the query itself resolves to a command that is not listed, offer
    /// it as the top row. This is the escape hatch that makes a harness gwae
    /// has never heard of usable immediately, without a config key.
    pub(crate) fn typed_candidate(&self) -> Option<String> {
        let q = self.query.trim();
        if q.is_empty() {
            return None;
        }
        if self.all.iter().any(|f| f.cmd == q) {
            return None;
        }
        crate::agent::command_available(q).then(|| q.to_string())
    }
    pub(crate) fn shown(&self) -> Vec<HarnessChoice> {
        let mut out: Vec<HarnessChoice> = self
            .all
            .iter()
            .filter(|f| self.matches(f))
            .cloned()
            .map(HarnessChoice::Listed)
            .collect();
        if let Some(t) = self.typed_candidate() {
            out.insert(0, HarnessChoice::Typed(t));
        }
        out.push(HarnessChoice::Shell);
        out
    }
    pub(crate) fn current(&self) -> Option<HarnessChoice> {
        // Clamped, not indexed: backspacing the filter can shrink the list
        // under a highlight moved by the arrows, and ⏎ must still spawn
        // something rather than silently cancel.
        let shown = self.shown();
        if shown.is_empty() {
            return None;
        }
        shown.get(self.sel.min(shown.len() - 1)).cloned()
    }
    /// Move the highlight, clamped to the filtered list, wrapping at either
    /// end like the directory picker.
    pub(crate) fn step(&mut self, d: i32) {
        let n = self.shown().len();
        if n == 0 {
            self.sel = 0;
            return;
        }
        let i = self.sel as i32 + d;
        self.sel = i.rem_euclid(n as i32) as usize;
    }
}

/// Draw the harness picker: the filter line, an optional notice, the matching
/// harnesses with the selection highlighted, and the key legend.
pub(crate) fn draw_harness_picker(
    out: &mut [Cell],
    cols: u16,
    rows: u16,
    pick: &HarnessPicker,
    pal: &Palette,
) {
    let shown = pick.shown();
    let rows_shown = shown.len().clamp(1, 10);
    let notice = pick.notice.as_deref().unwrap_or("");
    let notice_lines = if notice.is_empty() { 0 } else { 1 };
    let title = format!(" pick agent: {}_ ", pick.query);
    let help = " ↑/↓·⌃/⌥+j/k pick   ⏎ spawn   esc cancel ";
    // Row text: labels only, so the list fits a quarter-width pane. The first
    // row is the Enter default and is labeled as such.
    let row_text: Vec<String> = shown
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let dflt = if i == 0 { "  (default)" } else { "" };
            match c {
                HarnessChoice::Listed(f) => format!("{}{dflt}", f.label),
                HarnessChoice::Typed(t) => format!("run: {t}{dflt}"),
                HarnessChoice::Shell => format!("just a shell{dflt}"),
            }
        })
        .collect();
    let widest = row_text
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0);
    let bw = (widest
        .max(title.chars().count())
        .max(notice.chars().count())
        .max(help.chars().count())
        + 2)
    .min((cols as usize).saturating_sub(2));
    let bh = rows_shown + notice_lines + 4;
    if bw < 6 || (rows as usize) < bh + 2 {
        return;
    }
    let ox = ((cols as usize) - bw) / 2;
    let oy = ((rows as usize) - bh) / 2;
    for y in 0..bh {
        for x in 0..bw {
            if let Some(c) = out.get_mut((oy + y) * cols as usize + ox + x) {
                *c = Cell {
                    ch: ' ',
                    style: gwae_term::Style {
                        fg: pal.text,
                        bg: pal.surface,
                        ..Default::default()
                    },
                    width: 1,
                    ..Default::default()
                };
            }
        }
    }
    let mut edge = |x: usize, y: usize, ch: char| {
        if let Some(c) = out.get_mut(y * cols as usize + x) {
            c.ch = ch;
            c.style.fg = pal.accent;
            c.style.bg = pal.surface;
            c.width = 1;
        }
    };
    for x in 0..bw {
        edge(ox + x, oy, '─');
        edge(ox + x, oy + bh - 1, '─');
    }
    for y in 0..bh {
        edge(ox, oy + y, '│');
        edge(ox + bw - 1, oy + y, '│');
    }
    edge(ox, oy, '╭');
    edge(ox + bw - 1, oy, '╮');
    edge(ox, oy + bh - 1, '╰');
    edge(ox + bw - 1, oy + bh - 1, '╯');

    #[allow(clippy::too_many_arguments)]
    fn text(
        out: &mut [Cell],
        cols: u16,
        limit: usize,
        row: usize,
        col: usize,
        s: &str,
        fg: CColor,
        bg: CColor,
        bold: bool,
    ) {
        for (i, ch) in s.chars().enumerate() {
            if col + i >= limit {
                break;
            }
            if let Some(c) = out.get_mut(row * cols as usize + col + i) {
                c.ch = ch;
                c.style.fg = fg;
                c.style.bg = bg;
                c.style.bold = bold;
                c.width = 1;
            }
        }
    }
    let lim = ox + bw - 1;
    text(
        out,
        cols,
        lim,
        oy + 1,
        ox + 1,
        &title,
        pal.accent,
        pal.surface,
        true,
    );
    let mut y = oy + 2;
    if !notice.is_empty() {
        text(
            out,
            cols,
            lim,
            y,
            ox + 2,
            notice,
            pal.label,
            pal.surface,
            false,
        );
        y += 1;
    }
    let first = pick.sel.saturating_sub(rows_shown.saturating_sub(1));
    for (i, label) in row_text.iter().skip(first).take(rows_shown).enumerate() {
        let yy = y + i;
        let selected = first + i == pick.sel;
        let (fg, bg) = if selected {
            (pal.base, pal.accent)
        } else {
            (pal.text, pal.surface)
        };
        if selected {
            for x in 1..bw - 1 {
                if let Some(cell) = out.get_mut(yy * cols as usize + ox + x) {
                    cell.ch = ' ';
                    cell.style.bg = bg;
                    cell.style.fg = fg;
                    cell.width = 1;
                }
            }
        }
        // Keep the end of the label, where the name lives.
        let label_width = bw.saturating_sub(4);
        let label_len = label.chars().count();
        let label = if label_width == 0 {
            String::new()
        } else if label_len > label_width {
            format!(
                "…{}",
                label
                    .chars()
                    .skip(label_len - label_width + 1)
                    .collect::<String>()
            )
        } else {
            label.clone()
        };
        text(out, cols, lim, yy, ox + 2, &label, fg, bg, selected);
    }
    text(
        out,
        cols,
        lim,
        oy + bh - 2,
        ox + 1,
        help,
        pal.label,
        pal.surface,
        false,
    );
}

/// Centered disclaimer for the force-quit chord (`⌥+Shift+q`).
///
/// Quitting kills every pane and everything running in them, which is the one
/// irreversible thing gwae can do, so the chord opens this overlay instead
/// of exiting outright: it names the cost (how many panes die) and requires a
/// second, deliberate keystroke.
pub(crate) fn draw_quit_confirm(
    out: &mut [Cell],
    cols: u16,
    rows: u16,
    panes: usize,
    pal: &Palette,
) {
    let title = format!(
        " force quit gwae? {} pane{} will be killed ",
        panes,
        if panes == 1 { "" } else { "s" }
    );
    let warn = " running commands are terminated immediately ";
    let help = format!(
        " {} again or ⏎ quits   esc cancels ",
        crate::keys::shift_chord("q")
    );
    let bw = [title.as_str(), warn, help.as_str()]
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    let bh = 5usize;
    if (cols as usize) < bw + 2 || (rows as usize) < bh + 2 {
        return;
    }
    let ox = ((cols as usize) - bw) / 2;
    let oy = ((rows as usize) - bh) / 2;
    for y in 0..bh {
        for x in 0..bw {
            if let Some(c) = out.get_mut((oy + y) * cols as usize + ox + x) {
                *c = Cell {
                    ch: ' ',
                    style: gwae_term::Style {
                        fg: pal.text,
                        bg: pal.surface,
                        ..Default::default()
                    },
                    width: 1,
                    ..Default::default()
                };
            }
        }
    }
    // The border uses the failed tint: this is the destructive overlay, and it
    // must not be mistaken at a glance for another overlay.
    let mut edge = |x: usize, y: usize, ch: char| {
        if let Some(c) = out.get_mut(y * cols as usize + x) {
            c.ch = ch;
            c.style.fg = pal.failed;
            c.style.bg = pal.surface;
            c.width = 1;
        }
    };
    for x in 0..bw {
        edge(ox + x, oy, '─');
        edge(ox + x, oy + bh - 1, '─');
    }
    for y in 0..bh {
        edge(ox, oy + y, '│');
        edge(ox + bw - 1, oy + y, '│');
    }
    edge(ox, oy, '╭');
    edge(ox + bw - 1, oy, '╮');
    edge(ox, oy + bh - 1, '╰');
    edge(ox + bw - 1, oy + bh - 1, '╯');

    let mut text = |row: usize, s: &str, fg: CColor, bold: bool| {
        let chars: Vec<char> = s.chars().collect();
        let tx = ox + 1 + (bw - 2).saturating_sub(chars.len()) / 2;
        for (i, ch) in chars.iter().enumerate() {
            if tx + i >= ox + bw - 1 {
                break;
            }
            if let Some(c) = out.get_mut(row * cols as usize + tx + i) {
                c.ch = *ch;
                c.style.fg = fg;
                c.style.bg = pal.surface;
                c.style.bold = bold;
                c.width = 1;
            }
        }
    };
    text(oy + 1, &title, pal.failed, true);
    text(oy + 2, warn, pal.text, false);
    text(oy + 3, &help, pal.text, false);
}

#[cfg(test)]
mod tests {
    use super::super::input::{handle_key, Cmd};
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use gwae_term::{CColor, Cell};

    #[test]
    fn dir_picker_keeps_long_directory_matches_visible() {
        let label = format!("~/work/{}/fresh-scaffold", "long-parent-".repeat(10));
        let pick = DirPicker {
            all: vec![crate::spawndir::Candidate {
                path: std::path::PathBuf::from(&label),
                label,
                origin: "directory",
            }],
            query: "fresh-scaffold".into(),
            sel: 0,
            harness_label: String::new(),
        };
        for cols in [60, 80, 100] {
            let rows = 24;
            let mut out = vec![Cell::default(); cols as usize * rows as usize];
            draw_dir_picker(&mut out, cols, rows, &pick, &Palette::default());
            let lines: Vec<String> = out
                .chunks(cols as usize)
                .map(|row| row.iter().map(|cell| cell.ch).collect())
                .collect();
            assert!(
                lines.iter().any(|line| line.contains("spawn dir:")),
                "{cols}: {lines:?}"
            );
            assert!(
                lines.iter().any(|line| line.contains("…")
                    && line.contains("fresh-scaffold")
                    && line.contains("directory")),
                "keep the basename and origin visible at {cols} columns: {lines:?}"
            );
            assert_eq!(pick.current().unwrap().path, pick.all[0].path);
        }
    }

    #[test]
    fn force_quit_chord_arms_a_centered_disclaimer() {
        // The chord itself must still decode as Quit (the run loop turns that
        // into "arm the overlay"), and the overlay must actually paint a
        // centered, framed box that names the cost in the user's own key
        // vocabulary. A quit that exits without this box is the bug.
        let ev = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert_eq!(handle_key(&ev), Some(Cmd::Quit));

        let cols: u16 = 80;
        let rows: u16 = 24;
        let mut out = vec![Cell::default(); cols as usize * rows as usize];
        let pal = Palette::default();
        draw_quit_confirm(&mut out, cols, rows, 4, &pal);
        let lines: Vec<String> = (0..rows)
            .map(|y| {
                (0..cols)
                    .map(|x| out[y as usize * cols as usize + x as usize].ch)
                    .collect()
            })
            .collect();
        assert!(
            lines.iter().any(|s| s.contains("force quit gwae?")),
            "disclaimer names the action, got {lines:?}"
        );
        assert!(
            lines.iter().any(|s| s.contains("4 panes will be killed")),
            "disclaimer names the cost, got {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|s| s.contains(&crate::keys::shift_chord("q"))),
            "disclaimer says how to confirm, got {lines:?}"
        );
        assert!(
            lines.iter().any(|s| s.contains("esc cancels")),
            "disclaimer says how to back out, got {lines:?}"
        );
        // Framed and centered: corners exist, and the painted rows sit around
        // the middle of the screen rather than at an edge. The panel fill
        // uses the terminal-native surface (Default), so painted rows are
        // detected by glyphs, not by background color.
        let painted: Vec<usize> = (0..rows as usize)
            .filter(|y| (0..cols as usize).any(|x| out[y * cols as usize + x].ch != ' '))
            .collect();
        assert!(
            out.iter().any(|c| c.ch == '╭') && out.iter().any(|c| c.ch == '╯'),
            "disclaimer is a framed box"
        );
        let mid = rows as usize / 2;
        assert!(
            painted.first().is_some_and(|f| *f < mid) && painted.last().is_some_and(|l| *l > mid),
            "disclaimer straddles the screen center, painted {painted:?}"
        );
        // Singular/plural, because "1 panes" reads like a bug in a warning.
        let mut one = vec![Cell::default(); cols as usize * rows as usize];
        draw_quit_confirm(&mut one, cols, rows, 1, &pal);
        let text: String = one.iter().map(|c| c.ch).collect();
        assert!(
            text.contains("1 pane will be killed"),
            "singular pane count"
        );
        // Too small to render honestly: paint nothing rather than a clipped
        // warning the user cannot read.
        let mut tiny = vec![Cell::default(); 10 * 4];
        draw_quit_confirm(&mut tiny, 10, 4, 3, &pal);
        assert!(
            tiny.iter().all(|c| c.style.bg == CColor::Default),
            "no partial disclaimer may be painted"
        );
    }

    fn hfound(cmd: &str) -> crate::agent::Found {
        crate::agent::Found {
            cmd: cmd.into(),
            label: cmd.into(),
            path: std::path::PathBuf::from("/bin").join(cmd),
        }
    }

    fn harness_pick(cmds: &[&str]) -> HarnessPicker {
        HarnessPicker {
            all: cmds.iter().map(|c| hfound(c)).collect(),
            query: String::new(),
            sel: 0,
            new_row: false,
            notice: None,
        }
    }

    #[test]
    fn the_harness_picker_always_offers_a_shell_and_labels_the_default() {
        let pick = harness_pick(&["claude", "aider"]);
        let shown = pick.shown();
        assert_eq!(shown.len(), 3, "two harnesses plus the shell row");
        assert!(matches!(pick.current(), Some(HarnessChoice::Listed(_))));
        // Enter on a fresh picker is the first harness, never the shell.
        match pick.current().unwrap() {
            HarnessChoice::Listed(f) => assert_eq!(f.cmd, "claude"),
            other => panic!("expected the first harness, got {other:?}"),
        }
        // With nothing installed the only row is the shell.
        let empty = harness_pick(&[]);
        assert_eq!(empty.shown().len(), 1);
        assert!(matches!(empty.current(), Some(HarnessChoice::Shell)));

        let cols: u16 = 80;
        let rows: u16 = 24;
        let mut out = vec![Cell::default(); cols as usize * rows as usize];
        draw_harness_picker(&mut out, cols, rows, &pick, &Palette::default());
        let text: String = out.iter().map(|c| c.ch).collect();
        assert!(text.contains("pick agent"), "got:\n{text}");
        assert!(text.contains("just a shell"), "got:\n{text}");
        assert!(text.contains("(default)"), "got:\n{text}");
    }

    #[test]
    fn the_harness_picker_filters_and_offers_typed_commands() {
        let mut pick = harness_pick(&["claude", "aider"]);
        pick.query = "aid".into();
        let shown = pick.shown();
        assert_eq!(shown.len(), 2, "one match plus the shell row: {shown:?}");
        assert!(
            matches!(pick.current(), Some(HarnessChoice::Listed(_))),
            "the match stays the default"
        );
        // A resolvable but unlisted command becomes the top row.
        pick.query = "sh".into();
        assert_eq!(pick.typed_candidate().as_deref(), Some("sh"));
        assert!(
            matches!(pick.current(), Some(HarnessChoice::Typed(_))),
            "the typed command wins over the shell row"
        );
        // A typo resolves to nothing and offers only the shell.
        pick.query = "gwae-no-such-harness-xyz".into();
        assert_eq!(pick.typed_candidate(), None);
        assert_eq!(pick.shown().len(), 1);
    }

    #[test]
    fn the_harness_picker_steps_and_wraps_like_the_directory_picker() {
        let mut pick = harness_pick(&["claude"]);
        // claude, shell: stepping past the end wraps.
        pick.step(1);
        assert!(matches!(pick.current(), Some(HarnessChoice::Shell)));
        pick.step(1);
        assert!(matches!(pick.current(), Some(HarnessChoice::Listed(_))));
        pick.step(-1);
        assert!(matches!(pick.current(), Some(HarnessChoice::Shell)));
    }

    #[test]
    fn enter_after_shrinking_the_filter_still_spawns() {
        // Arrows then filtering can leave sel past the end of the list;
        // current() clamps to a real row so ⏎ spawns instead of silently
        // canceling. (The shell row is always last, so the worst case is a
        // shell, never nothing.)
        let mut pick = harness_pick(&["claude", "aider"]);
        pick.step(2);
        pick.query = "claude".into();
        assert!(
            pick.current().is_some(),
            "a shrunk list must still offer a row"
        );
    }
}
