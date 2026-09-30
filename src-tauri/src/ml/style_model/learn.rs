//! Small pure-Rust learners: feature standardization with median imputation, ridge
//! regression (Cholesky) and gradient-boosted regression trees (histogram splits, L2 loss)
//! fitted on the ridge residuals. Deterministic; sized for hundreds to a few thousand
//! samples and ~60 features.

use serde::{Deserialize, Serialize};

/// Median imputation + z-scoring, fitted on the training rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Standardizer {
    pub fill: Vec<f32>,
    pub mean: Vec<f32>,
    pub scale: Vec<f32>,
}

impl Standardizer {
    pub fn fit(rows: &[Vec<f32>]) -> Standardizer {
        let d = rows.first().map_or(0, Vec::len);
        let mut fill = vec![0.0; d];
        let mut mean = vec![0.0; d];
        let mut scale = vec![1.0; d];
        for j in 0..d {
            let mut v: Vec<f32> = rows.iter().map(|r| r[j]).filter(|x| x.is_finite()).collect();
            if v.is_empty() {
                continue;
            }
            v.sort_by(f32::total_cmp);
            fill[j] = v[v.len() / 2];
            let n = rows.len() as f64;
            let m = rows.iter().map(|r| f64::from(if r[j].is_finite() { r[j] } else { fill[j] })).sum::<f64>() / n;
            let var = rows
                .iter()
                .map(|r| {
                    let x = f64::from(if r[j].is_finite() { r[j] } else { fill[j] }) - m;
                    x * x
                })
                .sum::<f64>()
                / n;
            mean[j] = m as f32;
            scale[j] = if var > 1e-12 { var.sqrt() as f32 } else { 1.0 };
        }
        Standardizer { fill, mean, scale }
    }

    pub fn apply(&self, row: &[f32]) -> Vec<f32> {
        row.iter()
            .enumerate()
            .map(|(j, &x)| {
                let x = if x.is_finite() { x } else { self.fill.get(j).copied().unwrap_or(0.0) };
                (x - self.mean.get(j).copied().unwrap_or(0.0)) / self.scale.get(j).copied().unwrap_or(1.0)
            })
            .collect()
    }
}

/// `y = w . x + b` on standardized features.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ridge {
    pub w: Vec<f32>,
    pub b: f32,
}

impl Ridge {
    /// Least squares with an L2 penalty `lambda * n` on `w` (intercept unpenalized).
    pub fn fit(x: &[Vec<f32>], y: &[f32], lambda: f64) -> Ridge {
        Self::fit_weighted(x, y, &vec![1.0; y.len()], lambda)
    }

    /// Weighted least squares (`sum w_i (y_i - f(x_i))^2 + lambda * sum(w) * |w|^2`).
    pub fn fit_weighted(x: &[Vec<f32>], y: &[f32], wt: &[f32], lambda: f64) -> Ridge {
        let d = x.first().map_or(0, Vec::len);
        let sw: f64 = wt.iter().map(|v| f64::from(*v)).sum();
        if x.is_empty() || sw <= 0.0 {
            return Ridge { w: vec![0.0; d], b: 0.0 };
        }
        let ym = y.iter().zip(wt).map(|(v, w)| f64::from(*v) * f64::from(*w)).sum::<f64>() / sw;
        let mut xm = vec![0.0f64; d];
        for (r, w) in x.iter().zip(wt) {
            for (m, v) in xm.iter_mut().zip(r) {
                *m += f64::from(*v) * f64::from(*w) / sw;
            }
        }
        let mut a = vec![0.0f64; d * d];
        let mut rhs = vec![0.0f64; d];
        for ((r, &t), &w) in x.iter().zip(y).zip(wt) {
            let w = f64::from(w);
            let c: Vec<f64> = r.iter().zip(&xm).map(|(v, m)| f64::from(*v) - m).collect();
            let yt = f64::from(t) - ym;
            for i in 0..d {
                rhs[i] += w * c[i] * yt;
                for j in 0..=i {
                    a[i * d + j] += w * c[i] * c[j];
                }
            }
        }
        for i in 0..d {
            for j in 0..i {
                a[j * d + i] = a[i * d + j];
            }
            a[i * d + i] += lambda * sw + 1e-9;
        }
        let w = cholesky_solve(&mut a, &rhs, d).unwrap_or_else(|| vec![0.0; d]);
        let b = ym - w.iter().zip(&xm).map(|(w, m)| w * m).sum::<f64>();
        Ridge { w: w.iter().map(|v| *v as f32).collect(), b: b as f32 }
    }

    pub fn predict(&self, x: &[f32]) -> f32 {
        self.b + self.w.iter().zip(x).map(|(w, v)| w * v).sum::<f32>()
    }
}

