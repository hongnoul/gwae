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

    #[test]
    fn retired_and_unknown_statuses_degrade_instead_of_failing() {
        // A dev-reload handover written by an older gwae may carry the
        // retired `Done` status; a newer one may carry something we have
        // never heard of. Either way the handover must load (dropping every
        // pane over one status string would be absurd): `Done` reads as
        // `Idle` (success at a prompt is waiting for input), unknowns as
        // `Plain` (no claim).
        use crate::PaneStatus;
        let done: PaneStatus = serde_json::from_str("\"Done\"").unwrap();
        assert_eq!(done, PaneStatus::Idle);
        let unknown: PaneStatus = serde_json::from_str("\"Sparkling\"").unwrap();
        assert_eq!(unknown, PaneStatus::Plain);
        // Live variants still round-trip exactly.
        for st in [
            PaneStatus::Plain,
            PaneStatus::Running,
            PaneStatus::Idle,
            PaneStatus::Failed,
        ] {
            let json = serde_json::to_string(&st).unwrap();
            let back: PaneStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(st, back);
        }
    }
}
