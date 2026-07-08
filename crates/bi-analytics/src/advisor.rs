//! Kohaku Test Advisor: 列の型・群数・分布・等分散を診断し、
//! 適切な統計検定を「提案」する(勝手に断定しない半自動化)。

use crate::htest::{self, AssumptionCheck, GroupSummary};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TestOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Recommendation {
    /// 分析目的(推定)
    pub intent: String,
    /// 第一候補の検定id
    pub primary: String,
    pub primary_label: String,
    /// 代替候補
    pub alternatives: Vec<TestOption>,
    /// 提案理由
    pub reasons: Vec<String>,
    /// 注意・警告
    pub warnings: Vec<String>,
    /// 前提条件チェック
    pub assumptions: Vec<AssumptionCheck>,
    /// 群別要約
    pub group_summaries: Vec<GroupSummary>,
    /// 選択可能な全検定(UIのプルダウン用)
    pub available: Vec<TestOption>,
}

fn opt(id: &str, label: &str) -> TestOption {
    TestOption {
        id: id.to_string(),
        label: label.to_string(),
    }
}

fn group_summary(label: &str, x: &[f64]) -> GroupSummary {
    GroupSummary {
        label: label.to_string(),
        n: x.len(),
        mean: htest::mean(x),
        sd: htest::sd(x),
    }
}

/// 群内偏差(値−群平均)をプールして正規性を評価する。
/// ANOVA/t検定が仮定するのは残差の正規性なので、この方が理にかなう。
fn pooled_normality(groups: &[(String, Vec<f64>)]) -> Option<AssumptionCheck> {
    let mut resid = Vec::new();
    for (_, g) in groups {
        if g.len() < 2 {
            continue;
        }
        let m = htest::mean(g);
        resid.extend(g.iter().map(|v| v - m));
    }
    htest::jarque_bera(&resid)
}

