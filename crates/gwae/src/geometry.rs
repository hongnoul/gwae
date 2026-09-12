//! Pixel metrics belong to the host's font, not to a pane's character count.
//! Preserve them when creating/resizing child PTYs so image applications can
//! measure their drawable area without querying the outer terminal directly.

use crossterm::terminal::{window_size, WindowSize};
use portable_pty::PtySize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CellPixels {
    pub width: u16,
    pub height: u16,
}

impl CellPixels {
    pub fn measure() -> Option<Self> {
        Self::from_window(window_size().ok()?)
    }

    fn from_window(size: WindowSize) -> Option<Self> {
        // Some terminals/platforms only provide rows and columns. Unknown is
        // zero, never an invented font size. Ignore partial/invalid reports.
        let width = size.width.checked_div(size.columns)?;
        let height = size.height.checked_div(size.rows)?;
        (width > 0 && height > 0).then_some(Self { width, height })
    }

    pub fn for_grid(self, cols: u16, rows: u16) -> Self {
        // winsize and terminal-core size replies use u16 products. Clamp the
        // cell metric first, keeping the ioctl and escape reply consistent.
        Self {
            width: self.width.min(u16::MAX / cols.max(1)),
            height: self.height.min(u16::MAX / rows.max(1)),
        }
    }

    pub fn pty_size(self, cols: u16, rows: u16) -> PtySize {
        let cell = self.for_grid(cols, rows);
        PtySize {
            cols,
            rows,
            pixel_width: cols * cell.width,
            pixel_height: rows * cell.height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_cells_exclude_fractional_window_padding() {
        let cell = CellPixels::from_window(WindowSize {
            columns: 215,
            rows: 63,
            width: 3448,
            height: 2160,
        })
        .unwrap();
        assert_eq!(
            cell,
            CellPixels {
                width: 16,
                height: 34
            }
        );
        let pane = cell.pty_size(52, 61);
        assert_eq!((pane.pixel_width, pane.pixel_height), (832, 2074));
    }

    #[test]
    fn unknown_or_invalid_metrics_are_not_fabricated() {
        for (cols, rows, width, height) in [
            (80, 24, 0, 0),
            (0, 24, 640, 384),
            (80, 0, 640, 384),
            (80, 24, 640, 0),
            (80, 24, 79, 384),
        ] {
            assert_eq!(
                CellPixels::from_window(WindowSize {
                    columns: cols,
                    rows,
                    width,
                    height,
                }),
                None
            );
        }
        let size = CellPixels::default().pty_size(80, 24);
        assert_eq!((size.pixel_width, size.pixel_height), (0, 0));
    }

    #[test]
    fn extreme_dimensions_do_not_overflow_or_disagree_with_cell_metrics() {
        let cell = CellPixels {
            width: u16::MAX,
            height: u16::MAX,
        };
        for (cols, rows) in [(0, 0), (1, 1), (1000, 3000), (u16::MAX, u16::MAX)] {
            let size = cell.pty_size(cols, rows);
            let clipped = cell.for_grid(cols, rows);
            assert_eq!(size.pixel_width as u32, cols as u32 * clipped.width as u32);
            assert_eq!(
                size.pixel_height as u32,
                rows as u32 * clipped.height as u32
            );
        }
    }
}
