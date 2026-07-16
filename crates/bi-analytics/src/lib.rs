//! bi-analytics: 統計分析・回帰分析・クラスタリング・統計検定。
//! 外部依存なしの純Rust実装(低スペックPC向けに軽量)。
//! NaN は欠損値として扱う。

pub mod advisor;
pub mod distributions;
pub mod htest;

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
#[allow(clippy::needless_range_loop)] // 正規方程式の行列演算でインデックスを直接使う
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
        return Err(format!(
            "データ数({n})が説明変数の数({p})に対して不足しています"
        ));
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
        let se: Vec<f64> = (0..dim)
            .map(|i| (sigma2 * inv[i][i]).max(0.0).sqrt())
            .collect();
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
#[allow(clippy::needless_range_loop)] // ガウス・ジョルダン法で行・列を直接インデックスする
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
        let pivot =
            (col..n).max_by(|&i, &j| m[i][col].abs().partial_cmp(&m[j][col].abs()).unwrap())?;
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
        .map(|c| {
            (0..dim)
                .map(|d| centroids_flat[c * dim + d] * stds[d] + means[d])
                .collect()
        })
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
#[allow(clippy::needless_range_loop)] // フラット配列を i*dim..でスライスするため
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

// ---------- エルボー法(クラスタ数kの自動提案) ----------

/// エルボー法の結果。k=1..=k_max の慣性系列と提案k
#[derive(Debug, Clone, Serialize)]
pub struct ElbowResult {
    pub ks: Vec<usize>,
    /// 標準化空間での慣性(クラスタ内二乗和)。kに対して減少していく
    pub inertias: Vec<f64>,
    pub suggested_k: usize,
}

/// エルボー法: k=1..=k_max で k-means を実行し、慣性の減少が緩やかになる
/// 「肘」の位置を提案する。決定的(seed固定)。
/// rows は kmeans と同じ前提(各行が特徴量ベクトル、NaNなし)。
pub fn elbow(rows: &[Vec<f64>], k_max: usize, seed: u64) -> Result<ElbowResult, String> {
    let n = rows.len();
    if n < 2 {
        return Err("データ数が少なすぎます(2行以上必要)".to_string());
    }
    let dim = rows[0].len();
    if dim == 0 {
        return Err("特徴量を指定してください".to_string());
    }
    let k_max = k_max.clamp(2, 20).min(n);

    // k=1 の慣性は標準化空間での全体二乗和。z-score(分母n)の性質から
    // 「非定数列の数 × n」に一致するため、k-means を回さず直接求める
    let mut nonconst = 0usize;
    for d in 0..dim {
        let mean = rows.iter().map(|r| r[d]).sum::<f64>() / n as f64;
        let var = rows.iter().map(|r| (r[d] - mean).powi(2)).sum::<f64>() / n as f64;
        if var.sqrt() > 1e-12 {
            nonconst += 1;
        }
    }
    let mut ks = vec![1usize];
    let mut inertias = vec![(n * nonconst) as f64];
    for k in 2..=k_max {
        ks.push(k);
        inertias.push(kmeans(rows, k, seed)?.inertia);
    }
    let suggested_k = suggest_elbow(&ks, &inertias);
    Ok(ElbowResult {
        ks,
        inertias,
        suggested_k,
    })
}

/// 「肘」の検出: 曲線を[0,1]に正規化し、両端を結ぶ弦 y=1-x から
/// 最も下に離れた点を選ぶ(Kneedle法の簡易版)。同値なら小さいkを優先。
fn suggest_elbow(ks: &[usize], inertias: &[f64]) -> usize {
    let m = ks.len();
    if m < 3 {
        return ks[m - 1]; // 点が2つ以下では曲線にならないため最大kを返す
    }
    let span = inertias[0] - inertias[m - 1];
    if span <= 1e-12 {
        return ks[1]; // 慣性が下がらない=クラスタ構造なし。最小の2を返す
    }
    let mut best_i = 1;
    let mut best_d = f64::MIN;
    for i in 1..m - 1 {
        let x = (ks[i] - ks[0]) as f64 / (ks[m - 1] - ks[0]) as f64;
        let y = (inertias[i] - inertias[m - 1]) / span;
        let d = 1.0 - x - y; // 弦からの下方距離(ユークリッド距離の√2倍に比例)
        if d > best_d {
            best_d = d;
            best_i = i;
        }
    }
    ks[best_i]
}