/// 数値目的変数 × カテゴリ群 の検定提案。
pub fn advise_numeric_groups(
    groups: &[(String, Vec<f64>)],
    paired: bool,
) -> Result<Recommendation, String> {
    let k = groups.len();
    if k < 2 {
        return Err("2群以上を選択してください".to_string());
    }
    let summaries: Vec<GroupSummary> = groups
        .iter()
        .map(|(name, g)| group_summary(name, g))
        .collect();
    let min_n = groups.iter().map(|(_, g)| g.len()).min().unwrap_or(0);
    let mut reasons = vec![];
    let mut warnings = vec![];
    let mut assumptions = vec![];

    // 前提: 正規性(プール残差)
    let normality = pooled_normality(groups);
    let normal_ok = normality.as_ref().map(|a| a.passed).unwrap_or(true);
    if let Some(a) = normality {
        assumptions.push(a);
    }

    if min_n < 5 {
        warnings.push(format!(
            "最小の群サイズが{min_n}件と小さいため、結果は慎重に解釈してください。"
        ));
    }

    if paired && k == 2 {
        // 対応あり2群
        let (primary, primary_label, alt) = if normal_ok {
            (
                "paired_t",
                "対応のあるt検定",
                vec![opt("wilcoxon", "Wilcoxon符号付順位検定")],
            )
        } else {
            reasons.push(
                "残差の正規性に疑いがあるため、ノンパラメトリックを第一候補にしました。"
                    .to_string(),
            );
            (
                "wilcoxon",
                "Wilcoxon符号付順位検定",
                vec![opt("paired_t", "対応のあるt検定")],
            )
        };
        reasons.push("同一対象の2測定(before/after)として対応ありで比較します。".to_string());
        return Ok(Recommendation {
            intent: "対応する2測定の差の検定".to_string(),
            primary: primary.to_string(),
            primary_label: primary_label.to_string(),
            alternatives: alt,
            reasons,
            warnings,
            assumptions,
            group_summaries: summaries,
            available: vec![
                opt("paired_t", "対応のあるt検定"),
                opt("wilcoxon", "Wilcoxon符号付順位検定"),
            ],
        });
    }

    if k == 2 {
        // 独立2群
        let levene = htest::levene(&[groups[0].1.clone(), groups[1].1.clone()]);
        let equal_var = levene.as_ref().map(|a| a.passed).unwrap_or(true);
        if let Some(a) = levene {
            assumptions.push(a);
        }
        let (primary, primary_label, alt): (&str, &str, Vec<TestOption>) = if !normal_ok {
            reasons.push(
                "残差の正規性に疑いがあるため、Mann-Whitney U検定を第一候補にしました。"
                    .to_string(),
            );
            (
                "mann_whitney",
                "Mann-Whitney U検定",
                vec![opt("welch_t", "Welchのt検定")],
            )
        } else {
            reasons.push(
                "独立2群の平均差の検定です。実務データに頑健なWelchのt検定を既定にしています。"
                    .to_string(),
            );
            let mut alt = vec![opt("mann_whitney", "Mann-Whitney U検定")];
            if equal_var {
                alt.push(opt("student_t", "Studentのt検定(等分散)"));
            }
            ("welch_t", "Welchのt検定", alt)
        };
        if !equal_var {
            reasons
                .push("等分散の仮定が疑わしいため、等分散を仮定しない手法が安全です。".to_string());
        }
        return Ok(Recommendation {
            intent: "独立2群の差の検定".to_string(),
            primary: primary.to_string(),
            primary_label: primary_label.to_string(),
            alternatives: alt,
            reasons,
            warnings,
            assumptions,
            group_summaries: summaries,
            available: vec![
                opt("welch_t", "Welchのt検定"),
                opt("student_t", "Studentのt検定(等分散)"),
                opt("mann_whitney", "Mann-Whitney U検定"),
                opt("levene", "(分散の比較) Levene検定"),
                opt("f_var", "(分散の比較) F検定"),
            ],
        });
    }

    // 3群以上
    let gv: Vec<Vec<f64>> = groups.iter().map(|(_, g)| g.clone()).collect();
    let levene = htest::levene(&gv);
    let equal_var = levene.as_ref().map(|a| a.passed).unwrap_or(true);
    if let Some(a) = levene {
        assumptions.push(a);
    }
    let (primary, primary_label, alt): (&str, &str, Vec<TestOption>) = if !normal_ok {
        reasons.push(
            "残差の正規性に疑いがあるため、Kruskal-Wallis検定を第一候補にしました。".to_string(),
        );
        (
            "kruskal",
            "Kruskal-Wallis検定",
            vec![opt("welch_anova", "Welchの分散分析")],
        )
    } else if equal_var {
        reasons.push(
            "等分散が棄却されなかったため、一元配置分散分析を第一候補にしました。".to_string(),
        );
        (
            "anova",
            "一元配置分散分析 (ANOVA)",
            vec![
                opt("welch_anova", "Welchの分散分析"),
                opt("kruskal", "Kruskal-Wallis検定"),
            ],
        )
    } else {
        reasons.push("等分散が疑わしいため、Welchの分散分析を第一候補にしました。".to_string());
        (
            "welch_anova",
            "Welchの分散分析",
            vec![opt("kruskal", "Kruskal-Wallis検定")],
        )
    };
    reasons.push(format!(
        "{k}群の母平均(分布位置)が全て等しいかを検定します。"
    ));
    Ok(Recommendation {
        intent: "3群以上の差の検定".to_string(),
        primary: primary.to_string(),
        primary_label: primary_label.to_string(),
        alternatives: alt,
        reasons,
        warnings,
        assumptions,
        group_summaries: summaries,
        available: vec![
            opt("anova", "一元配置分散分析 (ANOVA)"),
            opt("welch_anova", "Welchの分散分析"),
            opt("kruskal", "Kruskal-Wallis検定"),
            opt("levene", "(分散の比較) Levene検定"),
        ],
    })
}

