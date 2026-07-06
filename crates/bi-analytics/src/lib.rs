//! bi-analytics: 統計分析・回帰分析・クラスタリング。
//! 外部依存なしの純Rust実装(低スペックPC向けに軽量)。
//! NaN は欠損値として扱う。

use serde::Serialize;

// ---------- 記述統計 ----------

#[derive(Debug, Clone, Serialize)]
pub struct NumericStats {
    pub count: usize,
    pub mean: f64,
    pub std: f64,
    pub min: f64,
    pub q25: f64,
    pub median: f64,
    pub q75: f64,
    pub max: f64,
}

/// NaNを除外して基本統計量を計算する。有効値が無ければ None。
pub fn numeric_stats(values: &[f64]) -> Option<NumericStats> {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    let mean = v.iter().sum::<f64>() / n as f64;
    let var = if n > 1 {
        v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64
    } else {
        0.0
    };
    Some(NumericStats {
        count: n,
        mean,
        std: var.sqrt(),
        min: v[0],
        q25: quantile_sorted(&v, 0.25),
        median: quantile_sorted(&v, 0.5),
        q75: quantile_sorted(&v, 0.75),
        max: v[n - 1],
    })
}

fn quantile_sorted(sorted: &[f64], q: f64) -> f64 {
    let n = sorted.len();
    if n == 1 {
        return sorted[0];
    }
    let pos = q * (n - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    let frac = pos - lo as f64;
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

/// ピアソン相関係数(ペアワイズ完全観測)。有効ペアが2未満なら None。
pub fn pearson(x: &[f64], y: &[f64]) -> Option<f64> {
    let pairs: Vec<(f64, f64)> = x
        .iter()
        .zip(y.iter())
        .filter(|(a, b)| a.is_finite() && b.is_finite())
        .map(|(a, b)| (*a, *b))
        .collect();
    let n = pairs.len();
    if n < 2 {
        return None;
    }
    let mx = pairs.iter().map(|p| p.0).sum::<f64>() / n as f64;
    let my = pairs.iter().map(|p| p.1).sum::<f64>() / n as f64;
    let mut sxy = 0.0;
    let mut sxx = 0.0;
    let mut syy = 0.0;
    for (a, b) in &pairs {
        sxy += (a - mx) * (b - my);
        sxx += (a - mx) * (a - mx);
        syy += (b - my) * (b - my);
    }
    if sxx <= 0.0 || syy <= 0.0 {
        return None;
    }
    Some(sxy / (sxx * syy).sqrt())
}

// ---------- 回帰分析(OLS) ----------

#[derive(Debug, Clone, Serialize)]
pub struct OlsResult {
    /// [切片, 係数1, 係数2, ...]
    pub coef: Vec<f64>,
    /// 各係数の標準誤差(自由度不足時は空)
    pub stderr: Vec<f64>,
    /// 各係数のt値(自由度不足時は空)
    pub tvalues: Vec<f64>,
    pub r2: f64,
    pub adj_r2: f64,
    pub rmse: f64,
    pub n: usize,
    pub predicted: Vec<f64>,
}

/// 最小二乗法による線形回帰(切片あり)。
/// rows: 各行が説明変数ベクトル(NaNなし前提)、y: 目的変数。
pub fn ols(rows: &[Vec<f64>], y: &[f64]) -> Result<OlsResult, String> {
    let n = rows.len();
    if n == 0 || n != y.len() {
        return Err("データがありません".to_string());
    }
    let p = rows[0].len();
    if p == 0 {
        return Err("説明変数を指定してください".to_string());
    }
    if n <= p {
        return Err(format!("データ数({n})が説明変数の数({p})に対して不足しています"));
    }
    let dim = p + 1; // 切片分
    // 正規方程式 X'X b = X'y を構築
    let mut xtx = vec![vec![0.0f64; dim]; dim];
    let mut xty = vec![0.0f64; dim];
    for (row, &yv) in rows.iter().zip(y.iter()) {
        let mut xi = Vec::with_capacity(dim);
        xi.push(1.0);
        xi.extend_from_slice(row);
        for i in 0..dim {
            xty[i] += xi[i] * yv;
            for j in i..dim {
                xtx[i][j] += xi[i] * xi[j];
            }
        }
    }
    for i in 0..dim {
        for j in 0..i {
            xtx[i][j] = xtx[j][i];
        }
    }
    let inv = invert(&xtx).ok_or("説明変数間に完全な多重共線性があるか、値が一定の列があります")?;
    let coef: Vec<f64> = (0..dim)
        .map(|i| (0..dim).map(|j| inv[i][j] * xty[j]).sum())
        .collect();

    // 予測値・残差・決定係数
    let mean_y = y.iter().sum::<f64>() / n as f64;
    let mut predicted = Vec::with_capacity(n);
    let mut ssr = 0.0; // 残差平方和
    let mut sst = 0.0;
    for (row, &yv) in rows.iter().zip(y.iter()) {
        let mut pred = coef[0];
        for (j, v) in row.iter().enumerate() {
            pred += coef[j + 1] * v;
        }
        predicted.push(pred);
        ssr += (yv - pred) * (yv - pred);
        sst += (yv - mean_y) * (yv - mean_y);
    }
    let r2 = if sst > 0.0 { 1.0 - ssr / sst } else { 0.0 };
    let dof = n - dim;
    let adj_r2 = if dof > 0 && sst > 0.0 {
        1.0 - (ssr / dof as f64) / (sst / (n - 1) as f64)
    } else {
        r2
    };
    let rmse = (ssr / n as f64).sqrt();
    let (stderr, tvalues) = if dof > 0 {
        let sigma2 = ssr / dof as f64;
        let se: Vec<f64> = (0..dim).map(|i| (sigma2 * inv[i][i]).max(0.0).sqrt()).collect();
        let tv: Vec<f64> = coef
            .iter()
            .zip(se.iter())
            .map(|(c, s)| if *s > 0.0 { c / s } else { f64::NAN })
            .collect();
        (se, tv)
    } else {
        (vec![], vec![])
    };
    Ok(OlsResult {
        coef,
        stderr,
        tvalues,
        r2,
        adj_r2,
        rmse,
        n,
        predicted,
    })
}

/// ガウス・ジョルダン法による逆行列(部分ピボット)。特異なら None。
fn invert(a: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    let mut m: Vec<Vec<f64>> = a
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut r = row.clone();
            r.extend((0..n).map(|j| if i == j { 1.0 } else { 0.0 }));
            r
        })
        .collect();
    for col in 0..n {
        // ピボット選択
        let pivot = (col..n).max_by(|&i, &j| m[i][col].abs().partial_cmp(&m[j][col].abs()).unwrap())?;
        if m[pivot][col].abs() < 1e-12 {
            return None;
        }
        m.swap(col, pivot);
        let pv = m[col][col];
        for v in m[col].iter_mut() {
            *v /= pv;
        }
        for row in 0..n {
            if row != col && m[row][col].abs() > 0.0 {
                let factor = m[row][col];
                for j in 0..2 * n {
                    m[row][j] -= factor * m[col][j];
                }
            }
        }
    }
    Some(m.into_iter().map(|r| r[n..].to_vec()).collect())
}

