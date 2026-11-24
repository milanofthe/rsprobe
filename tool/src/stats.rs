//! Sums, medians, bootstrap intervals, the scaling fit and the serial
//! fraction.

use crate::constants::{BOOTSTRAP, CONFIDENCE, LOG_POWERS, SEED};

/// A sum with Neumaier's compensation (exact for integers below 2^53).
pub fn sum(xs: impl IntoIterator<Item = f64>) -> f64 {
    let (mut s, mut c) = (0.0f64, 0.0f64);
    for x in xs {
        let t = s + x;
        c += if s.abs() >= x.abs() {
            (s - t) + x
        } else {
            (x - t) + s
        };
        s = t;
    }
    if c != 0.0 && c.is_finite() {
        s + c
    } else {
        s
    }
}

/// The median; the mean of the middle two for an even count.
pub fn median(xs: &[f64]) -> f64 {
    let mut s = xs.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let k = s.len() / 2;
    match s.len() % 2 {
        _ if s.is_empty() => f64::NAN,
        1 => s[k],
        _ => (s[k - 1] + s[k]) / 2.0,
    }
}

/// SplitMix64 (Steele, Lea and Flood, OOPSLA 2014): a small seeded
/// generator for the bootstrap and the arm order of `ab`.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// The generator of a seed.
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in 0..n (Lemire's multiply-shift, bias below 2^-32 for the
    /// sizes here).
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next_u64() as u128 * n as u128) >> 64) as usize
    }

    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Fisher-Yates shuffle.
    pub fn shuffle<T>(&mut self, xs: &mut [T]) {
        for i in (1..xs.len()).rev() {
            xs.swap(i, self.below(i + 1));
        }
    }
}

/// median(b) / median(a) and its bootstrap percentile interval at
/// CONFIDENCE: a and b resampled independently BOOTSTRAP times with a fixed
/// seed. Resamples with a median of zero in a are dropped.
pub fn ratio_ci(a: &[f64], b: &[f64]) -> (f64, f64, f64) {
    let mut rng = Rng::new(SEED);
    let mut resample = |xs: &[f64]| {
        median(
            &(0..xs.len())
                .map(|_| xs[rng.below(xs.len())])
                .collect::<Vec<_>>(),
        )
    };
    let mut rs = Vec::with_capacity(BOOTSTRAP);
    for _ in 0..BOOTSTRAP {
        let (ma, mb) = (resample(a), resample(b));
        if ma > 0.0 {
            rs.push(mb / ma);
        }
    }
    rs.sort_by(|x, y| x.total_cmp(y));
    let tail = (1.0 - CONFIDENCE) / 2.0;
    let n = rs.len() as f64;
    let at = |q: f64| {
        rs.get(((q * n) as usize).min(rs.len().max(1) - 1))
            .copied()
            .unwrap_or(f64::NAN)
    };
    (median(b) / median(a), at(tail), at(1.0 - tail))
}

/// Least-squares a + b x through the points; (a, b, rms residual).
pub fn line(x: &[f64], y: &[f64]) -> (f64, f64, f64) {
    let k = x.len() as f64;
    let (mx, my) = (sum(x.iter().copied()) / k, sum(y.iter().copied()) / k);
    let sxx = sum(x.iter().map(|u| (u - mx) * (u - mx)));
    let b = sum(x.iter().zip(y).map(|(u, w)| (u - mx) * (w - my))) / sxx;
    let a = my - b * mx;
    let res = sum(x
        .iter()
        .zip(y)
        .map(|(u, w)| (w - a - b * u) * (w - a - b * u)))
        / k;
    (a, b, res.sqrt())
}

/// The model c n^p log^q(n) fitted to y > 0 by least squares in log space,
/// log y - q log log n = log c + p log n, for each q in LOG_POWERS (q > 0
/// only where every n > 1); the q with the smallest residual. Returns (c, p,
/// q, rms residual of log y).
pub fn power_fit(n: &[f64], y: &[f64]) -> (f64, f64, i32, f64) {
    let mut best: Option<(f64, f64, i32, f64)> = None;
    let small = n.iter().any(|&u| u <= 1.0);
    for q in LOG_POWERS.into_iter().filter(|&q| q == 0 || !small) {
        let ly: Vec<f64> = match q {
            0 => y.iter().map(|w| w.ln()).collect(),
            _ => n
                .iter()
                .zip(y)
                .map(|(u, w)| w.ln() - q as f64 * u.ln().ln())
                .collect(),
        };
        let (a, p, res) = line(&n.iter().map(|u| u.ln()).collect::<Vec<_>>(), &ly);
        if best.is_none_or(|b| res < b.3) {
            best = Some((a.exp(), p, q, res));
        }
    }
    best.unwrap_or((f64::NAN, f64::NAN, 0, f64::NAN))
}

/// d log y / d log n of c n^p log^q(n) at n.
pub fn local_exponent(p: f64, q: i32, n: f64) -> f64 {
    p + q as f64 / n.ln()
}

/// The serial fraction e = (1/S - 1/p) / (1 - 1/p) of a speedup S on p
/// times the base thread count (Karp and Flatt, CACM 1990).
pub fn karp_flatt(speedup: f64, p: f64) -> f64 {
    (1.0 / speedup - 1.0 / p) / (1.0 - 1.0 / p)
}
