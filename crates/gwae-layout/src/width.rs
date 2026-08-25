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
#[serde(from = "WireWidth", into = "WireWidth")]
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
    ///
    /// Note: for laying out a strip of columns, prefer
    /// `Layout::column_x_ranges`, which rounds *cumulative boundaries* so
    /// preset columns tile the viewport exactly. Summing per-column `cells`
    /// values overshoots by up to `d-1` cells when the viewport is not
    /// divisible by the preset denominator.
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

    /// This width in *twelfths of a cell* given the viewport width. Twelfths
    /// are exact for every preset denominator (2, 3, 4 all divide 12), so a
    /// strip can accumulate column positions without rounding drift and round
    /// only at each column boundary (see `Layout::column_x_ranges`).
    pub fn twelfths(self, viewport_cols: u16) -> u64 {
        match self {
            Width::Preset(p) => {
                let (n, d) = p.ratio();
                (viewport_cols as u64) * (n as u64) * (12 / d as u64)
            }
            Width::Cells(c) => (c.max(1) as u64) * 12,
        }
    }
}

/// Preset lookup by the name a *user* writes: case- and separator-insensitive,
/// so `"two-thirds"`, `"two_thirds"` and `"TwoThirds"` are all the same thing.
///
/// Config is hand-written and (since `gwae init`) machine-written prose,
/// not a serialization format, so the friendly spelling is the real one.
impl Preset {
    pub fn by_name(name: &str) -> Option<Preset> {
        let n: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        Some(match n.as_str() {
            "quarter" | "14" => Preset::Quarter,
            "third" | "13" => Preset::Third,
            "half" | "12" => Preset::Half,
            "twothirds" | "23" => Preset::TwoThirds,
            "threequarters" | "34" => Preset::ThreeQuarters,
            "full" | "1" | "11" => Preset::Full,
            _ => return None,
        })
    }

    /// The canonical config spelling, the inverse of [`Preset::by_name`].
    pub fn name(self) -> &'static str {
        match self {
            Preset::Quarter => "quarter",
            Preset::Third => "third",
            Preset::Half => "half",
            Preset::TwoThirds => "two-thirds",
            Preset::ThreeQuarters => "three-quarters",
            Preset::Full => "full",
        }
    }
}

/// The wire form of [`Width`], and the reason config can say any of:
///
/// ```toml
/// default_column_width = "half"                 # a preset by name
/// default_column_width = 80                     # fixed cells
/// default_column_width = { preset = "half" }    # the documented table
/// default_column_width = { cells = 80 }
/// ```
///
/// The derived enum representation only ever accepted `{ Preset = "Half" }`,
/// which is nobody's idea of a config file and did not match `docs/CONFIG.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WireWidth {
    /// `"half"` or `"1/2"`.
    Name(String),
    /// A bare integer: fixed cells.
    Cells(u16),
    /// `{ preset = "half" }` / `{ cells = 80 }` / legacy `{ Preset = "Half" }`.
    Table(WidthTable),
}

/// The table form, with both the friendly and the legacy derived spellings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidthTable {
    #[serde(alias = "Preset")]
    pub preset: Option<String>,
    #[serde(alias = "Cells")]
    pub cells: Option<u16>,
}

impl From<WireWidth> for Width {
    fn from(w: WireWidth) -> Width {
        match w {
            WireWidth::Name(n) => Preset::by_name(&n)
                .map(Width::Preset)
                .unwrap_or(Width::DEFAULT),
            WireWidth::Cells(c) => Width::Cells(c.max(1)),
            WireWidth::Table(t) => match (t.preset, t.cells) {
                (Some(p), _) => Preset::by_name(&p)
                    .map(Width::Preset)
                    .unwrap_or(Width::DEFAULT),
                (None, Some(c)) => Width::Cells(c.max(1)),
                (None, None) => Width::DEFAULT,
            },
        }
    }
}

impl From<Width> for WireWidth {
    fn from(w: Width) -> WireWidth {
        match w {
            Width::Preset(p) => WireWidth::Name(p.name().to_string()),
            Width::Cells(c) => WireWidth::Cells(c),
        }
    }
}

impl Default for Width {
    fn default() -> Self {
        Width::DEFAULT
    }
}
