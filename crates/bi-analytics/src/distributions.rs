//! 統計分布の特殊関数とCDF/生存関数(純Rust・依存なし)。
//! 検定のp値計算基盤。アルゴリズムは Numerical Recipes / Lanczos 近似に基づく。

use std::f64::consts::PI;

/// 対数ガンマ関数(Lanczos近似, g=7)。
pub fn ln_gamma(x: f64) -> f64 {
    // Lanczos係数は公表値をそのまま用いる(桁は意図的)
    #[allow(clippy::excessive_precision)]
    const C: [f64; 9] = [
        0.999_999_999_999_809_93,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_13,
        -176.615_029_162_140_59,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_571_6e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // 反射公式 Γ(x)Γ(1-x) = π/sin(πx)
        (PI).ln() - (PI * x).sin().abs().ln() - ln_gamma(1.0 - x)
    } else {
        let x = x - 1.0;
        let t = x + 7.5;
        let mut a = C[0];
        for (i, &c) in C.iter().enumerate().skip(1) {
            a += c / (x + i as f64);
        }
        0.5 * (2.0 * PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
    }
}

/// 正則化下側不完全ガンマ関数 P(a, x)。
pub fn gamma_p(a: f64, x: f64) -> f64 {
    if x < 0.0 || a <= 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return 0.0;
    }
    if x < a + 1.0 {
        gamma_series(a, x)
    } else {
        1.0 - gamma_cf(a, x)
    }
}

/// 正則化上側不完全ガンマ関数 Q(a, x) = 1 - P(a, x)。
pub fn gamma_q(a: f64, x: f64) -> f64 {
    1.0 - gamma_p(a, x)
}

fn gamma_series(a: f64, x: f64) -> f64 {
    let gln = ln_gamma(a);
    let mut ap = a;
    let mut sum = 1.0 / a;
    let mut del = sum;
    for _ in 0..500 {
        ap += 1.0;
        del *= x / ap;
        sum += del;
        if del.abs() < sum.abs() * 1e-16 {
            break;
        }
    }
    sum * (-x + a * x.ln() - gln).exp()
}

