-- Sieve catalog schema v8 (UX additions, IPC v8).
--
-- Burst keepers chosen by the user (`set_burst_keeper`). Burst groups are rebuilt on every
-- rescore, so the choice is stored per image: when regrouping, a group containing a pinned
-- image makes it the keeper (highest `overall` among several pinned members). Setting a
-- keeper unpins the other members of that group. Written by `ml::store` only.
CREATE TABLE burst_keeper_pins (
    image_id  INTEGER PRIMARY KEY REFERENCES images(id) ON DELETE CASCADE,
    pinned_at INTEGER NOT NULL
);

-- UI preferences (`get_ui_prefs` / `set_ui_prefs`) live in catalog_meta under key
-- 'ui_prefs' as JSON of `UiPrefs`; absent = defaults. No DDL needed.
