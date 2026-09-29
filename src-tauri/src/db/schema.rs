//! Ordered schema migrations. Index `i` upgrades `user_version` from `i` to `i + 1`.
//! Never edit a shipped migration; append a new one.

pub const MIGRATIONS: &[&str] = &[include_str!("../../migrations/0001_init.sql")];