/// 1標本(数値列と基準値の比較)の検定提案。
pub fn advise_one_sample(x: &[f64], mu0: f64) -> Result<Recommendation, String> {
    if x.len() < 3 {
        return Err("3件以上の数値データが必要です".to_string());
    }
    let mut assumptions = vec![];
    let jb = htest::jarque_bera(x);
    let normal_ok = jb.as_ref().map(|a| a.passed).unwrap_or(true);
    if let Some(a) = jb {
        assumptions.push(a);
    }
    let mut reasons = vec![format!(
        "数値列の中心(平均・中央値)が基準値 {mu0} と異なるかを検定します。"
    )];
    let mut warnings = vec![];
    if x.len() < 15 {
        warnings.push("標本サイズが小さいため、結果は慎重に解釈してください。".to_string());
    }
    let (primary, primary_label, alt) = if normal_ok {
        (
            "one_sample_t",
            "1標本t検定",
            vec![opt("wilcoxon_1s", "Wilcoxon符号付順位検定(1標本)")],
        )
    } else {
        reasons
            .push("正規性に疑いがあるため、ノンパラメトリックを第一候補にしました。".to_string());
        (
            "wilcoxon_1s",
            "Wilcoxon符号付順位検定(1標本)",
            vec![opt("one_sample_t", "1標本t検定")],
        )
    };
    Ok(Recommendation {
        intent: "1標本の基準値比較".to_string(),
        primary: primary.to_string(),
        primary_label: primary_label.to_string(),
        alternatives: alt,
        reasons,
        warnings,
        assumptions,
        group_summaries: vec![group_summary("標本", x)],
        available: vec![
            opt("one_sample_t", "1標本t検定"),
            opt("wilcoxon_1s", "Wilcoxon符号付順位検定(1標本)"),
        ],
    })
}

/// 比率(二項検定)の提案。counts はカテゴリ別の件数(出現順)。
/// group_summaries に カテゴリ/件数/比率 を入れて返す(UIの成功カテゴリ選択用)。
pub fn advise_proportion(counts: &[(String, usize)]) -> Result<Recommendation, String> {
    if counts.is_empty() {
        return Err("対象列に値がありません".to_string());
    }
    let total: usize = counts.iter().map(|(_, c)| c).sum();
    // 件数の多い順に並べる(UIの既定選択が自然になる)
    let mut sorted: Vec<&(String, usize)> = counts.iter().collect();
    sorted.sort_by_key(|x| std::cmp::Reverse(x.1));
    let summaries: Vec<GroupSummary> = sorted
        .iter()
        .map(|(l, c)| GroupSummary {
            label: l.clone(),
            n: *c,
            mean: *c as f64 / total as f64, // 比率
            sd: f64::NAN,
        })
        .collect();
    let mut warnings = vec![];
    if counts.len() > 2 {
        warnings.push(
            "カテゴリが3種類以上あります。「成功」とみなすカテゴリを1つ選ぶと、それ以外をまとめて二値化して検定します。"
                .to_string(),
        );
    }
    if total < 20 {
        warnings.push("標本サイズが小さいため、結果は慎重に解釈してください。".to_string());
    }
    Ok(Recommendation {
        intent: "比率と基準値の比較".to_string(),
        primary: "binomial".to_string(),
        primary_label: "二項検定(正確法)".to_string(),
        alternatives: vec![],
        reasons: vec![
            "選んだカテゴリの出現比率が基準比率と異なるかを、正確な二項分布で検定します。"
                .to_string(),
        ],
        warnings,
        assumptions: vec![],
        group_summaries: summaries,
        available: vec![opt("binomial", "二項検定(正確法)")],
    })
}