/// Solves `A x = b` for symmetric positive definite `A` (row-major `d x d`, overwritten).
fn cholesky_solve(a: &mut [f64], b: &[f64], d: usize) -> Option<Vec<f64>> {
    for j in 0..d {
        let mut s = a[j * d + j];
        for k in 0..j {
            s -= a[j * d + k] * a[j * d + k];
        }
        if s <= 0.0 {
            return None;
        }
        let l = s.sqrt();
        a[j * d + j] = l;
        for i in j + 1..d {
            let mut s = a[i * d + j];
            for k in 0..j {
                s -= a[i * d + k] * a[j * d + k];
            }
            a[i * d + j] = s / l;
        }
    }
    let mut z = vec![0.0; d];
    for i in 0..d {
        let mut s = b[i];
        for k in 0..i {
            s -= a[i * d + k] * z[k];
        }
        z[i] = s / a[i * d + i];
    }
    let mut x = vec![0.0; d];
    for i in (0..d).rev() {
        let mut s = z[i];
        for k in i + 1..d {
            s -= a[k * d + i] * x[k];
        }
        x[i] = s / a[i * d + i];
    }
    Some(x)
}

/// Tree node: leaf if `feature == u16::MAX`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub feature: u16,
    pub threshold: f32,
    pub left: u32,
    pub right: u32,
    pub value: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tree {
    pub nodes: Vec<Node>,
}

impl Tree {
    pub fn predict(&self, x: &[f32]) -> f32 {
        let mut i = 0usize;
        loop {
            let Some(n) = self.nodes.get(i) else { return 0.0 };
            if n.feature == u16::MAX {
                return n.value;
            }
            i = if x.get(n.feature as usize).copied().unwrap_or(0.0) <= n.threshold {
                n.left as usize
            } else {
                n.right as usize
            };
        }
    }
}

/// Boosting hyper-parameters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GbdtParams {
    pub max_rounds: usize,
    pub depth: usize,
    pub min_leaf: usize,
    pub learning_rate: f32,
    /// Leaf L2 shrinkage (pseudo-count).
    pub leaf_l2: f32,
    pub bins: usize,
}

impl Default for GbdtParams {
    fn default() -> Self {
        Self { max_rounds: 300, depth: 3, min_leaf: 8, learning_rate: 0.05, leaf_l2: 4.0, bins: 32 }
    }
}

/// Split candidates per feature (quantile thresholds of the training values).
pub struct Binned {
    thresholds: Vec<Vec<f32>>,
    /// `codes[i * d + j]` = bin of row i, feature j (value <= thresholds[j][code] or last).
    codes: Vec<u8>,
    d: usize,
}

impl Binned {
    pub fn new(x: &[Vec<f32>], bins: usize) -> Binned {
        let d = x.first().map_or(0, Vec::len);
        let bins = bins.clamp(2, 255);
        let mut thresholds = Vec::with_capacity(d);
        for j in 0..d {
            let mut v: Vec<f32> = x.iter().map(|r| r[j]).collect();
            v.sort_by(f32::total_cmp);
            v.dedup();
            let mut t: Vec<f32> = if v.len() <= bins {
                v.windows(2).map(|w| 0.5 * (w[0] + w[1])).collect()
            } else {
                (1..bins)
                    .map(|k| {
                        let i = k * (v.len() - 1) / bins;
                        0.5 * (v[i] + v[i + 1])
                    })
                    .collect()
            };
            t.dedup();
            thresholds.push(t);
        }
        let mut codes = vec![0u8; x.len() * d];
        for (i, r) in x.iter().enumerate() {
            for j in 0..d {
                codes[i * d + j] = thresholds[j].partition_point(|t| *t < r[j]) as u8;
            }
        }
        Binned { thresholds, codes, d }
    }
}

/// Fits one tree to `grad` (residuals) over the rows `idx`, rows weighted by `wt`
/// (normalized to mean 1).
fn fit_tree(b: &Binned, grad: &[f32], wt: &[f32], idx: &[usize], p: &GbdtParams) -> Tree {
    let mut nodes = Vec::new();
    grow(b, grad, wt, idx.to_vec(), 0, p, &mut nodes);
    Tree { nodes }
}

fn leaf(sum: f64, w: f64, p: &GbdtParams) -> f32 {
    (sum / (w + f64::from(p.leaf_l2))) as f32 * p.learning_rate
}

