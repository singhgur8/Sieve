//! Ordered schema migrations. Index `i` upgrades `user_version` from `i` to `i + 1`.
//! Never edit a shipped migration; append a new one.

pub const MIGRATIONS: &[&str] = &[
    include_str!("../../migrations/0001_init.sql"),
    include_str!("../../migrations/0002_ingest.sql"),
    include_str!("../../migrations/0003_analysis.sql"),
    include_str!("../../migrations/0004_xmp.sql"),
    include_str!("../../migrations/0005_editor.sql"),
    include_str!("../../migrations/0006_export.sql"),
    include_str!("../../migrations/0007_scenes.sql"),
    include_str!("../../migrations/0008_ux.sql"),
    include_str!("../../migrations/0009_parity_sources.sql"),
    include_str!("../../migrations/0010_masks.sql"),
];