// ---------- クラスタリング(k-means++) ----------

#[derive(Debug, Clone, Serialize)]
pub struct KMeansResult {
    pub assignments: Vec<usize>,
    /// 元スケールでのクラスタ中心 [k][特徴量]
    pub centroids: Vec<Vec<f64>>,
    pub sizes: Vec<usize>,
    /// 標準化空間での慣性(クラスタ内二乗和)
    pub inertia: f64,
    pub iterations: usize,
}

/// k-meansクラスタリング。特徴量は内部で標準化(z-score)する。
/// rows: 各行が特徴量ベクトル(NaNなし前提)。決定的(seed固定)。
pub fn kmeans(rows: &[Vec<f64>], k: usize, seed: u64) -> Result<KMeansResult, String> {
    let n = rows.len();
    if k < 2 {
        return Err("クラスタ数kは2以上を指定してください".to_string());
    }
    if n < k {
        return Err(format!("データ数({n})がクラスタ数({k})より少ないです"));
    }
    let dim = rows[0].len();
    if dim == 0 {
        return Err("特徴量を指定してください".to_string());
    }

    // 標準化(キャッシュ効率のためフラット配列 n×dim に格納)
    let mut means = vec![0.0f64; dim];
    let mut stds = vec![0.0f64; dim];
    for d in 0..dim {
        let mean = rows.iter().map(|r| r[d]).sum::<f64>() / n as f64;
        let var = rows.iter().map(|r| (r[d] - mean).powi(2)).sum::<f64>() / n as f64;
        means[d] = mean;
        stds[d] = if var.sqrt() > 1e-12 { var.sqrt() } else { 1.0 };
    }
    let mut data = Vec::with_capacity(n * dim);
    for r in rows {
        for d in 0..dim {
            data.push((r[d] - means[d]) / stds[d]);
        }
    }

    // 大規模データでは学習をサンプル(最大2万点)で行い、全点への割り当ては1回だけにする
    // (低スペックPC対策: Lloyd反復のコストを一定に抑える)
    const TRAIN_CAP: usize = 20_000;
    let sampled;
    let (train, n_train) = if n > TRAIN_CAP {
        let step = n.div_ceil(TRAIN_CAP);
        let mut s = Vec::with_capacity(TRAIN_CAP * dim);
        let mut i = 0;
        while i < n {
            s.extend_from_slice(&data[i * dim..(i + 1) * dim]);
            i += step;
        }
        let nt = s.len() / dim;
        sampled = s;
        (&sampled[..], nt)
    } else {
        (&data[..], n)
    };

    // リスタートして最良(慣性最小)を採用。サンプル学習時は1回で十分
    let restarts = if n > TRAIN_CAP { 1 } else { 3 };
    let mut best: Option<(Vec<f64>, f64, usize)> = None;
    for restart in 0..restarts as u64 {
        let r = kmeans_once(train, n_train, dim, k, seed.wrapping_add(restart));
        if best.as_ref().map(|b| r.1 < b.1).unwrap_or(true) {
            best = Some(r);
        }
    }
    let (centroids_flat, _, iterations) = best.unwrap();

    // 学習結果の中心で全点を割り当てる
    let mut assignments = vec![0usize; n];
    let mut sizes = vec![0usize; k];
    let mut inertia = 0.0;
    for i in 0..n {
        let row = &data[i * dim..(i + 1) * dim];
        let mut best_c = 0;
        let mut best_d = f64::MAX;
        for c in 0..k {
            let d = sq_dist(row, &centroids_flat[c * dim..(c + 1) * dim]);
            if d < best_d {
                best_d = d;
                best_c = c;
            }
        }
        assignments[i] = best_c;
        sizes[best_c] += 1;
        inertia += best_d;
    }

    // クラスタ中心を元スケールへ戻す
    let centroids: Vec<Vec<f64>> = (0..k)
        .map(|c| (0..dim).map(|d| centroids_flat[c * dim + d] * stds[d] + means[d]).collect())
        .collect();
    Ok(KMeansResult {
        assignments,
        centroids,
        sizes,
        inertia,
        iterations,
    })
}

