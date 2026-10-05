//! Auto Sync (IPC v19.2 `sync_delta`): one committed edit of the active photo (`before` ->
//! `after`) goes to the source and to every target as one undoable batch (kind `sync`).
//! Only the groups that changed are touched; exposure and white balance can be applied
//! **relatively** (the source's change added to each target's own value), so photos with
//! different per-frame exposure / WB (matched scenes) keep their differences.
//!
//! White balance in relative mode: the temperature change is taken in mireds
//! (`1e6 / K`, perceptually even, the scale `ParametricAdjustments::lerp` uses) and the tint
//! change is additive; `as_shot` sides are resolved to the camera's as-shot values
//! (`DevelopInfo.asShot`) supplied by the caller. A change *to* `as_shot` is copied as is.
//! When an as-shot value is unknown the target gets the source's absolute white balance
//! (reported in `SyncDeltaResult.absoluteWbIds`).
//! Implemented by the architect; rust-engine-dev owns it from here.

use std::collections::HashMap;

use rusqlite::Connection;

use super::batches::{self, BatchItem, BatchKind};
use super::history;
use super::wb::{MAX_TEMP, MAX_TINT, MIN_TEMP, MIN_TINT};
use crate::db::repo;
use crate::ipc::error::{AppError, AppResult};
use crate::ipc::types::{
    AdjustmentField, ImageId, ParametricAdjustments, SyncDeltaOptions, SyncDeltaResult, WhiteBalance,
    WhiteBalanceValues,
};

/// Groups compared to find what an edit changed: every group except the per-frame ones
/// (`SyncDeltaOptions::NEVER_SYNCED`) and the noise-reduction subsets (covered by
/// `noise_reduction`).
fn candidate_fields() -> impl Iterator<Item = AdjustmentField> {
    AdjustmentField::ALL.iter().copied().filter(|f| {
        !SyncDeltaOptions::NEVER_SYNCED.contains(f)
            && !matches!(f, AdjustmentField::NoiseReductionLuminance | AdjustmentField::NoiseReductionColor)
    })
}

/// Groups that differ between `before` and `after`, in `AdjustmentField` order, limited to
/// `only` when given. Never crop / masks / transform.
pub fn changed_fields(
    before: &ParametricAdjustments,
    after: &ParametricAdjustments,
    only: Option<&[AdjustmentField]>,
) -> Vec<AdjustmentField> {
    candidate_fields()
        .filter(|f| only.is_none_or(|o| o.contains(f)))
        .filter(|f| {
            let mut probe = after.clone();
            probe.copy_fields(before, &[*f]);
            probe != *after
        })
        .collect()
}

/// Validates `options.relative` and `options.label`; returns the label.
fn check_options(options: &SyncDeltaOptions) -> AppResult<String> {
    if let Some(f) = options.relative.iter().find(|f| !SyncDeltaOptions::RELATIVE.contains(f)) {
        return Err(AppError::invalid(format!(
            "{} cannot be synced relatively (only exposure and white_balance)",
            f.as_str()
        )));
    }
    let label = options.label.clone().unwrap_or_else(|| SyncDeltaOptions::DEFAULT_LABEL.to_owned());
    let n = label.chars().count();
    if n == 0 || n > 100 {
        return Err(AppError::invalid("label must be 1..=100 characters"));
    }
    Ok(label)
}

fn mired(k: f32) -> f32 {
    1.0e6 / k
}

/// Temperature / tint of `wb`, resolving `as_shot` with `as_shot`.
fn values(wb: WhiteBalance, as_shot: Option<WhiteBalanceValues>) -> Option<WhiteBalanceValues> {
    match wb {
        WhiteBalance::Custom { temperature_k, tint } => Some(WhiteBalanceValues { temperature_k, tint }),
        WhiteBalance::AsShot => as_shot,
    }
}

