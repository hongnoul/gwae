//! Serialization notes for the layout model.
//!
//! `Layout` derives `Serialize`/`Deserialize` directly so the binary can dump
//! the whole tree via `Alt+Shift+d`. Round-trip identity is verified in tests
//! below (`serde_json` is a dev-dependency only; the library keeps its
//! std + serde surface, per the monorepo rules).

#[cfg(test)]
mod tests {
    use crate::{Layout, Width};

    #[test]
    fn default_layout_roundtrips() {
        let layout = Layout::default();
        let json = serde_json::to_string(&layout).unwrap();
        let back: Layout = serde_json::from_str(&json).unwrap();
        assert_eq!(layout, back);
    }

    #[test]
    fn multi_column_roundtrips() {
        let mut layout = Layout::default();
        let row = layout.focus.row;
        let p = layout.alloc_pane();
        layout.add_column(row, Width::Cells(40), vec![p]);
        let json = serde_json::to_string(&layout).unwrap();
        let back: Layout = serde_json::from_str(&json).unwrap();
        assert_eq!(layout, back);
    }
}