fn gamma_cf(a: f64, x: f64) -> f64 {
    // Lentz法による連分数(Qを返す)
    let gln = ln_gamma(a);
    let tiny = 1e-300;
    let mut b = x + 1.0 - a;
    let mut c = 1.0 / tiny;
    let mut d = 1.0 / b;
    let mut h = d;
    for i in 1..500 {
        let an = -(i as f64) * (i as f64 - a);
        b += 2.0;
        d = an * d + b;
        if d.abs() < tiny {
            d = tiny;
        }
        c = b + an / c;
        if c.abs() < tiny {
            c = tiny;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < 1e-16 {
            break;
        }
    }
    (-x + a * x.ln() - gln).exp() * h
}

/// 正則化不完全ベータ関数 I_x(a, b)。
pub fn beta_i(a: f64, b: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let bt = (ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln()).exp();
    if x < (a + 1.0) / (a + b + 2.0) {
        bt * beta_cf(a, b, x) / a
    } else {
        1.0 - bt * beta_cf(b, a, 1.0 - x) / b
    }
}

fn beta_cf(a: f64, b: f64, x: f64) -> f64 {
    let tiny = 1e-300;
    let qab = a + b;
    let qap = a + 1.0;
    let qam = a - 1.0;
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < tiny {
        d = tiny;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..500 {
        let m = m as f64;
        let m2 = 2.0 * m;
        let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < tiny {
            d = tiny;
        }
        c = 1.0 + aa / c;
        if c.abs() < tiny {
            c = tiny;
        }
        d = 1.0 / d;
        h *= d * c;
        let aa2 = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa2 * d;
        if d.abs() < tiny {
            d = tiny;
        }
        c = 1.0 + aa2 / c;
        if c.abs() < tiny {
            c = tiny;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < 1e-16 {
            break;
        }
    }
    h
}

/// 誤差関数。
pub fn erf(x: f64) -> f64 {
    if x >= 0.0 {
        gamma_p(0.5, x * x)
    } else {
        -gamma_p(0.5, x * x)
    }
}

// ---------- 標準正規分布 ----------

/// 標準正規分布のCDF。
pub fn normal_cdf(z: f64) -> f64 {
    0.5 * (1.0 + erf(z / std::f64::consts::SQRT_2))
}

/// 標準正規分布の両側p値 (|Z| >= |z|)。
pub fn normal_sf_two(z: f64) -> f64 {
    2.0 * (1.0 - normal_cdf(z.abs()))
}

/// 標準正規分布の分位点(逆CDF)。Acklamの有理近似 + Halley1段補正。
pub fn normal_ppf(p: f64) -> f64 {
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    // Acklam の逆正規CDF近似係数(公表値、桁は意図的)
    #[allow(clippy::excessive_precision)]
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_690e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    let plow = 0.024_25;
    let phigh = 1.0 - plow;
    let mut x = if p < plow {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= phigh {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    // Halley法1段で精度を上げる
    let e = normal_cdf(x) - p;
    let u = e * (2.0 * PI).sqrt() * (x * x / 2.0).exp();
    x -= u / (1.0 + x * u / 2.0);
    x
}

// ---------- Student t 分布 ----------

/// t分布のCDF (自由度 df)。
pub fn t_cdf(t: f64, df: f64) -> f64 {
    let x = df / (df + t * t);
    let ib = 0.5 * beta_i(df / 2.0, 0.5, x);
    if t >= 0.0 {
        1.0 - ib
    } else {
        ib
    }
}

/// t分布の両側p値 (|T| >= |t|)。
pub fn t_sf_two(t: f64, df: f64) -> f64 {
    let x = df / (df + t * t);
    beta_i(df / 2.0, 0.5, x)
}

/// t分布の分位点(逆CDF)。二分法。
pub fn t_ppf(p: f64, df: f64) -> f64 {
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    let mut lo = -1000.0;
    let mut hi = 1000.0;
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if t_cdf(mid, df) < p {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

// ---------- カイ二乗分布 ----------

/// カイ二乗分布の上側確率 (X >= x), 自由度 k。
pub fn chi2_sf(x: f64, k: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    gamma_q(k / 2.0, x / 2.0)
}

// ---------- F 分布 ----------

/// F分布の上側確率 (F >= f), 自由度 (d1, d2)。
pub fn f_sf(f: f64, d1: f64, d2: f64) -> f64 {
    if f <= 0.0 {
        return 1.0;
    }
    beta_i(d2 / 2.0, d1 / 2.0, d2 / (d2 + d1 * f))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn test_ln_gamma() {
        assert!(close(ln_gamma(5.0), 24.0_f64.ln(), 1e-9)); // Γ(5)=24
        assert!(close(ln_gamma(0.5), PI.sqrt().ln(), 1e-9)); // Γ(0.5)=√π
        assert!(close(ln_gamma(1.0), 0.0, 1e-9));
    }

    #[test]
    fn test_gamma_p() {
        // P(1, x) = 1 - e^{-x}
        assert!(close(gamma_p(1.0, 1.0), 1.0 - (-1.0_f64).exp(), 1e-12));
        assert!(close(gamma_p(1.0, 2.0), 1.0 - (-2.0_f64).exp(), 1e-12));
    }

    #[test]
    fn test_erf_normal() {
        assert!(close(erf(1.0), 0.842_700_792_949_715, 1e-10));
        assert!(close(normal_cdf(0.0), 0.5, 1e-12));
        assert!(close(normal_cdf(1.0), 0.841_344_746_068_5, 1e-9));
        assert!(close(normal_cdf(1.959_963_985), 0.975, 1e-8));
    }

    #[test]
    fn test_normal_ppf() {
        assert!(close(normal_ppf(0.975), 1.959_963_985, 1e-6));
        assert!(close(normal_ppf(0.5), 0.0, 1e-9));
        assert!(close(normal_ppf(0.025), -1.959_963_985, 1e-6));
    }

    #[test]
    fn test_beta_i() {
        // I_0.5(2,3) = 0.6875 (Beta(2,3)のCDF at 0.5)
        assert!(close(beta_i(2.0, 3.0, 0.5), 0.6875, 1e-9));
    }

    #[test]
    fn test_t_dist() {
        // df=10, t=2.228139 は両側p=0.05
        assert!(close(t_sf_two(2.228_139, 10.0), 0.05, 1e-5));
        assert!(close(t_cdf(2.228_139, 10.0), 0.975, 1e-5));
        assert!(close(t_cdf(0.0, 10.0), 0.5, 1e-9));
        // 逆
        assert!(close(t_ppf(0.975, 10.0), 2.228_139, 1e-4));
    }

    #[test]
    fn test_chi2() {
        // χ²_0.05(1) = 3.841459
        assert!(close(chi2_sf(3.841_459, 1.0), 0.05, 1e-5));
        // χ²_0.05(5) = 11.0705
        assert!(close(chi2_sf(11.070_498, 5.0), 0.05, 1e-5));
    }

    #[test]
    fn test_f_dist() {
        // F_0.05(3, 20) = 3.098391
        assert!(close(f_sf(3.098_391, 3.0, 20.0), 0.05, 1e-5));
        // F_0.05(1, 10) = 4.9646 → sf ≈ 0.05
        assert!(close(f_sf(4.964_603, 1.0, 10.0), 0.05, 1e-5));
    }
}