/// The relative white-balance change of `before` -> `after` applied to `target`. `None` when it
/// cannot be computed (an as-shot value is unknown): the caller copies `after` instead.
pub fn shift_white_balance(
    target: WhiteBalance,
    target_as_shot: Option<WhiteBalanceValues>,
    before: WhiteBalance,
    after: WhiteBalance,
    source_as_shot: Option<WhiteBalanceValues>,
) -> Option<WhiteBalance> {
    if before == after {
        return Some(target);
    }
    if after == WhiteBalance::AsShot {
        return Some(WhiteBalance::AsShot);
    }
    let (b, a) = (values(before, source_as_shot)?, values(after, source_as_shot)?);
    let t = values(target, target_as_shot)?;
    let m = (mired(t.temperature_k) + mired(a.temperature_k) - mired(b.temperature_k)).max(mired(MAX_TEMP));
    Some(WhiteBalance::Custom {
        temperature_k: (1.0e6 / m).clamp(MIN_TEMP, MAX_TEMP).round(),
        tint: (t.tint + a.tint - b.tint).clamp(MIN_TINT, MAX_TINT).round(),
    })
}

/// `target` with the `fields` changes of `before` -> `after`: groups in `relative` shifted by
/// the source's change, the others copied from `after`. Returns the new settings and whether
/// the white balance had to be copied absolutely (unknown as-shot value).
pub fn apply_delta(
    target: &ParametricAdjustments,
    target_as_shot: Option<WhiteBalanceValues>,
    before: &ParametricAdjustments,
    after: &ParametricAdjustments,
    source_as_shot: Option<WhiteBalanceValues>,
    fields: &[AdjustmentField],
    relative: &[AdjustmentField],
) -> (ParametricAdjustments, bool) {
    let mut next = target.clone();
    let mut absolute_wb = false;
    let copied: Vec<AdjustmentField> = fields.iter().copied().filter(|f| !relative.contains(f)).collect();
    next.copy_fields(after, &copied);
    for f in fields.iter().filter(|f| relative.contains(f)) {
        match f {
            AdjustmentField::Exposure => {
                let v = target.exposure + (after.exposure - before.exposure);
                next.exposure = ((v.clamp(-5.0, 5.0)) * 100.0).round() / 100.0;
            }
            AdjustmentField::WhiteBalance => {
                match shift_white_balance(
                    target.white_balance,
                    target_as_shot,
                    before.white_balance,
                    after.white_balance,
                    source_as_shot,
                ) {
                    Some(wb) => next.white_balance = wb,
                    None => {
                        next.white_balance = after.white_balance;
                        absolute_wb = true;
                    }
                }
            }
            _ => next.copy_fields(after, &[*f]),
        }
    }
    (next, absolute_wb)
}