fn sq_dist(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y) * (x - y)).sum()
}

// ---------- 時系列分解(古典的分解) ----------

/// 時系列分解の結果。系列は入力と同じ長さで、移動平均が計算できない
/// 両端の trend / residual は NaN になる。
#[derive(Debug, Clone, Serialize)]
pub struct DecomposeResult {
    pub trend: Vec<f64>,
    pub seasonal: Vec<f64>,
    pub residual: Vec<f64>,
    /// 1周期分の季節パターン(位相 0..period-1)。加法は平均0、乗法は平均1に正規化
    pub seasonal_pattern: Vec<f64>,
    /// トレンド強度 Ft = max(0, 1 - Var(R)/Var(T+R))。0〜1
    pub trend_strength: f64,
    /// 季節性強度 Fs = max(0, 1 - Var(R)/Var(S+R))。0〜1
    pub seasonal_strength: f64,
}

/// 古典的分解: 観測値をトレンド・季節成分・残差に分ける。
/// y は等間隔の系列(NaNなし)、period は季節の周期(日次の週周期=7 など)。
/// multiplicative=true で乗法モデル(y = T×S×R、正の値のみ)。
pub fn decompose(
    y: &[f64],
    period: usize,
    multiplicative: bool,
) -> Result<DecomposeResult, String> {
    let n = y.len();
    if period < 2 {
        return Err("周期は2以上を指定してください".to_string());
    }
    if n < period * 2 {
        return Err(format!(
            "データ数({n})が不足しています(周期{period}の2倍以上必要)"
        ));
    }
    if y.iter().any(|v| !v.is_finite()) {
        return Err("欠損を含む系列は分解できません(事前に除外・補完してください)".to_string());
    }
    if multiplicative && y.iter().any(|&v| v <= 0.0) {
        return Err(
            "乗法モデルは正の値の系列のみ対応です(0以下を含む場合は加法モデルを使用してください)"
                .to_string(),
        );
    }

    // トレンド: 中心化移動平均。周期が偶数のときは両端を半分の重みにして
    // 中心を合わせる(2×m移動平均)。計算できない両端は NaN のまま。
    let half = period / 2;
    let mut trend = vec![f64::NAN; n];
    for i in half..n - half {
        trend[i] = if period % 2 == 1 {
            y[i - half..=i + half].iter().sum::<f64>() / period as f64
        } else {
            let inner: f64 = y[i - half + 1..i + half].iter().sum();
            (0.5 * y[i - half] + inner + 0.5 * y[i + half]) / period as f64
        };
    }

    // 季節成分: トレンド除去後の値を位相(i % period)ごとに平均する
    let mut sums = vec![0.0f64; period];
    let mut counts = vec![0usize; period];
    for i in 0..n {
        if trend[i].is_finite() {
            let d = if multiplicative {
                y[i] / trend[i]
            } else {
                y[i] - trend[i]
            };
            sums[i % period] += d;
            counts[i % period] += 1;
        }
    }
    let mut pattern: Vec<f64> = (0..period)
        .map(|p| {
            if counts[p] > 0 {
                sums[p] / counts[p] as f64
            } else {
                0.0
            }
        })
        .collect();
    // 正規化: 季節成分の合計がトレンドに混ざらないよう、加法は平均0・乗法は平均1にする
    let mean = pattern.iter().sum::<f64>() / period as f64;
    for v in pattern.iter_mut() {
        if multiplicative {
            *v /= if mean.abs() > 1e-12 { mean } else { 1.0 };
        } else {
            *v -= mean;
        }
    }

    let seasonal: Vec<f64> = (0..n).map(|i| pattern[i % period]).collect();
    let residual: Vec<f64> = (0..n)
        .map(|i| {
            if !trend[i].is_finite() {
                f64::NAN
            } else if multiplicative {
                y[i] / (trend[i] * seasonal[i])
            } else {
                y[i] - trend[i] - seasonal[i]
            }
        })
        .collect();

    // 強度指標(Hyndman)。乗法モデルでも比較可能にするため、加法的な
    // 等価残差 R = y - T×S で統一して分散比を取る
    let mut tr = Vec::new(); // T + R
    let mut sr = Vec::new(); // S + R
    let mut rr = Vec::new(); // R
    for i in 0..n {
        if !trend[i].is_finite() {
            continue;
        }
        let (s_add, r_add) = if multiplicative {
            (
                trend[i] * seasonal[i] - trend[i],
                y[i] - trend[i] * seasonal[i],
            )
        } else {
            (seasonal[i], residual[i])
        };
        tr.push(trend[i] + r_add);
        sr.push(s_add + r_add);
        rr.push(r_add);
    }
    let strength = |denom: &[f64]| {
        let vd = variance(denom);
        if vd < 1e-12 {
            0.0
        } else {
            (1.0 - variance(&rr) / vd).max(0.0)
        }
    };
    let trend_strength = strength(&tr);
    let seasonal_strength = strength(&sr);

    Ok(DecomposeResult {
        trend,
        seasonal,
        residual,
        seasonal_pattern: pattern,
        trend_strength,
        seasonal_strength,
    })
}