fn grow(
    b: &Binned,
    grad: &[f32],
    wt: &[f32],
    idx: Vec<usize>,
    depth: usize,
    p: &GbdtParams,
    nodes: &mut Vec<Node>,
) -> u32 {
    let me = nodes.len() as u32;
    let total: f64 = idx.iter().map(|&i| f64::from(grad[i] * wt[i])).sum();
    let wsum: f64 = idx.iter().map(|&i| f64::from(wt[i])).sum();
    nodes.push(Node { feature: u16::MAX, threshold: 0.0, left: 0, right: 0, value: leaf(total, wsum, p) });
    if depth >= p.depth || idx.len() < 2 * p.min_leaf {
        return me;
    }
    let n = idx.len();
    let l2 = f64::from(p.leaf_l2);
    let base = total * total / (wsum + l2);
    let mut best: Option<(f64, usize, usize)> = None;
    for j in 0..b.d {
        let nb = b.thresholds[j].len() + 1;
        if nb < 2 {
            continue;
        }
        let mut s = vec![0.0f64; nb];
        let mut sw = vec![0.0f64; nb];
        let mut c = vec![0usize; nb];
        for &i in &idx {
            let k = b.codes[i * b.d + j] as usize;
            s[k] += f64::from(grad[i] * wt[i]);
            sw[k] += f64::from(wt[i]);
            c[k] += 1;
        }
        let (mut sl, mut wl, mut cl) = (0.0f64, 0.0f64, 0usize);
        for k in 0..nb - 1 {
            sl += s[k];
            wl += sw[k];
            cl += c[k];
            let cr = n - cl;
            if cl < p.min_leaf || cr < p.min_leaf {
                continue;
            }
            let sr = total - sl;
            let gain = sl * sl / (wl + l2) + sr * sr / (wsum - wl + l2) - base;
            if gain > 1e-12 && best.is_none_or(|bst| gain > bst.0) {
                best = Some((gain, j, k));
            }
        }
    }
    let Some((_, j, k)) = best else { return me };
    let (li, ri): (Vec<usize>, Vec<usize>) = idx.iter().partition(|&&i| (b.codes[i * b.d + j] as usize) <= k);
    let left = grow(b, grad, wt, li, depth + 1, p, nodes);
    let right = grow(b, grad, wt, ri, depth + 1, p, nodes);
    nodes[me as usize] =
        Node { feature: j as u16, threshold: b.thresholds[j][k], left, right, value: nodes[me as usize].value };
    me
}

/// Boosted trees on residual targets.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Gbdt {
    pub trees: Vec<Tree>,
}

impl Gbdt {
    /// Fits up to `rounds` trees on `y`; `on_round(round, &current_predictions_of_eval_rows)`
    /// lets cross-validation read the validation curve without refitting.
    pub fn fit(
        x: &[Vec<f32>],
        y: &[f32],
        wt: &[f32],
        rounds: usize,
        p: &GbdtParams,
        eval: &[Vec<f32>],
        mut on_round: impl FnMut(usize, &[f32]),
    ) -> Gbdt {
        let b = Binned::new(x, p.bins);
        let mean_w = wt.iter().sum::<f32>() / wt.len().max(1) as f32;
        let wt: Vec<f32> = wt.iter().map(|w| w / mean_w.max(1e-9)).collect();
        let idx: Vec<usize> = (0..x.len()).collect();
        let mut pred = vec![0.0f32; x.len()];
        let mut eval_pred = vec![0.0f32; eval.len()];
        let mut trees = Vec::with_capacity(rounds);
        for r in 0..rounds {
            let grad: Vec<f32> = y.iter().zip(&pred).map(|(t, p)| t - p).collect();
            let tree = fit_tree(&b, &grad, &wt, &idx, p);
            if tree.nodes.len() <= 1 && tree.nodes.first().is_none_or(|n| n.value.abs() < 1e-9) {
                break;
            }
            for (i, row) in x.iter().enumerate() {
                pred[i] += tree.predict(row);
            }
            for (i, row) in eval.iter().enumerate() {
                eval_pred[i] += tree.predict(row);
            }
            trees.push(tree);
            on_round(r + 1, &eval_pred);
        }
        Gbdt { trees }
    }

    pub fn predict(&self, x: &[f32]) -> f32 {
        self.trees.iter().map(|t| t.predict(x)).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ridge_recovers_a_linear_function() {
        let x: Vec<Vec<f32>> = (0..50).map(|i| vec![i as f32 / 10.0, ((i * 7) % 11) as f32]).collect();
        let y: Vec<f32> = x.iter().map(|r| 2.0 * r[0] - 0.5 * r[1] + 3.0).collect();
        let m = Ridge::fit(&x, &y, 1e-6);
        assert!((m.w[0] - 2.0).abs() < 1e-3 && (m.w[1] + 0.5).abs() < 1e-3 && (m.b - 3.0).abs() < 1e-2, "{m:?}");
    }

    #[test]
    fn boosted_trees_fit_a_step() {
        let x: Vec<Vec<f32>> = (0..200).map(|i| vec![i as f32, (i % 3) as f32]).collect();
        let y: Vec<f32> = x.iter().map(|r| if r[0] < 100.0 { -10.0 } else { 10.0 }).collect();
        let p = GbdtParams { learning_rate: 0.3, leaf_l2: 0.0, ..Default::default() };
        let g = Gbdt::fit(&x, &y, &[1.0; 200], 60, &p, &[], |_, _| {});
        assert!((g.predict(&[20.0, 0.0]) + 10.0).abs() < 0.1);
        assert!((g.predict(&[180.0, 1.0]) - 10.0).abs() < 0.1);
    }

    #[test]
    fn standardizer_imputes_missing_values_with_the_median() {
        let rows = vec![vec![1.0, f32::NAN], vec![2.0, 4.0], vec![3.0, 6.0], vec![f32::NAN, 8.0]];
        let s = Standardizer::fit(&rows);
        assert_eq!(s.fill, vec![2.0, 6.0]);
        let z = s.apply(&[f32::NAN, f32::NAN]);
        assert!(z.iter().all(|v| v.is_finite()));
    }
}