/// Ids whose as-shot white balance `sync_delta` needs (resolve them before
/// [`sync_delta_recorded`]): the source when its `before` is `as_shot` and the white balance
/// is synced relatively, and every target whose stored white balance is `as_shot`.
pub fn as_shot_needed(
    conn: &Connection,
    source_id: ImageId,
    before: &ParametricAdjustments,
    after: &ParametricAdjustments,
    target_ids: &[ImageId],
    options: &SyncDeltaOptions,
) -> AppResult<Vec<ImageId>> {
    let fields = changed_fields(before, after, options.fields.as_deref());
    let relative_wb = fields.contains(&AdjustmentField::WhiteBalance)
        && options.relative.contains(&AdjustmentField::WhiteBalance)
        && after.white_balance != WhiteBalance::AsShot;
    if !relative_wb {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    if before.white_balance == WhiteBalance::AsShot {
        out.push(source_id);
    }
    for &id in target_ids {
        if id != source_id
            && !out.contains(&id)
            && repo::get_adjustments(conn, id)?.white_balance == WhiteBalance::AsShot
        {
            out.push(id);
        }
    }
    Ok(out)
}

/// `sync_delta`: writes `after` to the source and the `before` -> `after` change to every
/// target (duplicates and the source itself ignored) as one batch of kind `sync` labelled
/// `options.label`. `as_shot` = known as-shot white balances (see [`as_shot_needed`]). Atomic;
/// unknown image -> `not_found`; invalid settings / options -> `invalid_argument`.
pub fn sync_delta_recorded(
    conn: &mut Connection,
    source_id: ImageId,
    before: &ParametricAdjustments,
    after: &ParametricAdjustments,
    target_ids: &[ImageId],
    options: &SyncDeltaOptions,
    as_shot: &HashMap<ImageId, WhiteBalanceValues>,
) -> AppResult<SyncDeltaResult> {
    before.validate().map_err(AppError::invalid)?;
    after.validate().map_err(AppError::invalid)?;
    let label = check_options(options)?;
    let fields = changed_fields(before, after, options.fields.as_deref());
    let relative_fields: Vec<AdjustmentField> =
        fields.iter().copied().filter(|f| options.relative.contains(f)).collect();
    // The source must exist even when nothing changed.
    repo::get_adjustments(conn, source_id)?;
    let mut items =
        vec![BatchItem { image_id: source_id, adjustments: after.clone(), scene_id: None, review_reason: None }];
    let mut absolute_wb_ids = Vec::new();
    if !fields.is_empty() {
        let source_as_shot = as_shot.get(&source_id).copied();
        for &id in target_ids {
            if items.iter().any(|it| it.image_id == id) {
                continue;
            }
            let current = repo::get_adjustments(conn, id)?;
            let (next, absolute) = apply_delta(
                &current,
                as_shot.get(&id).copied(),
                before,
                after,
                source_as_shot,
                &fields,
                &relative_fields,
            );
            if absolute {
                absolute_wb_ids.push(id);
            }
            items.push(BatchItem { image_id: id, adjustments: next, scene_id: None, review_reason: None });
        }
    } else {
        for &id in target_ids {
            repo::get_adjustments(conn, id)?;
        }
    }
    let batch = batches::commit_recorded(conn, &items, &label, BatchKind::Sync)?;
    let history = history::history(conn, source_id)?;
    Ok(SyncDeltaResult { batch, fields, relative_fields, absolute_wb_ids, history })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::ipc::error::ErrorKind;
    use crate::ipc::types::EditBatchKind;

    fn custom(temperature_k: f32, tint: f32) -> WhiteBalance {
        WhiteBalance::Custom { temperature_k, tint }
    }

    fn setup(n: i64) -> Connection {
        let conn = open_in_memory();
        conn.execute("INSERT INTO folders (id, path, added_at) VALUES (1, '/f', 0)", []).unwrap();
        for id in 1..=n {
            conn.execute(
                "INSERT INTO images (id, folder_id, path, file_name, format, camera_make, sensor_layout,
                                     file_size, file_mtime_ms, imported_at)
                 VALUES (?1, 1, '/f/' || ?1 || '.ARW', ?1 || '.ARW', 'arw', 'sony', 'bayer', 1, 0, 0)",
                [id],
            )
            .unwrap();
        }
        conn
    }

    fn save(conn: &mut Connection, id: ImageId, a: &ParametricAdjustments) {
        history::commit(conn, id, a, "Setup").unwrap();
    }

    #[test]
    fn changed_fields_finds_only_what_the_edit_changed() {
        let before = ParametricAdjustments::default();
        let mut after = before.clone();
        after.exposure = 0.5;
        after.hsl.hue.red = 10.0;
        after.crop.angle = 3.0;
        after.detail.noise_reduction.color = 40.0;
        assert_eq!(
            changed_fields(&before, &after, None),
            [AdjustmentField::Exposure, AdjustmentField::HslHue, AdjustmentField::NoiseReduction],
            "crop never syncs; noise-reduction subsets fold into noise_reduction"
        );
        assert_eq!(changed_fields(&before, &after, Some(&[AdjustmentField::HslHue])), [AdjustmentField::HslHue]);
        assert!(changed_fields(&before, &before, None).is_empty());
    }

    #[test]
    fn white_balance_shifts_in_mireds_and_resolves_as_shot() {
        let shot = WhiteBalanceValues { temperature_k: 5000.0, tint: 4.0 };
        // Source 5000 K -> 5500 K (-18.18 mired), tint +6.
        let (b, a) = (custom(5000.0, 0.0), custom(5500.0, 6.0));
        assert_eq!(shift_white_balance(custom(5000.0, 0.0), None, b, a, None), Some(custom(5500.0, 6.0)));
        let WhiteBalance::Custom { temperature_k, tint } =
            shift_white_balance(custom(3200.0, -10.0), None, b, a, None).unwrap()
        else {
            panic!()
        };
        assert_eq!(tint, -4.0);
        let expected: f32 = 1.0e6 / (1.0e6 / 3200.0 + 1.0e6 / 5500.0 - 1.0e6 / 5000.0);
        assert!((temperature_k - expected.round()).abs() < 0.5, "{temperature_k} vs {expected}");
        // As-shot target resolved with its own as-shot values.
        let r = shift_white_balance(WhiteBalance::AsShot, Some(shot), b, a, None).unwrap();
        assert_eq!(r, custom(5500.0, 10.0));
        // Unknown as-shot -> None (caller copies absolutely).
        assert_eq!(shift_white_balance(WhiteBalance::AsShot, None, b, a, None), None);
        // Source from as-shot: needs its as-shot values.
        assert_eq!(shift_white_balance(custom(4000.0, 0.0), None, WhiteBalance::AsShot, a, None), None);
        assert_eq!(
            shift_white_balance(custom(5000.0, 0.0), None, WhiteBalance::AsShot, a, Some(shot)),
            Some(custom(5500.0, 2.0))
        );
        // Back to as-shot is copied; unchanged WB leaves the target alone; clamps.
        assert_eq!(
            shift_white_balance(custom(4000.0, 0.0), None, b, WhiteBalance::AsShot, None),
            Some(WhiteBalance::AsShot)
        );
        assert_eq!(shift_white_balance(custom(4000.0, 3.0), None, b, b, None), Some(custom(4000.0, 3.0)));
        let hot = shift_white_balance(custom(45000.0, 148.0), None, custom(3000.0, 0.0), custom(50000.0, 20.0), None);
        assert_eq!(hot, Some(custom(MAX_TEMP, MAX_TINT)));
    }

    #[test]
    fn sync_delta_moves_exposure_relatively_and_copies_other_groups() {
        let mut conn = setup(4);
        let base = ParametricAdjustments::default();
        let t2 =
            ParametricAdjustments { exposure: 1.0, contrast: 30.0, white_balance: custom(4000.0, 5.0), ..base.clone() };
        let t3 = ParametricAdjustments { exposure: -4.8, white_balance: WhiteBalance::AsShot, ..base.clone() };
        save(&mut conn, 2, &t2);
        save(&mut conn, 3, &t3);
        let before = ParametricAdjustments { white_balance: custom(5000.0, 0.0), ..base.clone() };
        save(&mut conn, 1, &before);
        let after = ParametricAdjustments {
            exposure: -0.5,
            clarity: 12.0,
            white_balance: custom(5500.0, 6.0),
            ..before.clone()
        };
        let opts = SyncDeltaOptions::default();
        assert_eq!(as_shot_needed(&conn, 1, &before, &after, &[2, 3, 4], &opts).unwrap(), [3, 4]);
        let shot = HashMap::from([(3, WhiteBalanceValues { temperature_k: 5000.0, tint: 0.0 })]);
        let r = sync_delta_recorded(&mut conn, 1, &before, &after, &[2, 3, 4, 2, 1], &opts, &shot).unwrap();
        assert_eq!(r.fields, [AdjustmentField::WhiteBalance, AdjustmentField::Exposure, AdjustmentField::Clarity]);
        assert_eq!(r.relative_fields, [AdjustmentField::WhiteBalance, AdjustmentField::Exposure]);
        assert_eq!(r.absolute_wb_ids, [4], "photo 4 is as-shot with no known as-shot value");
        assert_eq!(r.batch.changed_ids, [1, 2, 3, 4]);
        assert_eq!(r.batch.label, "Auto Sync");
        assert_eq!(r.history.entries.last().unwrap().label, "Auto Sync");
        let g = |c: &Connection, id| repo::get_adjustments(c, id).unwrap();
        assert_eq!(g(&conn, 1), after);
        let a2 = g(&conn, 2);
        assert_eq!((a2.exposure, a2.contrast, a2.clarity), (0.5, 30.0, 12.0), "relative exposure, own contrast kept");
        assert!(matches!(a2.white_balance, WhiteBalance::Custom { tint, .. } if tint == 11.0));
        let a3 = g(&conn, 3);
        assert_eq!(a3.exposure, -5.0, "clamped");
        assert_eq!(a3.white_balance, custom(5500.0, 6.0));
        assert_eq!(g(&conn, 4).white_balance, custom(5500.0, 6.0), "copied absolutely");
        let info = batches::batch_info(&conn, r.batch.batch_id.unwrap()).unwrap();
        assert_eq!((info.kind, info.image_count, info.undoable), (EditBatchKind::Sync, 4, true));

        // One undo reverts the source and every target.
        batches::undo(&mut conn, r.batch.batch_id.unwrap()).unwrap();
        assert_eq!(g(&conn, 1), before);
        assert_eq!(g(&conn, 2), t2);
        assert_eq!(g(&conn, 3), t3);
        assert!(g(&conn, 4).is_neutral());
    }

    #[test]
    fn sync_delta_absolute_mode_options_and_errors() {
        let mut conn = setup(3);
        let before = ParametricAdjustments::default();
        let after = ParametricAdjustments { exposure: 0.7, vibrance: 15.0, ..before.clone() };
        save(&mut conn, 2, &ParametricAdjustments { exposure: 1.0, ..before.clone() });
        let opts = SyncDeltaOptions {
            relative: vec![],
            fields: Some(vec![AdjustmentField::Exposure]),
            label: Some("Exposure".into()),
        };
        let r = sync_delta_recorded(&mut conn, 1, &before, &after, &[2, 3], &opts, &HashMap::new()).unwrap();
        assert_eq!(r.fields, [AdjustmentField::Exposure]);
        assert!(r.relative_fields.is_empty());
        let a2 = repo::get_adjustments(&conn, 2).unwrap();
        assert_eq!((a2.exposure, a2.vibrance), (0.7, 0.0), "absolute copy, vibrance not listed");
        assert_eq!(repo::get_adjustments(&conn, 1).unwrap(), after, "the source always gets `after`");

        // Nothing changed: no batch.
        let r = sync_delta_recorded(&mut conn, 1, &after, &after, &[2], &SyncDeltaOptions::default(), &HashMap::new())
            .unwrap();
        assert_eq!((r.batch.batch_id, r.fields.len()), (None, 0));

        let bad = |conn: &mut Connection, o: SyncDeltaOptions, targets: &[ImageId]| {
            sync_delta_recorded(conn, 1, &before, &after, targets, &o, &HashMap::new()).unwrap_err().kind
        };
        let rel = SyncDeltaOptions { relative: vec![AdjustmentField::Contrast], ..Default::default() };
        assert_eq!(bad(&mut conn, rel, &[2]), ErrorKind::InvalidArgument);
        let label = SyncDeltaOptions { label: Some(String::new()), ..Default::default() };
        assert_eq!(bad(&mut conn, label, &[2]), ErrorKind::InvalidArgument);
        assert_eq!(bad(&mut conn, SyncDeltaOptions::default(), &[2, 99]), ErrorKind::NotFound);
        assert_eq!(repo::get_adjustments(&conn, 2).unwrap().exposure, 0.7, "atomic");
    }
}