// ---------- SPC管理図(I管理図 + ネルソンルール) ----------

/// 管理図のルール違反。rule: 1=3σ超え / 2=9点連続同側 / 3=6点連続増減
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpcViolation {
    pub index: usize,
    pub rule: u8,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpcResult {
    /// 中心線(平均)
    pub center: f64,
    /// 移動範囲法によるσ推定(MR̄ / 1.128)
    pub sigma: f64,
    pub ucl: f64,
    pub lcl: f64,
    pub violations: Vec<SpcViolation>,
}

/// I管理図(個々の測定値の管理図)。
/// σは標本標準偏差ではなく移動範囲(MR)法で推定する。標本標準偏差は
/// 工程の平均シフトまで「ばらつき」に含めてしまい、管理限界が不当に
/// 広がって異常を見逃すため(SPCの定石)。
/// y は時系列順の測定値(NaNなし)。
pub fn spc(y: &[f64]) -> Result<SpcResult, String> {
    let n = y.len();
    if n < 8 {
        return Err(format!(
            "データ数({n})が不足しています(管理図には8点以上必要)"
        ));
    }
    if y.iter().any(|v| !v.is_finite()) {
        return Err("欠損を含む系列は管理図にできません(事前に除外・補完してください)".to_string());
    }
    let center = y.iter().sum::<f64>() / n as f64;
    // 移動範囲の平均。d2定数(部分群サイズ2)= 1.128 で割ってσを推定する
    let mr_bar = y.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (n - 1) as f64;
    let sigma = mr_bar / 1.128;
    let ucl = center + 3.0 * sigma;
    let lcl = center - 3.0 * sigma;

    let mut violations = Vec::new();
    // ルール1: 管理限界(±3σ)の外
    for (i, &v) in y.iter().enumerate() {
        if v > ucl || v < lcl {
            violations.push(SpcViolation { index: i, rule: 1 });
        }
    }
    // ルール2: 9点連続で中心線の同じ側(中心線上の点は連続を断ち切る)
    let mut run_side = 0i8; // +1 / -1 / 0
    let mut run_len = 0usize;
    for (i, &v) in y.iter().enumerate() {
        let side = if v > center {
            1
        } else if v < center {
            -1
        } else {
            0
        };
        if side != 0 && side == run_side {
            run_len += 1;
        } else {
            run_side = side;
            run_len = if side == 0 { 0 } else { 1 };
        }
        if run_len >= 9 {
            violations.push(SpcViolation { index: i, rule: 2 });
        }
    }
    // ルール3: 6点連続で増加または減少(5回連続の同方向変化)
    let mut dir = 0i8;
    let mut steps = 0usize;
    for i in 1..n {
        let d = if y[i] > y[i - 1] {
            1
        } else if y[i] < y[i - 1] {
            -1
        } else {
            0
        };
        if d != 0 && d == dir {
            steps += 1;
        } else {
            dir = d;
            steps = if d == 0 { 0 } else { 1 };
        }
        if steps >= 5 {
            violations.push(SpcViolation { index: i, rule: 3 });
        }
    }
    Ok(SpcResult {
        center,
        sigma,
        ucl,
        lcl,
        violations,
    })
}

/// 母分散
fn variance(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / v.len() as f64
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
            rows.push(vec![
                offset + (i / 3) as f64 * 0.1,
                offset + (i / 3) as f64 * 0.1,
            ]);
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
        let rows: Vec<Vec<f64>> = (0..50)
            .map(|i| vec![(i % 7) as f64, (i % 11) as f64])
            .collect();
        let a = kmeans(&rows, 3, 7).unwrap();
        let b = kmeans(&rows, 3, 7).unwrap();
        assert_eq!(a.assignments, b.assignments);
    }

    #[test]
    fn test_elbow_three_clusters() {
        // 明確に分離した3クラスタ → k=3 が提案されるはず
        let mut rows = Vec::new();
        for i in 0..60 {
            let offset = (i % 3) as f64 * 100.0;
            rows.push(vec![
                offset + (i / 3) as f64 * 0.1,
                offset - (i / 3) as f64 * 0.1,
            ]);
        }
        let r = elbow(&rows, 10, 42).unwrap();
        assert_eq!(r.suggested_k, 3);
        assert_eq!(r.ks, (1..=10).collect::<Vec<_>>());
        // 慣性は k に対して(ほぼ)単調減少する
        for i in 1..r.inertias.len() {
            assert!(r.inertias[i] <= r.inertias[i - 1] + 1e-9);
        }
        // k=1 の慣性 = 非定数列数 × n = 2 × 60
        assert!((r.inertias[0] - 120.0).abs() < 1e-9);
    }

    #[test]
    fn test_elbow_deterministic_and_clamped() {
        let rows: Vec<Vec<f64>> = (0..5).map(|i| vec![i as f64]).collect();
        // k_max=50 でもデータ数5に切り詰められる
        let a = elbow(&rows, 50, 42).unwrap();
        assert_eq!(*a.ks.last().unwrap(), 5);
        let b = elbow(&rows, 50, 42).unwrap();
        assert_eq!(a.suggested_k, b.suggested_k);
        assert_eq!(a.inertias, b.inertias);
    }

    #[test]
    fn test_elbow_no_structure() {
        // 全行同一値(定数列のみ)→ 慣性が下がらないので最小の k=2 を返す
        let rows: Vec<Vec<f64>> = (0..20).map(|_| vec![5.0, 5.0]).collect();
        let r = elbow(&rows, 8, 42).unwrap();
        assert_eq!(r.suggested_k, 2);
        assert!(r.inertias[0].abs() < 1e-9);
    }

    #[test]
    fn test_decompose_additive_exact() {
        // 線形トレンド + 平均0の季節パターンは、中心化移動平均で厳密に復元できる
        let pattern = [3.0, -1.0, -2.0, 0.0];
        let y: Vec<f64> = (0..40)
            .map(|i| 10.0 + 0.5 * i as f64 + pattern[i % 4])
            .collect();
        let r = decompose(&y, 4, false).unwrap();
        for (p, expected) in pattern.iter().enumerate() {
            assert!(
                (r.seasonal_pattern[p] - expected).abs() < 1e-9,
                "位相{p}: {} != {expected}",
                r.seasonal_pattern[p]
            );
        }
        // トレンドは 10 + 0.5i に一致(定義域内)、残差はほぼ0
        assert!((r.trend[10] - 15.0).abs() < 1e-9);
        for i in 0..40 {
            if r.residual[i].is_finite() {
                assert!(r.residual[i].abs() < 1e-9);
            }
        }
        assert!(r.trend_strength > 0.99);
        assert!(r.seasonal_strength > 0.99);
        // 両端(half=2)の trend は NaN
        assert!(r.trend[0].is_nan() && r.trend[39].is_nan());
        assert!(r.trend[2].is_finite() && r.trend[37].is_finite());
    }

    #[test]
    fn test_decompose_multiplicative() {
        // 乗法: y = トレンド × 季節係数(平均1)
        let factor = [1.2, 0.9, 0.8, 1.1];
        let y: Vec<f64> = (0..48)
            .map(|i| (100.0 + i as f64) * factor[i % 4])
            .collect();
        let r = decompose(&y, 4, true).unwrap();
        for (p, expected) in factor.iter().enumerate() {
            assert!(
                (r.seasonal_pattern[p] - expected).abs() < 0.02,
                "位相{p}: {}",
                r.seasonal_pattern[p]
            );
        }
        assert!(r.seasonal_strength > 0.95);
        // 乗法の残差は1近傍
        assert!(r
            .residual
            .iter()
            .filter(|v| v.is_finite())
            .all(|v| (v - 1.0).abs() < 0.05));
    }

    #[test]
    fn test_decompose_no_seasonality() {
        // 純粋な線形トレンド → 季節性強度はほぼ0
        let y: Vec<f64> = (0..30).map(|i| 5.0 + 2.0 * i as f64).collect();
        let r = decompose(&y, 7, false).unwrap();
        assert!(r.seasonal_strength < 0.05, "Fs={}", r.seasonal_strength);
        assert!(r.trend_strength > 0.99);
    }

    #[test]
    fn test_spc_stable_no_violation() {
        // 中心10の交互系列: どのルールも発火しない
        let y: Vec<f64> = (0..20)
            .map(|i| if i % 2 == 0 { 9.0 } else { 11.0 })
            .collect();
        let r = spc(&y).unwrap();
        assert!((r.center - 10.0).abs() < 1e-9);
        assert!((r.sigma - 2.0 / 1.128).abs() < 1e-9); // MR̄=2.0
        assert!(r.violations.is_empty());
    }

    #[test]
    fn test_spc_rule1_outlier() {
        let mut y: Vec<f64> = (0..30)
            .map(|i| if i % 2 == 0 { 9.5 } else { 10.5 })
            .collect();
        y[15] = 20.0; // 明確な外れ値
        let r = spc(&y).unwrap();
        assert_eq!(r.violations, vec![SpcViolation { index: 15, rule: 1 }]);
        assert!(r.ucl < 20.0 && r.lcl > 0.0);
    }

    #[test]
    fn test_spc_rule2_run_above_center() {
        // 交互20点(最後は中心より下)のあと、中心よりわずかに上に9点連続。
        // 直前の点が下側であることで、連続がちょうど9点で判定される
        let mut y: Vec<f64> = (0..20)
            .map(|i| if i % 2 == 0 { 11.0 } else { 9.0 })
            .collect();
        y.extend(std::iter::repeat_n(10.8, 9));
        let r = spc(&y).unwrap();
        assert_eq!(r.violations, vec![SpcViolation { index: 28, rule: 2 }]);
    }

    #[test]
    fn test_spc_rule3_trend() {
        // 交互14点のあと、6点連続の緩やかな増加(3σ内・9点未満)
        let mut y: Vec<f64> = (0..14)
            .map(|i| if i % 2 == 0 { 9.0 } else { 11.0 })
            .collect();
        for k in 1..=6 {
            y.push(11.0 + 0.3 * k as f64);
        }
        let r = spc(&y).unwrap();
        assert!(!r.violations.is_empty());
        assert!(r.violations.iter().all(|v| v.rule == 3));
        assert_eq!(r.violations.last().unwrap().index, 19);
    }

    #[test]
    fn test_spc_errors() {
        assert!(spc(&[1.0; 7]).is_err()); // 8点未満
        let mut y = vec![1.0; 10];
        y[3] = f64::NAN;
        assert!(spc(&y).is_err()); // 欠損
    }

    #[test]
    fn test_decompose_errors() {
        let y: Vec<f64> = (0..10).map(|i| i as f64).collect();
        assert!(decompose(&y, 1, false).is_err()); // 周期が小さすぎる
        assert!(decompose(&y, 6, false).is_err()); // データ数不足(6*2 > 10)
        assert!(decompose(&[1.0, f64::NAN, 3.0, 4.0], 2, false).is_err()); // 欠損
        let neg: Vec<f64> = (0..12).map(|i| i as f64 - 5.0).collect();
        assert!(decompose(&neg, 3, true).is_err()); // 乗法に0以下
    }
}
