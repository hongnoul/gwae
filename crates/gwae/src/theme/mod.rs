//! Chrome palette: the enforced retro default plus manual overrides.
//!
//! gwae paints its own chrome colors. Anything under `[theme]` in the config
//! overrides the retro default key by key; there are no presets and no
//! picker.

mod config;
mod palette;

pub use config::ThemeConfig;
pub use palette::Palette;
