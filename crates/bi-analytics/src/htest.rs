//! 統計検定(パラメトリック/ノンパラメトリック)、効果量、前提条件チェック、
//! 多重比較補正。純Rust・依存なし。p値は distributions モジュールで計算する。

use crate::distributions as dist;
use serde::Serialize;

// ---------- 結果型 ----------

#[derive(Debug, Clone, Serialize)]
pub struct EffectSize {
    pub name: String,
    pub value: f64,
    pub magnitude: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfidenceInterval {
    pub level: f64,
    pub low: f64,
    pub high: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupSummary {
    pub label: String,
    pub n: usize,
    pub mean: f64,
    pub sd: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestResult {
    pub test: String,
    pub null_hypothesis: String,
    pub statistic_name: String,
    pub statistic: f64,
    pub df: Option<f64>,
    pub df2: Option<f64>,
    pub p_value: f64,
    pub estimate: Option<f64>,
    pub estimate_label: Option<String>,
    pub ci: Option<ConfidenceInterval>,
    pub effect: Option<EffectSize>,
    pub n: usize,
    pub groups: Vec<GroupSummary>,
    pub warnings: Vec<String>,
    pub interpretation: String,
}

// ---------- 基本統計ヘルパ ----------

pub fn mean(x: &[f64]) -> f64 {
    x.iter().sum::<f64>() / x.len() as f64
}

/// 標本分散(不偏, n-1)
pub fn var(x: &[f64]) -> f64 {
    let n = x.len();
    if n < 2 {
        return f64::NAN;
    }
    let m = mean(x);
    x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (n - 1) as f64
}

pub fn sd(x: &[f64]) -> f64 {
    var(x).sqrt()
}

fn median(x: &[f64]) -> f64 {
    let mut v = x.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn skewness(x: &[f64]) -> f64 {
    let n = x.len() as f64;
    let m = mean(x);
    let s = (x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / n).sqrt();
    if s == 0.0 {
        return 0.0;
    }
    x.iter().map(|v| ((v - m) / s).powi(3)).sum::<f64>() / n
}

fn kurtosis(x: &[f64]) -> f64 {
    // 通常の尖度(正規分布=3)
    let n = x.len() as f64;
    let m = mean(x);
    let s2 = x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / n;
    if s2 == 0.0 {
        return 3.0;
    }
    x.iter().map(|v| (v - m).powi(4)).sum::<f64>() / n / (s2 * s2)
}

/// 平均順位(同順位は平均で割り当て)。同順位補正項 Σ(t³-t) も返す。
fn ranks_with_ties(v: &[f64]) -> (Vec<f64>, f64) {
    let n = v.len();
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&i, &j| v[i].partial_cmp(&v[j]).unwrap());
    let mut r = vec![0.0; n];
    let mut tie_sum = 0.0;
    let mut i = 0;
    while i < n {
        let mut j = i;
        while j + 1 < n && v[idx[j + 1]] == v[idx[i]] {
            j += 1;
        }
        let count = (j - i + 1) as f64;
        let avg = (i + j) as f64 / 2.0 + 1.0;
        for k in i..=j {
            r[idx[k]] = avg;
        }
        tie_sum += count.powi(3) - count;
        i = j + 1;
    }
    (r, tie_sum)
}

fn summary(label: &str, x: &[f64]) -> GroupSummary {
    GroupSummary {
        label: label.to_string(),
        n: x.len(),
        mean: mean(x),
        sd: sd(x),
    }
}

// ---------- 効果量の大きさ判定 ----------

fn mag_d(d: f64) -> String {
    let a = d.abs();
    if a < 0.2 {
        "ごく小"
    } else if a < 0.5 {
        "小"
    } else if a < 0.8 {
        "中"
    } else {
        "大"
    }
    .to_string()
}

fn mag_r(r: f64) -> String {
    let a = r.abs();
    if a < 0.1 {
        "ごく小"
    } else if a < 0.3 {
        "小"
    } else if a < 0.5 {
        "中"
    } else {
        "大"
    }
    .to_string()
}

fn mag_eta2(e: f64) -> String {
    if e < 0.01 {
        "ごく小"
    } else if e < 0.06 {
        "小"
    } else if e < 0.14 {
        "中"
    } else {
        "大"
    }
    .to_string()
}

/// p値の表示用整形(極小値は「<0.0001」)
fn fmt_p(p: f64) -> String {
    if p > 0.0 && p < 0.0001 {
        "<0.0001".to_string()
    } else {
        format!("{p:.4}")
    }
}

fn sig_comment(p: f64, alpha: f64) -> String {
    if p < alpha {
        format!(
            "有意水準{:.0}%で統計的に有意な差・関連が見られます(p={})。",
            alpha * 100.0,
            fmt_p(p)
        )
    } else {
        format!(
            "有意水準{:.0}%では統計的に有意とは言えません(p={})。",
            alpha * 100.0,
            fmt_p(p)
        )
    }
}

// ---------- t検定 ----------

/// 1標本t検定。基準値 mu0 との比較。
pub fn one_sample_t(x: &[f64], mu0: f64, alpha: f64) -> Result<TestResult, String> {
    let n = x.len();
    if n < 2 {
        return Err("データが2件以上必要です".to_string());
    }
    let m = mean(x);
    let s = sd(x);
    if s == 0.0 {
        return Err("値が一定のため検定できません".to_string());
    }
    let se = s / (n as f64).sqrt();
    let df = (n - 1) as f64;
    let t = (m - mu0) / se;
    let p = dist::t_sf_two(t, df);
    let tcrit = dist::t_ppf(1.0 - alpha / 2.0, df);
    let d = (m - mu0) / s;
    let mut warnings = vec![];
    if n < 15 {
        warnings.push("標本サイズが小さいため正規性の影響を受けやすいです。".to_string());
    }
    Ok(TestResult {
        test: "1標本t検定".to_string(),
        null_hypothesis: format!("母平均は {mu0} に等しい"),
        statistic_name: "t".to_string(),
        statistic: t,
        df: Some(df),
        df2: None,
        p_value: p,
        estimate: Some(m - mu0),
        estimate_label: Some("平均と基準値の差".to_string()),
        ci: Some(ConfidenceInterval {
            level: 1.0 - alpha,
            low: (m - mu0) - tcrit * se,
            high: (m - mu0) + tcrit * se,
        }),
        effect: Some(EffectSize {
            name: "Cohen's d".to_string(),
            value: d,
            magnitude: mag_d(d),
        }),
        n,
        groups: vec![summary("標本", x)],
        warnings,
        interpretation: sig_comment(p, alpha),
    })
}

fn two_group_t(a: &[f64], b: &[f64], alpha: f64, welch: bool) -> Result<TestResult, String> {
    let (na, nb) = (a.len(), b.len());
    if na < 2 || nb < 2 {
        return Err("各群に2件以上のデータが必要です".to_string());
    }
    let (ma, mb) = (mean(a), mean(b));
    let (va, vb) = (var(a), var(b));
    if va == 0.0 && vb == 0.0 {
        return Err("両群とも値が一定のため検定できません".to_string());
    }
    let diff = ma - mb;
    let (se, df) = if welch {
        let se = (va / na as f64 + vb / nb as f64).sqrt();
        let df = (va / na as f64 + vb / nb as f64).powi(2)
            / ((va / na as f64).powi(2) / (na as f64 - 1.0)
                + (vb / nb as f64).powi(2) / (nb as f64 - 1.0));
        (se, df)
    } else {
        let dfp = (na + nb - 2) as f64;
        let sp2 = ((na as f64 - 1.0) * va + (nb as f64 - 1.0) * vb) / dfp;
        let se = (sp2 * (1.0 / na as f64 + 1.0 / nb as f64)).sqrt();
        (se, dfp)
    };
    let t = diff / se;
    let p = dist::t_sf_two(t, df);
    let tcrit = dist::t_ppf(1.0 - alpha / 2.0, df);
    // 効果量: プールSDによる Cohen's d → Hedges' g 補正
    let sp = (((na as f64 - 1.0) * va + (nb as f64 - 1.0) * vb) / (na + nb - 2) as f64).sqrt();
    let d = if sp > 0.0 { diff / sp } else { 0.0 };
    let j = 1.0 - 3.0 / (4.0 * (na + nb) as f64 - 9.0);
    let g = d * j;
    let mut warnings = vec![];
    if !welch && (va.max(vb) / va.min(vb).max(1e-12)) > 3.0 {
        warnings
            .push("群間の分散差が大きいです。Welchのt検定の使用を推奨します。".to_string());
    }
    if na < 15 || nb < 15 {
        warnings.push("標本サイズが小さいため正規性の影響を受けやすいです。".to_string());
    }
    Ok(TestResult {
        test: if welch {
            "Welchのt検定"
        } else {
            "Studentのt検定(等分散)"
        }
        .to_string(),
        null_hypothesis: "2群の母平均は等しい".to_string(),
        statistic_name: "t".to_string(),
        statistic: t,
        df: Some(df),
        df2: None,
        p_value: p,
        estimate: Some(diff),
        estimate_label: Some("平均差 (群1 − 群2)".to_string()),
        ci: Some(ConfidenceInterval {
            level: 1.0 - alpha,
            low: diff - tcrit * se,
            high: diff + tcrit * se,
        }),
        effect: Some(EffectSize {
            name: "Hedges' g".to_string(),
            value: g,
            magnitude: mag_d(g),
        }),
        n: na + nb,
        groups: vec![summary("群1", a), summary("群2", b)],
        warnings,
        interpretation: sig_comment(p, alpha),
    })
}

pub fn welch_t(a: &[f64], b: &[f64], alpha: f64) -> Result<TestResult, String> {
    two_group_t(a, b, alpha, true)
}

pub fn student_t(a: &[f64], b: &[f64], alpha: f64) -> Result<TestResult, String> {
    two_group_t(a, b, alpha, false)
}

/// 対応ありt検定。a[i] と b[i] は同一対象の測定値。
pub fn paired_t(a: &[f64], b: &[f64], alpha: f64) -> Result<TestResult, String> {
    if a.len() != b.len() {
        return Err("対応ありデータは同数である必要があります".to_string());
    }
    let d: Vec<f64> = a.iter().zip(b.iter()).map(|(x, y)| x - y).collect();
    let n = d.len();
    if n < 2 {
        return Err("データが2件以上必要です".to_string());
    }
    let md = mean(&d);
    let sdd = sd(&d);
    if sdd == 0.0 {
        return Err("差が一定のため検定できません".to_string());
    }
    let se = sdd / (n as f64).sqrt();
    let df = (n - 1) as f64;
    let t = md / se;
    let p = dist::t_sf_two(t, df);
    let tcrit = dist::t_ppf(1.0 - alpha / 2.0, df);
    let dz = md / sdd;
    Ok(TestResult {
        test: "対応のあるt検定".to_string(),
        null_hypothesis: "対応する2測定の母平均差は0".to_string(),
        statistic_name: "t".to_string(),
        statistic: t,
        df: Some(df),
        df2: None,
        p_value: p,
        estimate: Some(md),
        estimate_label: Some("平均差 (前 − 後)".to_string()),
        ci: Some(ConfidenceInterval {
            level: 1.0 - alpha,
            low: md - tcrit * se,
            high: md + tcrit * se,
        }),
        effect: Some(EffectSize {
            name: "Cohen's dz".to_string(),
            value: dz,
            magnitude: mag_d(dz),
        }),
        n,
        groups: vec![summary("測定1", a), summary("測定2", b)],
        warnings: vec![],
        interpretation: sig_comment(p, alpha),
    })
}

// ---------- ノンパラメトリック ----------

/// Mann-Whitney U検定(正規近似, 同順位補正あり)。
pub fn mann_whitney(a: &[f64], b: &[f64], alpha: f64) -> Result<TestResult, String> {
    let (na, nb) = (a.len(), b.len());
    if na < 1 || nb < 1 {
        return Err("各群に1件以上のデータが必要です".to_string());
    }
    let mut all = a.to_vec();
    all.extend_from_slice(b);
    let (r, tie_sum) = ranks_with_ties(&all);
    let r1: f64 = r[..na].iter().sum();
    let n = (na + nb) as f64;
    let u1 = r1 - na as f64 * (na as f64 + 1.0) / 2.0;
    let mu = na as f64 * nb as f64 / 2.0;
    let sigma2 =
        na as f64 * nb as f64 / 12.0 * ((n + 1.0) - tie_sum / (n * (n - 1.0)));
    let sigma = sigma2.sqrt();
    let z = if sigma > 0.0 {
        (u1 - mu) / sigma
    } else {
        0.0
    };
    let p = dist::normal_sf_two(z);
    let rb = 1.0 - 2.0 * u1 / (na as f64 * nb as f64); // rank-biserial (符号は群1基準)
    let mut warnings = vec![];
    if na < 8 || nb < 8 {
        warnings.push("標本が小さく正規近似の精度が落ちます。".to_string());
    }
    Ok(TestResult {
        test: "Mann-Whitney U検定".to_string(),
        null_hypothesis: "2群の分布位置は等しい".to_string(),
        statistic_name: "U".to_string(),
        statistic: u1,
        df: None,
        df2: None,
        p_value: p,
        estimate: Some(-rb),
        estimate_label: Some("順位二相関 (群1が大きいほど正)".to_string()),
        ci: None,
        effect: Some(EffectSize {
            name: "順位二相関 r".to_string(),
            value: -rb,
            magnitude: mag_r(rb),
        }),
        n: na + nb,
        groups: vec![summary("群1", a), summary("群2", b)],
        warnings,
        interpretation: sig_comment(p, alpha),
    })
}

/// Wilcoxon符号付順位検定(正規近似)。
pub fn wilcoxon_signed_rank(a: &[f64], b: &[f64], alpha: f64) -> Result<TestResult, String> {
    if a.len() != b.len() {
        return Err("対応ありデータは同数である必要があります".to_string());
    }
    let diffs: Vec<f64> = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| x - y)
        .filter(|d| *d != 0.0)
        .collect();
    let n = diffs.len();
    if n < 1 {
        return Err("差が全て0のため検定できません".to_string());
    }
    let absd: Vec<f64> = diffs.iter().map(|d| d.abs()).collect();
    let (r, tie_sum) = ranks_with_ties(&absd);
    let w_plus: f64 = r
        .iter()
        .zip(diffs.iter())
        .filter(|(_, d)| **d > 0.0)
        .map(|(rk, _)| *rk)
        .sum();
    let nn = n as f64;
    let mu = nn * (nn + 1.0) / 4.0;
    let sigma2 = nn * (nn + 1.0) * (2.0 * nn + 1.0) / 24.0 - tie_sum / 48.0;
    let sigma = sigma2.sqrt();
    let z = if sigma > 0.0 { (w_plus - mu) / sigma } else { 0.0 };
    let p = dist::normal_sf_two(z);
    let rb = z / nn.sqrt(); // 近似効果量 r = z/√N
    let mut warnings = vec![];
    if n < 10 {
        warnings.push("標本が小さく正規近似の精度が落ちます。".to_string());
    }
    Ok(TestResult {
        test: "Wilcoxon符号付順位検定".to_string(),
        null_hypothesis: "対応差の分布は0を中心に対称".to_string(),
        statistic_name: "W+".to_string(),
        statistic: w_plus,
        df: None,
        df2: None,
        p_value: p,
        estimate: None,
        estimate_label: None,
        ci: None,
        effect: Some(EffectSize {
            name: "効果量 r".to_string(),
            value: rb,
            magnitude: mag_r(rb),
        }),
        n,
        groups: vec![summary("測定1", a), summary("測定2", b)],
        warnings,
        interpretation: sig_comment(p, alpha),
    })
}

// ---------- 分散分析 ----------

/// 一元配置分散分析(等分散を仮定)。
pub fn one_way_anova(groups: &[Vec<f64>], alpha: f64) -> Result<TestResult, String> {
    let k = groups.len();
    if k < 2 {
        return Err("2群以上が必要です".to_string());
    }
    if groups.iter().any(|g| g.len() < 2) {
        return Err("各群に2件以上のデータが必要です".to_string());
    }
    let total_n: usize = groups.iter().map(|g| g.len()).sum();
    let all: Vec<f64> = groups.iter().flatten().copied().collect();
    let grand = mean(&all);
    let ss_between: f64 = groups
        .iter()
        .map(|g| g.len() as f64 * (mean(g) - grand).powi(2))
        .sum();
    let ss_within: f64 = groups
        .iter()
        .map(|g| {
            let m = mean(g);
            g.iter().map(|v| (v - m).powi(2)).sum::<f64>()
        })
        .sum();
    let df1 = (k - 1) as f64;
    let df2 = (total_n - k) as f64;
    let ms_between = ss_between / df1;
    let ms_within = ss_within / df2;
    if ms_within == 0.0 {
        return Err("群内変動が0のため検定できません".to_string());
    }
    let f = ms_between / ms_within;
    let p = dist::f_sf(f, df1, df2);
    let eta2 = ss_between / (ss_between + ss_within);
    Ok(TestResult {
        test: "一元配置分散分析 (ANOVA)".to_string(),
        null_hypothesis: "全群の母平均は等しい".to_string(),
        statistic_name: "F".to_string(),
        statistic: f,
        df: Some(df1),
        df2: Some(df2),
        p_value: p,
        estimate: None,
        estimate_label: None,
        ci: None,
        effect: Some(EffectSize {
            name: "η² (イータ二乗)".to_string(),
            value: eta2,
            magnitude: mag_eta2(eta2),
        }),
        n: total_n,
        groups: groups
            .iter()
            .enumerate()
            .map(|(i, g)| summary(&format!("群{}", i + 1), g))
            .collect(),
        warnings: vec![],
        interpretation: sig_comment(p, alpha),
    })
}

/// Welchの分散分析(等分散を仮定しない)。
pub fn welch_anova(groups: &[Vec<f64>], alpha: f64) -> Result<TestResult, String> {
    let k = groups.len();
    if k < 2 {
        return Err("2群以上が必要です".to_string());
    }
    if groups.iter().any(|g| g.len() < 2) {
        return Err("各群に2件以上のデータが必要です".to_string());
    }
    let mut w = vec![0.0; k];
    let mut means = vec![0.0; k];
    let mut ns = vec![0.0; k];
    for (i, g) in groups.iter().enumerate() {
        let v = var(g);
        if v == 0.0 {
            return Err("いずれかの群の分散が0のため検定できません".to_string());
        }
        ns[i] = g.len() as f64;
        means[i] = mean(g);
        w[i] = ns[i] / v;
    }
    let sw: f64 = w.iter().sum();
    let xbar: f64 = w.iter().zip(means.iter()).map(|(wi, mi)| wi * mi).sum::<f64>() / sw;
    let numer: f64 = w
        .iter()
        .zip(means.iter())
        .map(|(wi, mi)| wi * (mi - xbar).powi(2))
        .sum::<f64>()
        / (k as f64 - 1.0);
    let kf = k as f64;
    let denom_sum: f64 = w
        .iter()
        .zip(ns.iter())
        .map(|(wi, ni)| (1.0 - wi / sw).powi(2) / (ni - 1.0))
        .sum();
    let denom = 1.0 + 2.0 * (kf - 2.0) / (kf * kf - 1.0) * denom_sum;
    let f = numer / denom;
    let df1 = kf - 1.0;
    let df2 = (kf * kf - 1.0) / (3.0 * denom_sum);
    let p = dist::f_sf(f, df1, df2);
    // η²は通常のANOVAベースで参考値
    let all: Vec<f64> = groups.iter().flatten().copied().collect();
    let grand = mean(&all);
    let ss_between: f64 = groups
        .iter()
        .map(|g| g.len() as f64 * (mean(g) - grand).powi(2))
        .sum();
    let ss_within: f64 = groups
        .iter()
        .map(|g| {
            let m = mean(g);
            g.iter().map(|v| (v - m).powi(2)).sum::<f64>()
        })
        .sum();
    let eta2 = ss_between / (ss_between + ss_within);
    Ok(TestResult {
        test: "Welchの分散分析".to_string(),
        null_hypothesis: "全群の母平均は等しい".to_string(),
        statistic_name: "F".to_string(),
        statistic: f,
        df: Some(df1),
        df2: Some(df2),
        p_value: p,
        estimate: None,
        estimate_label: None,
        ci: None,
        effect: Some(EffectSize {
            name: "η² (参考)".to_string(),
            value: eta2,
            magnitude: mag_eta2(eta2),
        }),
        n: all.len(),
        groups: groups
            .iter()
            .enumerate()
            .map(|(i, g)| summary(&format!("群{}", i + 1), g))
            .collect(),
        warnings: vec![],
        interpretation: sig_comment(p, alpha),
    })
}

/// Kruskal-Wallis検定。
pub fn kruskal_wallis(groups: &[Vec<f64>], alpha: f64) -> Result<TestResult, String> {
    let k = groups.len();
    if k < 2 {
        return Err("2群以上が必要です".to_string());
    }
    let sizes: Vec<usize> = groups.iter().map(|g| g.len()).collect();
    let total_n: usize = sizes.iter().sum();
    if total_n < 3 {
        return Err("データが不足しています".to_string());
    }
    let all: Vec<f64> = groups.iter().flatten().copied().collect();
    let (r, tie_sum) = ranks_with_ties(&all);
    let n = total_n as f64;
    let mut h = 0.0;
    let mut offset = 0;
    for &sz in &sizes {
        let rsum: f64 = r[offset..offset + sz].iter().sum();
        h += rsum * rsum / sz as f64;
        offset += sz;
    }
    h = 12.0 / (n * (n + 1.0)) * h - 3.0 * (n + 1.0);
    // 同順位補正
    let correction = 1.0 - tie_sum / (n.powi(3) - n);
    if correction > 0.0 {
        h /= correction;
    }
    let df = (k - 1) as f64;
    let p = dist::chi2_sf(h, df);
    let eps2 = (h - k as f64 + 1.0) / (n - k as f64); // ε²効果量
    Ok(TestResult {
        test: "Kruskal-Wallis検定".to_string(),
        null_hypothesis: "全群の分布位置は等しい".to_string(),
        statistic_name: "H".to_string(),
        statistic: h,
        df: Some(df),
        df2: None,
        p_value: p,
        estimate: None,
        estimate_label: None,
        ci: None,
        effect: Some(EffectSize {
            name: "ε² (イプシロン二乗)".to_string(),
            value: eps2,
            magnitude: mag_eta2(eps2),
        }),
        n: total_n,
        groups: groups
            .iter()
            .enumerate()
            .map(|(i, g)| summary(&format!("群{}", i + 1), g))
            .collect(),
        warnings: vec![],
        interpretation: sig_comment(p, alpha),
    })
}

// ---------- カテゴリ検定 ----------

/// カイ二乗独立性検定。table[行][列] = 観測度数。
pub fn chi_square_independence(table: &[Vec<f64>], alpha: f64) -> Result<TestResult, String> {
    let rows = table.len();
    if rows < 2 {
        return Err("2行以上必要です".to_string());
    }
    let cols = table[0].len();
    if cols < 2 || table.iter().any(|r| r.len() != cols) {
        return Err("2列以上・矩形の表が必要です".to_string());
    }
    let row_sums: Vec<f64> = table.iter().map(|r| r.iter().sum()).collect();
    let col_sums: Vec<f64> = (0..cols)
        .map(|c| table.iter().map(|r| r[c]).sum())
        .collect();
    let total: f64 = row_sums.iter().sum();
    if total == 0.0 {
        return Err("度数の合計が0です".to_string());
    }
    let mut chi2 = 0.0;
    let mut small_expected = 0;
    for (i, row) in table.iter().enumerate() {
        for (j, &obs) in row.iter().enumerate() {
            let exp = row_sums[i] * col_sums[j] / total;
            if exp < 5.0 {
                small_expected += 1;
            }
            if exp > 0.0 {
                chi2 += (obs - exp).powi(2) / exp;
            }
        }
    }
    let df = ((rows - 1) * (cols - 1)) as f64;
    let p = dist::chi2_sf(chi2, df);
    // Cramér's V
    let min_dim = (rows.min(cols) - 1) as f64;
    let v = (chi2 / (total * min_dim)).sqrt();
    let mut warnings = vec![];
    if small_expected > 0 {
        warnings.push(format!(
            "期待度数が5未満のセルが{small_expected}個あります。Fisherの正確確率検定を検討してください。"
        ));
    }
    Ok(TestResult {
        test: "カイ二乗独立性検定".to_string(),
        null_hypothesis: "2つのカテゴリ変数は独立(関連なし)".to_string(),
        statistic_name: "χ²".to_string(),
        statistic: chi2,
        df: Some(df),
        df2: None,
        p_value: p,
        estimate: None,
        estimate_label: None,
        ci: None,
        effect: Some(EffectSize {
            name: "Cramér's V".to_string(),
            value: v,
            magnitude: mag_r(v),
        }),
        n: total as usize,
        groups: vec![],
        warnings,
        interpretation: sig_comment(p, alpha),
    })
}

/// Fisherの正確確率検定 (2×2)。両側p値(全表の確率のうち観測以下を合算)。
pub fn fisher_exact_2x2(a: f64, b: f64, c: f64, d: f64, alpha: f64) -> Result<TestResult, String> {
    let (a, b, c, d) = (a.round(), b.round(), c.round(), d.round());
    if a < 0.0 || b < 0.0 || c < 0.0 || d < 0.0 {
        return Err("度数は非負の整数で指定してください".to_string());
    }
    let r1 = a + b;
    let r2 = c + d;
    let c1 = a + c;
    let n = a + b + c + d;
    if n == 0.0 {
        return Err("度数の合計が0です".to_string());
    }
    // 超幾何確率: P(x) = C(r1,x)C(r2,c1-x)/C(n,c1)
    let ln_c = |n: f64, k: f64| -> f64 {
        dist::ln_gamma(n + 1.0) - dist::ln_gamma(k + 1.0) - dist::ln_gamma(n - k + 1.0)
    };
    let ln_p = |x: f64| -> f64 { ln_c(r1, x) + ln_c(r2, c1 - x) - ln_c(n, c1) };
    let x_min = 0.0_f64.max(c1 - r2);
    let x_max = c1.min(r1);
    let p_obs = ln_p(a).exp();
    let mut p_two = 0.0;
    let mut x = x_min;
    while x <= x_max + 0.5 {
        let px = ln_p(x).exp();
        if px <= p_obs * (1.0 + 1e-7) {
            p_two += px;
        }
        x += 1.0;
    }
    let p_two = p_two.min(1.0);
    // オッズ比
    let or = if b * c != 0.0 {
        (a * d) / (b * c)
    } else {
        f64::INFINITY
    };
    Ok(TestResult {
        test: "Fisherの正確確率検定 (2×2)".to_string(),
        null_hypothesis: "2つのカテゴリ変数は独立(関連なし)".to_string(),
        statistic_name: "オッズ比".to_string(),
        statistic: or,
        df: None,
        df2: None,
        p_value: p_two,
        estimate: Some(or),
        estimate_label: Some("オッズ比".to_string()),
        ci: None,
        effect: None,
        n: n as usize,
        groups: vec![],
        warnings: vec![],
        interpretation: sig_comment(p_two, alpha),
    })
}

// ---------- 相関検定 ----------

fn pearson_r(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len() as f64;
    let mx = mean(x);
    let my = mean(y);
    let mut sxy = 0.0;
    let mut sxx = 0.0;
    let mut syy = 0.0;
    for i in 0..x.len() {
        sxy += (x[i] - mx) * (y[i] - my);
        sxx += (x[i] - mx).powi(2);
        syy += (y[i] - my).powi(2);
    }
    let _ = n;
    if sxx <= 0.0 || syy <= 0.0 {
        return f64::NAN;
    }
    sxy / (sxx * syy).sqrt()
}

fn correlation_result(name: &str, r: f64, n: usize, alpha: f64, spearman: bool) -> TestResult {
    let df = (n - 2) as f64;
    let t = r * (df / (1.0 - r * r)).sqrt();
    let p = dist::t_sf_two(t, df);
    // Fisher z による信頼区間(Pearson)
    let ci = if !spearman && n > 3 {
        let z = r.atanh();
        let se = 1.0 / ((n - 3) as f64).sqrt();
        let zc = dist::normal_ppf(1.0 - alpha / 2.0);
        Some(ConfidenceInterval {
            level: 1.0 - alpha,
            low: (z - zc * se).tanh(),
            high: (z + zc * se).tanh(),
        })
    } else {
        None
    };
    TestResult {
        test: name.to_string(),
        null_hypothesis: "母相関係数は0(無相関)".to_string(),
        statistic_name: "t".to_string(),
        statistic: t,
        df: Some(df),
        df2: None,
        p_value: p,
        estimate: Some(r),
        estimate_label: Some("相関係数".to_string()),
        ci,
        effect: Some(EffectSize {
            name: "相関係数 r".to_string(),
            value: r,
            magnitude: mag_r(r),
        }),
        n,
        groups: vec![],
        warnings: vec![],
        interpretation: sig_comment(p, alpha),
    }
}

pub fn pearson_test(x: &[f64], y: &[f64], alpha: f64) -> Result<TestResult, String> {
    if x.len() != y.len() || x.len() < 3 {
        return Err("同数で3件以上のデータが必要です".to_string());
    }
    let r = pearson_r(x, y);
    if r.is_nan() {
        return Err("いずれかの変数が一定のため相関を計算できません".to_string());
    }
    Ok(correlation_result("Pearson相関の検定", r, x.len(), alpha, false))
}

pub fn spearman_test(x: &[f64], y: &[f64], alpha: f64) -> Result<TestResult, String> {
    if x.len() != y.len() || x.len() < 3 {
        return Err("同数で3件以上のデータが必要です".to_string());
    }
    let (rx, _) = ranks_with_ties(x);
    let (ry, _) = ranks_with_ties(y);
    let r = pearson_r(&rx, &ry);
    if r.is_nan() {
        return Err("いずれかの変数が一定のため相関を計算できません".to_string());
    }
    Ok(correlation_result("Spearman順位相関の検定", r, x.len(), alpha, true))
}

// ---------- Kendall順位相関 ----------

/// 同順位グループのサイズ一覧(サイズ2以上のみ)。
fn tie_group_sizes(v: &[f64]) -> Vec<f64> {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut out = vec![];
    let mut run = 1usize;
    for i in 1..=s.len() {
        if i < s.len() && s[i] == s[i - 1] {
            run += 1;
        } else {
            if run > 1 {
                out.push(run as f64);
            }
            run = 1;
        }
    }
    out
}

/// Kendallの順位相関検定 (τ-b, 同順位補正付き正規近似)。
/// O(n²)のため件数上限あり(低スペックPC保護)。
pub fn kendall_test(x: &[f64], y: &[f64], alpha: f64) -> Result<TestResult, String> {
    let n = x.len();
    if n != y.len() || n < 3 {
        return Err("同数で3件以上のデータが必要です".to_string());
    }
    const KENDALL_MAX: usize = 10_000;
    if n > KENDALL_MAX {
        return Err(format!(
            "Kendall検定は計算量の都合で{KENDALL_MAX}件までです。Spearman順位相関を使用してください。"
        ));
    }
    let mut conc = 0i64;
    let mut disc = 0i64;
    for i in 0..n {
        for j in (i + 1)..n {
            let dx = x[i] - x[j];
            let dy = y[i] - y[j];
            if dx != 0.0 && dy != 0.0 {
                if (dx > 0.0) == (dy > 0.0) {
                    conc += 1;
                } else {
                    disc += 1;
                }
            }
        }
    }
    let nf = n as f64;
    let tx = tie_group_sizes(x);
    let ty = tie_group_sizes(y);
    let n0 = nf * (nf - 1.0) / 2.0;
    let n1: f64 = tx.iter().map(|t| t * (t - 1.0) / 2.0).sum();
    let n2: f64 = ty.iter().map(|t| t * (t - 1.0) / 2.0).sum();
    let denom = ((n0 - n1) * (n0 - n2)).sqrt();
    if denom <= 0.0 {
        return Err("いずれかの変数が一定のため相関を計算できません".to_string());
    }
    let s = (conc - disc) as f64;
    let tau = s / denom;
    // 分散(同順位補正付き)
    let vt: f64 = tx.iter().map(|t| t * (t - 1.0) * (2.0 * t + 5.0)).sum();
    let vu: f64 = ty.iter().map(|t| t * (t - 1.0) * (2.0 * t + 5.0)).sum();
    let v1x: f64 = tx.iter().map(|t| t * (t - 1.0)).sum();
    let v1y: f64 = ty.iter().map(|t| t * (t - 1.0)).sum();
    let v2x: f64 = tx.iter().map(|t| t * (t - 1.0) * (t - 2.0)).sum();
    let v2y: f64 = ty.iter().map(|t| t * (t - 1.0) * (t - 2.0)).sum();
    let v0 = nf * (nf - 1.0) * (2.0 * nf + 5.0);
    let mut var_s = (v0 - vt - vu) / 18.0 + v1x * v1y / (2.0 * nf * (nf - 1.0));
    if n > 2 {
        var_s += v2x * v2y / (9.0 * nf * (nf - 1.0) * (nf - 2.0));
    }
    if var_s <= 0.0 {
        return Err("分散を計算できません".to_string());
    }
    let z = s / var_s.sqrt();
    let p = dist::normal_sf_two(z);
    let mut warnings = vec![];
    if n < 10 {
        warnings.push("標本が小さく正規近似の精度が落ちます。".to_string());
    }
    Ok(TestResult {
        test: "Kendall順位相関の検定 (τ-b)".to_string(),
        null_hypothesis: "母順位相関(τ)は0(無相関)".to_string(),
        statistic_name: "z".to_string(),
        statistic: z,
        df: None,
        df2: None,
        p_value: p,
        estimate: Some(tau),
        estimate_label: Some("Kendallのτ-b".to_string()),
        ci: None,
        effect: Some(EffectSize {
            name: "τ-b".to_string(),
            value: tau,
            magnitude: mag_r(tau),
        }),
        n,
        groups: vec![],
        warnings,
        interpretation: sig_comment(p, alpha),
    })
}

// ---------- 1標本Wilcoxon・二項検定・分散の比較 ----------

/// 1標本Wilcoxon符号付順位検定(分布の中心と基準値の比較)。
pub fn wilcoxon_one_sample(x: &[f64], mu0: f64, alpha: f64) -> Result<TestResult, String> {
    let b = vec![mu0; x.len()];
    let mut r = wilcoxon_signed_rank(x, &b, alpha)?;
    r.test = "Wilcoxon符号付順位検定(1標本)".to_string();
    r.null_hypothesis = format!("分布の中心(中央値)は {mu0} に等しい");
    r.groups = vec![summary("標本", x)];
    Ok(r)
}

/// 二項検定(正確法・両側)。観測比率が基準比率 p0 と異なるかを検定する。
/// 信頼区間はWilsonスコア区間。
pub fn binomial_test(k: u64, n: u64, p0: f64, alpha: f64) -> Result<TestResult, String> {
    if n == 0 {
        return Err("データがありません".to_string());
    }
    if k > n {
        return Err("成功数が総数を超えています".to_string());
    }
    if !(p0 > 0.0 && p0 < 1.0) {
        return Err("基準比率は0より大きく1未満で指定してください".to_string());
    }
    let nf = n as f64;
    let ln_pmf = |i: u64| -> f64 {
        dist::ln_gamma(nf + 1.0)
            - dist::ln_gamma(i as f64 + 1.0)
            - dist::ln_gamma((n - i) as f64 + 1.0)
            + i as f64 * p0.ln()
            + (n - i) as f64 * (1.0 - p0).ln()
    };
    // 両側p値: 観測値以下の確率を持つ結果を全て合算(scipyと同じ方式)
    let obs = ln_pmf(k);
    let mut p = 0.0;
    for i in 0..=n {
        let lp = ln_pmf(i);
        if lp <= obs + 1e-7 {
            p += lp.exp();
        }
    }
    let p = p.min(1.0);
    let phat = k as f64 / nf;
    // Wilsonスコア信頼区間
    let zc = dist::normal_ppf(1.0 - alpha / 2.0);
    let dn = 1.0 + zc * zc / nf;
    let center = (phat + zc * zc / (2.0 * nf)) / dn;
    let half = zc * ((phat * (1.0 - phat) + zc * zc / (4.0 * nf)) / nf).sqrt() / dn;
    // 効果量: Cohen's h
    let h = 2.0 * phat.sqrt().asin() - 2.0 * p0.sqrt().asin();
    let mut warnings = vec![];
    if n < 20 {
        warnings.push("標本サイズが小さいため、結果は慎重に解釈してください。".to_string());
    }
    Ok(TestResult {
        test: "二項検定(正確法)".to_string(),
        null_hypothesis: format!("成功比率は {p0} に等しい"),
        statistic_name: "成功数 k".to_string(),
        statistic: k as f64,
        df: None,
        df2: None,
        p_value: p,
        estimate: Some(phat),
        estimate_label: Some("観測比率".to_string()),
        ci: Some(ConfidenceInterval {
            level: 1.0 - alpha,
            low: (center - half).max(0.0),
            high: (center + half).min(1.0),
        }),
        effect: Some(EffectSize {
            name: "Cohen's h".to_string(),
            value: h,
            magnitude: mag_d(h),
        }),
        n: n as usize,
        groups: vec![],
        warnings,
        interpretation: sig_comment(p, alpha),
    })
}

/// Levene検定の中核計算。(W統計量, df1, df2, p値) を返す。
fn levene_core(groups: &[Vec<f64>]) -> Option<(f64, f64, f64, f64)> {
    let k = groups.len();
    if k < 2 || groups.iter().any(|g| g.len() < 2) {
        return None;
    }
    // 各値の「群中央値からの絶対偏差」でANOVA(中央値ベース = Brown-Forsythe)
    let z: Vec<Vec<f64>> = groups
        .iter()
        .map(|g| {
            let med = median(g);
            g.iter().map(|v| (v - med).abs()).collect()
        })
        .collect();
    let total_n: usize = z.iter().map(|g| g.len()).sum();
    let all: Vec<f64> = z.iter().flatten().copied().collect();
    let grand = mean(&all);
    let ss_b: f64 = z
        .iter()
        .map(|g| g.len() as f64 * (mean(g) - grand).powi(2))
        .sum();
    let ss_w: f64 = z
        .iter()
        .map(|g| {
            let m = mean(g);
            g.iter().map(|v| (v - m).powi(2)).sum::<f64>()
        })
        .sum();
    let df1 = (k - 1) as f64;
    let df2 = (total_n - k) as f64;
    if ss_w == 0.0 {
        return None;
    }
    let w = (ss_b / df1) / (ss_w / df2);
    let p = dist::f_sf(w, df1, df2);
    Some((w, df1, df2, p))
}

/// Levene検定(中央値ベース = Brown-Forsythe)を独立した検定として実行する。
pub fn levene_test(groups: &[Vec<f64>], alpha: f64) -> Result<TestResult, String> {
    let (w, df1, df2, p) =
        levene_core(groups).ok_or("各群に2件以上のデータが必要です(偏差が全て0の場合も不可)")?;
    let n: usize = groups.iter().map(|g| g.len()).sum();
    Ok(TestResult {
        test: "Levene検定(中央値ベース / Brown-Forsythe)".to_string(),
        null_hypothesis: "全群の母分散は等しい".to_string(),
        statistic_name: "W".to_string(),
        statistic: w,
        df: Some(df1),
        df2: Some(df2),
        p_value: p,
        estimate: None,
        estimate_label: None,
        ci: None,
        effect: None,
        n,
        groups: groups
            .iter()
            .enumerate()
            .map(|(i, g)| summary(&format!("群{}", i + 1), g))
            .collect(),
        warnings: vec!["中央値ベースのため正規性からの逸脱に頑健です。".to_string()],
        interpretation: sig_comment(p, alpha),
    })
}

/// F検定(2群の分散比)。正規性の逸脱に敏感なため参考用途。
pub fn f_var_test(a: &[f64], b: &[f64], alpha: f64) -> Result<TestResult, String> {
    let (na, nb) = (a.len(), b.len());
    if na < 2 || nb < 2 {
        return Err("各群に2件以上のデータが必要です".to_string());
    }
    let (va, vb) = (var(a), var(b));
    if vb <= 0.0 || va <= 0.0 {
        return Err("いずれかの群の分散が0のため検定できません".to_string());
    }
    let f = va / vb;
    let df1 = (na - 1) as f64;
    let df2 = (nb - 1) as f64;
    let sf = dist::f_sf(f, df1, df2);
    let p = (2.0 * sf.min(1.0 - sf)).min(1.0);
    Ok(TestResult {
        test: "F検定(2群の分散比)".to_string(),
        null_hypothesis: "2群の母分散は等しい".to_string(),
        statistic_name: "F".to_string(),
        statistic: f,
        df: Some(df1),
        df2: Some(df2),
        p_value: p,
        estimate: Some(f),
        estimate_label: Some("分散比 (群1/群2)".to_string()),
        ci: None,
        effect: None,
        n: na + nb,
        groups: vec![summary("群1", a), summary("群2", b)],
        warnings: vec![
            "F検定は正規性の逸脱に敏感です。頑健なLevene検定の併用を推奨します。".to_string(),
        ],
        interpretation: sig_comment(p, alpha),
    })
}

// ---------- 前提条件チェック ----------

#[derive(Debug, Clone, Serialize)]
pub struct AssumptionCheck {
    pub name: String,
    pub statistic: f64,
    pub p_value: f64,
    pub passed: bool,
    pub note: String,
}

/// Jarque-Bera正規性検定。
pub fn jarque_bera(x: &[f64]) -> Option<AssumptionCheck> {
    let n = x.len();
    if n < 8 {
        return None;
    }
    let s = skewness(x);
    let k = kurtosis(x);
    let jb = n as f64 / 6.0 * (s * s + (k - 3.0).powi(2) / 4.0);
    let p = dist::chi2_sf(jb, 2.0);
    Some(AssumptionCheck {
        name: "正規性 (Jarque-Bera)".to_string(),
        statistic: jb,
        p_value: p,
        passed: p >= 0.05,
        note: if p < 0.05 {
            format!("正規分布から外れている可能性(歪度={s:.2}, 尖度={k:.2})")
        } else {
            "正規性の重大な逸脱は検出されず".to_string()
        },
    })
}

/// Levene検定(中央値ベース = Brown-Forsythe, 等分散性)。
pub fn levene(groups: &[Vec<f64>]) -> Option<AssumptionCheck> {
    let (w, _df1, _df2, p) = levene_core(groups)?;
    Some(AssumptionCheck {
        name: "等分散性 (Levene)".to_string(),
        statistic: w,
        p_value: p,
        passed: p >= 0.05,
        note: if p < 0.05 {
            "群間の分散が等しくない可能性。Welch系の手法を推奨。".to_string()
        } else {
            "等分散の仮定は棄却されず".to_string()
        },
    })
}

// ---------- 多重比較補正 ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Correction {
    None,
    Bonferroni,
    Holm,
    BenjaminiHochberg,
    BenjaminiYekutieli,
}

impl Correction {
    pub fn from_str(s: &str) -> Correction {
        match s.to_lowercase().as_str() {
            "bonferroni" => Correction::Bonferroni,
            "holm" => Correction::Holm,
            "bh" | "benjamini-hochberg" | "fdr" => Correction::BenjaminiHochberg,
            "by" | "benjamini-yekutieli" => Correction::BenjaminiYekutieli,
            _ => Correction::None,
        }
    }
}

/// p値ベクトルを補正して調整済みp値を返す。
pub fn adjust_pvalues(p: &[f64], method: Correction) -> Vec<f64> {
    let m = p.len();
    if m == 0 {
        return vec![];
    }
    let mf = m as f64;
    match method {
        Correction::None => p.to_vec(),
        Correction::Bonferroni => p.iter().map(|x| (x * mf).min(1.0)).collect(),
        Correction::Holm => {
            let mut idx: Vec<usize> = (0..m).collect();
            idx.sort_by(|&i, &j| p[i].partial_cmp(&p[j]).unwrap());
            let mut adj = vec![0.0; m];
            let mut running: f64 = 0.0;
            for (rank, &i) in idx.iter().enumerate() {
                let val = ((mf - rank as f64) * p[i]).min(1.0);
                running = running.max(val);
                adj[i] = running;
            }
            adj
        }
        Correction::BenjaminiHochberg => fdr_adjust(p, mf),
        Correction::BenjaminiYekutieli => {
            // BHに調和級数 c(m) = Σ 1/i を掛けた保守版(依存が強い検定群向け)
            let c: f64 = (1..=m).map(|i| 1.0 / i as f64).sum();
            fdr_adjust(p, mf * c)
        }
    }
}

/// FDR系補正の共通処理(factor = m または m·c(m))。
fn fdr_adjust(p: &[f64], factor: f64) -> Vec<f64> {
    let m = p.len();
    let mut idx: Vec<usize> = (0..m).collect();
    idx.sort_by(|&i, &j| p[j].partial_cmp(&p[i]).unwrap()); // 降順
    let mut adj = vec![0.0; m];
    let mut running: f64 = 1.0;
    for (rank_from_top, &i) in idx.iter().enumerate() {
        let rank = m - rank_from_top; // 昇順での順位(大きい方からm,m-1,...)
        let val = (p[i] * factor / rank as f64).min(1.0);
        running = running.min(val);
        adj[i] = running;
    }
    adj
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn test_kendall() {
        // C=8, D=2 → τ=0.6, z=1.4697 → p≈0.1416(正規近似)
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        let y = [1.0, 3.0, 2.0, 5.0, 4.0];
        let r = kendall_test(&x, &y, 0.05).unwrap();
        assert!(close(r.estimate.unwrap(), 0.6, 1e-9));
        assert!(close(r.p_value, 0.1416, 5e-3));
        // 完全一致順位 → τ=1
        let r2 = kendall_test(&x, &x, 0.05).unwrap();
        assert!(close(r2.estimate.unwrap(), 1.0, 1e-9));
    }

    #[test]
    fn test_binomial() {
        // Bin(20, 0.5) で k=15: 両側p = 2*P(X≥15) = 43400/2^20 ≈ 0.041389
        let r = binomial_test(15, 20, 0.5, 0.05).unwrap();
        assert!(close(r.p_value, 0.041389, 1e-4));
        assert!(close(r.estimate.unwrap(), 0.75, 1e-9));
        // 基準通りの比率 → p は 1 に近い
        let r2 = binomial_test(10, 20, 0.5, 0.05).unwrap();
        assert!(r2.p_value > 0.9);
    }

    #[test]
    fn test_wilcoxon_one_sample() {
        // 全て正の値 vs mu0=0 → 有意
        let x = [1.1, 2.3, 0.8, 1.9, 2.5, 1.2, 0.7, 1.8, 2.2, 1.4];
        let r = wilcoxon_one_sample(&x, 0.0, 0.05).unwrap();
        assert!(r.p_value < 0.01);
    }

    #[test]
    fn test_levene_test_and_f_var() {
        // 分散が大きく異なる2群
        let a = [1.0, 5.0, 9.0, 2.0, 8.0, 3.0, 7.0, 1.5, 8.5, 4.0];
        let b = [5.0, 5.2, 4.8, 5.1, 4.9, 5.0, 5.3, 4.7, 5.1, 4.9];
        let lv = levene_test(&[a.to_vec(), b.to_vec()], 0.05).unwrap();
        assert!(lv.p_value < 0.01);
        let fv = f_var_test(&a, &b, 0.05).unwrap();
        assert!(fv.p_value < 0.001);
        assert!(fv.statistic > 1.0); // va > vb
        // 分散が同程度なら有意にならない
        let c = [1.0, 5.0, 9.0, 2.0, 8.0, 3.0, 7.0, 1.5, 8.5, 4.0];
        let lv2 = levene_test(&[a.to_vec(), c.to_vec()], 0.05).unwrap();
        assert!(lv2.p_value > 0.9);
    }

    #[test]
    fn test_by_adjust() {
        // BH: 全て0.04 / BY: ×c(4)=2.0833… → 0.08333
        let p = [0.01, 0.02, 0.03, 0.04];
        let bh = adjust_pvalues(&p, Correction::BenjaminiHochberg);
        for v in &bh {
            assert!(close(*v, 0.04, 1e-9));
        }
        let by = adjust_pvalues(&p, Correction::BenjaminiYekutieli);
        for v in &by {
            assert!(close(*v, 0.04 * (1.0 + 0.5 + 1.0 / 3.0 + 0.25), 1e-9));
        }
    }

    #[test]
    fn test_welch_t() {
        let a = [27.0, 28.0, 29.0, 30.0, 31.0];
        let b = [22.0, 23.0, 24.0, 25.0, 26.0];
        let r = welch_t(&a, &b, 0.05).unwrap();
        assert!(close(r.statistic, 5.0, 1e-9));
        assert!(close(r.df.unwrap(), 8.0, 1e-9));
        assert!(close(r.estimate.unwrap(), 5.0, 1e-9));
        assert!(r.p_value < 0.01);
    }

    #[test]
    fn test_student_t() {
        let a = [27.0, 28.0, 29.0, 30.0, 31.0];
        let b = [22.0, 23.0, 24.0, 25.0, 26.0];
        let r = student_t(&a, &b, 0.05).unwrap();
        assert!(close(r.statistic, 5.0, 1e-9));
        assert!(close(r.df.unwrap(), 8.0, 1e-9));
    }

    #[test]
    fn test_paired_t() {
        let a = [5.0, 7.0, 7.0, 9.0, 10.0];
        let b = [4.0, 5.0, 6.0, 7.0, 8.0]; // 差は [1,2,1,2,2], 平均1.6
        let r = paired_t(&a, &b, 0.05).unwrap();
        assert!(close(r.estimate.unwrap(), 1.6, 1e-9));
        assert!(r.p_value < 0.01); // 一貫して正の差 → 有意
    }

    #[test]
    fn test_one_sample_t() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0]; // mean 3, sd sqrt(2.5)
        let r = one_sample_t(&x, 0.0, 0.05).unwrap();
        // t = 3 / (1.5811/sqrt5) = 3 / 0.7071 = 4.2426
        assert!(close(r.statistic, 4.242_640_687, 1e-6));
        assert!(close(r.df.unwrap(), 4.0, 1e-9));
    }

    #[test]
    fn test_anova() {
        let g = vec![
            vec![1.0, 2.0, 3.0],
            vec![4.0, 5.0, 6.0],
            vec![7.0, 8.0, 9.0],
        ];
        let r = one_way_anova(&g, 0.05).unwrap();
        assert!(close(r.statistic, 27.0, 1e-9));
        assert!(close(r.df.unwrap(), 2.0, 1e-9));
        assert!(close(r.df2.unwrap(), 6.0, 1e-9));
    }

    #[test]
    fn test_chi_square() {
        let table = vec![vec![10.0, 20.0], vec![20.0, 10.0]];
        let r = chi_square_independence(&table, 0.05).unwrap();
        assert!(close(r.statistic, 6.666_667, 1e-4));
        assert!(close(r.df.unwrap(), 1.0, 1e-9));
        assert!(r.p_value < 0.05);
    }

    #[test]
    fn test_fisher() {
        // 紅茶の実験 [[3,1],[1,3]] → 両側p ≈ 0.4857
        let r = fisher_exact_2x2(3.0, 1.0, 1.0, 3.0, 0.05).unwrap();
        assert!(close(r.p_value, 0.485_714, 1e-4));
    }

    #[test]
    fn test_pearson() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        let y = [2.0, 4.0, 6.0, 8.0, 10.0]; // 完全相関
        let r = pearson_test(&x, &y, 0.05).unwrap();
        assert!(close(r.estimate.unwrap(), 1.0, 1e-9));
        assert!(r.p_value < 0.001);
    }

    #[test]
    fn test_spearman_monotonic() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        let y = [1.0, 4.0, 9.0, 16.0, 25.0]; // 単調増加 → Spearman r=1
        let r = spearman_test(&x, &y, 0.05).unwrap();
        assert!(close(r.estimate.unwrap(), 1.0, 1e-9));
    }

    #[test]
    fn test_mann_whitney_separated() {
        let a = [1.0, 2.0, 3.0, 4.0];
        let b = [5.0, 6.0, 7.0, 8.0];
        let r = mann_whitney(&a, &b, 0.05).unwrap();
        assert!(close(r.statistic, 0.0, 1e-9)); // U1 = 0(完全分離)
    }

    #[test]
    fn test_holm_bh() {
        let p = [0.01, 0.02, 0.03, 0.04, 0.05];
        let holm = adjust_pvalues(&p, Correction::Holm);
        // Holm: 単調非減少
        for i in 1..holm.len() {
            assert!(holm[i] >= holm[i - 1] - 1e-12);
        }
        let bonf = adjust_pvalues(&p, Correction::Bonferroni);
        assert!(close(bonf[0], 0.05, 1e-12));
        let bh = adjust_pvalues(&p, Correction::BenjaminiHochberg);
        // BH最大は 0.05*5/5 = 0.05
        assert!(bh.iter().all(|&x| x <= 0.05 + 1e-9));
    }

    #[test]
    fn test_kruskal() {
        let g = vec![
            vec![1.0, 2.0, 3.0],
            vec![4.0, 5.0, 6.0],
            vec![7.0, 8.0, 9.0],
        ];
        let r = kruskal_wallis(&g, 0.05).unwrap();
        // 完全分離 → H = n-1 群平均順位が離れ、有意
        assert!(r.p_value < 0.05);
        assert!(close(r.df.unwrap(), 2.0, 1e-9));
    }
}