/// 数値 × 数値 の相関検定提案。
pub fn advise_two_numeric(x: &[f64], y: &[f64]) -> Result<Recommendation, String> {
    if x.len() != y.len() || x.len() < 3 {
        return Err("同数で3件以上のデータが必要です".to_string());
    }
    let mut assumptions = vec![];
    let nx = htest::jarque_bera(x);
    let ny = htest::jarque_bera(y);
    let both_normal = nx.as_ref().map(|a| a.passed).unwrap_or(true)
        && ny.as_ref().map(|a| a.passed).unwrap_or(true);
    if let Some(a) = nx {
        assumptions.push(AssumptionCheck {
            name: format!("{} [X]", a.name),
            ..a
        });
    }
    if let Some(a) = ny {
        assumptions.push(AssumptionCheck {
            name: format!("{} [Y]", a.name),
            ..a
        });
    }
    let (primary, primary_label, mut alt) = if both_normal {
        (
            "pearson",
            "Pearson相関の検定",
            vec![opt("spearman", "Spearman順位相関の検定")],
        )
    } else {
        (
            "spearman",
            "Spearman順位相関の検定",
            vec![opt("pearson", "Pearson相関の検定")],
        )
    };
    // Kendallは小標本・同順位が多い場合の代替(計算量の都合で1万件まで)
    if x.len() <= 10_000 {
        alt.push(opt("kendall", "Kendall順位相関の検定 (τ-b)"));
    }
    let mut reasons = vec!["2つの数値変数の関連(相関)を検定します。".to_string()];
    if !both_normal {
        reasons.push(
            "正規性に疑いがあるため、外れ値に頑健なSpearmanを第一候補にしました。".to_string(),
        );
    }
    let mut available = vec![
        opt("pearson", "Pearson相関の検定"),
        opt("spearman", "Spearman順位相関の検定"),
    ];
    if x.len() <= 10_000 {
        available.push(opt("kendall", "Kendall順位相関の検定 (τ-b)"));
    }
    Ok(Recommendation {
        intent: "2変数の相関の検定".to_string(),
        primary: primary.to_string(),
        primary_label: primary_label.to_string(),
        alternatives: alt,
        reasons,
        warnings: vec![],
        assumptions,
        group_summaries: vec![group_summary("X", x), group_summary("Y", y)],
        available,
    })
}

/// カテゴリ × カテゴリ のクロス集計検定提案。
pub fn advise_categorical(table: &[Vec<f64>]) -> Result<Recommendation, String> {
    let rows = table.len();
    if rows < 2 {
        return Err("2行以上必要です".to_string());
    }
    let cols = table[0].len();
    let row_sums: Vec<f64> = table.iter().map(|r| r.iter().sum()).collect();
    let col_sums: Vec<f64> = (0..cols)
        .map(|c| table.iter().map(|r| r[c]).sum())
        .collect();
    let total: f64 = row_sums.iter().sum();
    let mut small = 0;
    for &rs in &row_sums {
        for &cs in &col_sums {
            if rs * cs / total < 5.0 {
                small += 1;
            }
        }
    }
    let is_2x2 = rows == 2 && cols == 2;
    let mut warnings = vec![];
    let mut reasons = vec!["2つのカテゴリ変数の関連を検定します。".to_string()];
    let (primary, primary_label, alt) = if small > 0 && is_2x2 {
        warnings
            .push("期待度数が小さいセルがあるため、Fisherの正確確率検定が適切です。".to_string());
        (
            "fisher",
            "Fisherの正確確率検定 (2×2)",
            vec![opt("chi_square", "カイ二乗独立性検定")],
        )
    } else {
        if small > 0 {
            warnings.push(format!(
                "期待度数5未満のセルが{small}個あります。結果は慎重に解釈してください。"
            ));
        }
        reasons.push("期待度数が概ね十分なため、カイ二乗検定を第一候補にしました。".to_string());
        let mut alt = vec![];
        if is_2x2 {
            alt.push(opt("fisher", "Fisherの正確確率検定 (2×2)"));
        }
        ("chi_square", "カイ二乗独立性検定", alt)
    };
    let mut available = vec![opt("chi_square", "カイ二乗独立性検定")];
    if is_2x2 {
        available.push(opt("fisher", "Fisherの正確確率検定 (2×2)"));
    }
    Ok(Recommendation {
        intent: "カテゴリ間の関連の検定".to_string(),
        primary: primary.to_string(),
        primary_label: primary_label.to_string(),
        alternatives: alt,
        reasons,
        warnings,
        assumptions: vec![],
        group_summaries: vec![],
        available,
    })
}
