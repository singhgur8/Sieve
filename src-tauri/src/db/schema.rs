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
    include_str!("../../migrations/0011_hardening.sql"),
    include_str!("../../migrations/0012_workflow_styles.sql"),
    include_str!("../../migrations/0013_workflow_state.sql"),
    include_str!("../../migrations/0014_linear_undo.sql"),
    include_str!("../../migrations/0015_apply_bases.sql"),
    include_str!("../../migrations/0016_cull_clarity.sql"),
    include_str!("../../migrations/0017_capture_transform.sql"),
    include_str!("../../migrations/0018_camera_serial_sync.sql"),
    include_str!("../../migrations/0019_target_cull.sql"),
    include_str!("../../migrations/0020_target_review.sql"),
    include_str!("../../migrations/0021_baseline_edit.sql"),
];