/// Lloyd法1回分(フラット配列版)。(中心 k×dim, 慣性, 反復回数) を返す。
fn kmeans_once(data: &[f64], n: usize, dim: usize, k: usize, seed: u64) -> (Vec<f64>, f64, usize) {
    let mut rng = XorShift64::new(seed);
    let row = |i: usize| &data[i * dim..(i + 1) * dim];

    // k-means++ 初期化
    let mut centroids = Vec::with_capacity(k * dim);
    let first = (rng.next() as usize) % n;
    centroids.extend_from_slice(row(first));
    let mut dists: Vec<f64> = vec![f64::MAX; n];
    for c in 1..k {
        let last = &centroids[(c - 1) * dim..c * dim];
        let mut total = 0.0;
        for i in 0..n {
            let d = sq_dist(row(i), last);
            if d < dists[i] {
                dists[i] = d;
            }
            total += dists[i];
        }
        let mut target = rng.next_f64() * total;
        let mut chosen = n - 1;
        for (i, &d) in dists.iter().enumerate() {
            target -= d;
            if target <= 0.0 {
                chosen = i;
                break;
            }
        }
        centroids.extend_from_slice(row(chosen));
    }

    let mut sums = vec![0.0f64; k * dim];
    let mut counts = vec![0usize; k];
    let mut inertia = 0.0;
    let mut iterations = 0;
    for iter in 0..30 {
        iterations = iter + 1;
        // 割り当てと中心の合計を1パスで計算(最遠点も空クラスタ対策に記録)
        sums.iter_mut().for_each(|x| *x = 0.0);
        counts.iter_mut().for_each(|x| *x = 0);
        inertia = 0.0;
        let mut far_i = 0;
        let mut far_d = -1.0f64;
        for i in 0..n {
            let r = row(i);
            let mut best_c = 0;
            let mut best_d = f64::MAX;
            for c in 0..k {
                let d = sq_dist(r, &centroids[c * dim..(c + 1) * dim]);
                if d < best_d {
                    best_d = d;
                    best_c = c;
                }
            }
            counts[best_c] += 1;
            for d in 0..dim {
                sums[best_c * dim + d] += r[d];
            }
            inertia += best_d;
            if best_d > far_d {
                far_d = best_d;
                far_i = i;
            }
        }
        // 中心更新と移動量チェック
        let mut max_shift = 0.0f64;
        for c in 0..k {
            if counts[c] == 0 {
                // 空クラスタは最遠点で再初期化
                centroids[c * dim..(c + 1) * dim].copy_from_slice(row(far_i));
                max_shift = f64::MAX;
            } else {
                let mut shift = 0.0;
                for d in 0..dim {
                    let nv = sums[c * dim + d] / counts[c] as f64;
                    let old = centroids[c * dim + d];
                    shift += (nv - old) * (nv - old);
                    centroids[c * dim + d] = nv;
                }
                if shift > max_shift {
                    max_shift = shift;
                }
            }
        }
        // 中心がほぼ動かなくなったら収束(一様データでの無限微調整を打ち切る)
        if max_shift < 1e-8 {
            break;
        }
    }
    (centroids, inertia, iterations)
}

