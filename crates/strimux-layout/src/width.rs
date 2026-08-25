//! Column widths in the 2D strip grid.
//!
//! A column width is either a preset fraction of the viewport (`1/4, 1/3,
//! 1/2, 2/3, 3/4, 1`) or a fixed number of cells. Fractions re-derive from
//! the terminal width on resize; fixed-cell columns keep their cells
//! (see ``Layout Model`` invariant 3).

use serde::{Deserialize, Serialize};

/// A preset fraction for a column width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preset {
    Quarter,
    Third,
    Half,
    TwoThirds,
    ThreeQuarters,
    Full,
}

impl Preset {
    /// The default width for a brand-new column (`1/4` viewport).
    pub const DEFAULT: Preset = Preset::Quarter;

    /// The integer numerator / denominator of this preset.
    pub fn ratio(self) -> (u16, u16) {
        match self {
            Preset::Quarter => (1, 4),
            Preset::Third => (1, 3),
            Preset::Half => (1, 2),
            Preset::TwoThirds => (2, 3),
            Preset::ThreeQuarters => (3, 4),
            Preset::Full => (1, 1),
        }
    }

    /// The next preset in `cycle` order (the `cycle-width` verb). Cycles
    /// through the three user-facing sizes `1/3 -> 1/2 -> 1/4` and wraps.
    /// The legacy larger fractions snap straight back into that cycle.
    pub fn next(self) -> Preset {
        match self {
            Preset::Quarter => Preset::Third,
            Preset::Third => Preset::Half,
            Preset::Half => Preset::Quarter,
            Preset::TwoThirds | Preset::ThreeQuarters | Preset::Full => Preset::Quarter,
        }
    }
}

/// A column's width: a viewport fraction or a fixed cell count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Width {
    /// A fraction of the viewport's *current* width.
    Preset(Preset),
    /// A fixed number of cells, independent of the viewport.
    Cells(u16),
}

impl Width {
    /// The *default* new-column width: a quarter of the viewport.
    pub const DEFAULT: Width = Width::Preset(Preset::Quarter);

    /// Resolve this width to a concrete number of cells given the viewport
    /// width. Always returns at least 1 cell.
    pub fn cells(self, viewport_cols: u16) -> u16 {
        match self {
            Width::Preset(p) => {
                let (n, d) = p.ratio();
                // Fraction of the viewport, rounded up so a full column is
                // never squeezed to zero at the right edge.
                let cells = ((viewport_cols as u32) * (n as u32)).div_ceil(d as u32);
                cells.clamp(1, viewport_cols as u32) as u16
            }
            Width::Cells(c) => c.max(1),
        }
    }
}

impl Default for Width {
    fn default() -> Self {
        Width::DEFAULT
    }
}