fn sq_dist(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y) * (x - y)).sum()
}

struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        XorShift64 {
            state: seed.wrapping_mul(2685821657736338717).max(1),
        }
    }
    fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }
    fn next_f64(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_numeric_stats() {
        let s = numeric_stats(&[1.0, 2.0, 3.0, 4.0, f64::NAN]).unwrap();
        assert_eq!(s.count, 4);
        assert!((s.mean - 2.5).abs() < 1e-9);
        assert!((s.median - 2.5).abs() < 1e-9);
        assert_eq!(s.min, 1.0);
        assert_eq!(s.max, 4.0);
    }

    #[test]
    fn test_pearson() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        let y = [2.0, 4.0, 6.0, 8.0, 10.0];
        assert!((pearson(&x, &y).unwrap() - 1.0).abs() < 1e-9);
        let y_neg = [10.0, 8.0, 6.0, 4.0, 2.0];
        assert!((pearson(&x, &y_neg).unwrap() + 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_ols_simple() {
        // y = 3 + 2x + ノイズなし
        let rows: Vec<Vec<f64>> = (0..10).map(|i| vec![i as f64]).collect();
        let y: Vec<f64> = (0..10).map(|i| 3.0 + 2.0 * i as f64).collect();
        let r = ols(&rows, &y).unwrap();
        assert!((r.coef[0] - 3.0).abs() < 1e-8);
        assert!((r.coef[1] - 2.0).abs() < 1e-8);
        assert!(r.r2 > 0.9999);
    }

    #[test]
    fn test_ols_multiple() {
        // y = 1 + 2a - 3b
        let mut rows = Vec::new();
        let mut y = Vec::new();
        for a in 0..6 {
            for b in 0..6 {
                rows.push(vec![a as f64, b as f64]);
                y.push(1.0 + 2.0 * a as f64 - 3.0 * b as f64);
            }
        }
        let r = ols(&rows, &y).unwrap();
        assert!((r.coef[0] - 1.0).abs() < 1e-8);
        assert!((r.coef[1] - 2.0).abs() < 1e-8);
        assert!((r.coef[2] + 3.0).abs() < 1e-8);
    }

    #[test]
    fn test_ols_collinear() {
        // b = 2a は完全共線 → エラー
        let rows: Vec<Vec<f64>> = (0..10).map(|i| vec![i as f64, 2.0 * i as f64]).collect();
        let y: Vec<f64> = (0..10).map(|i| i as f64).collect();
        assert!(ols(&rows, &y).is_err());
    }

    #[test]
    fn test_kmeans_separated() {
        // 明確に分離した3クラスタ
        let mut rows = Vec::new();
        for i in 0..30 {
            let offset = (i % 3) as f64 * 100.0;
            rows.push(vec![offset + (i / 3) as f64 * 0.1, offset + (i / 3) as f64 * 0.1]);
        }
        let r = kmeans(&rows, 3, 42).unwrap();
        assert_eq!(r.sizes.iter().sum::<usize>(), 30);
        assert_eq!(r.sizes, vec![10, 10, 10].into_iter().collect::<Vec<_>>());
        // 同じオフセットの点は同じクラスタに入る
        for i in 0..30 {
            assert_eq!(r.assignments[i], r.assignments[i % 3]);
        }
    }

    #[test]
    fn test_kmeans_deterministic() {
        let rows: Vec<Vec<f64>> = (0..50).map(|i| vec![(i % 7) as f64, (i % 11) as f64]).collect();
        let a = kmeans(&rows, 3, 7).unwrap();
        let b = kmeans(&rows, 3, 7).unwrap();
        assert_eq!(a.assignments, b.assignments);
    }
}
