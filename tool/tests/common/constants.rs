/// Relative error of a value that the tool computes from inputs which f64
/// represents exactly (integer counts and times, exact powers of n), by sums,
/// ratios, logarithms and a least-squares fit over a few points. Each f64
/// operation adds at most half an ulp (1.1e-16), so 1e-12 bounds chains of
/// several hundred operations at a condition of ten.
pub const ROUND_OFF: f64 = 1e-12;

/// Relative error of a fit to synthetic times rounded to whole ns. The
/// rounding moves a time by at most 0.5 ns, which is 7.2e-6 of the smallest
/// such time in the tests (10 n ln n at n = 1000). A fit over ln n propagates
/// this to the exponent and the residual with a factor below 1 and to the
/// constant with a factor of at most ln n = 9.7, which stays below 1e-4.
pub const WHOLE_NS: f64 = 1e-4;

/// Tolerance of exponents and residuals fitted to synthetic times with 2
/// percent normal noise. The standard error of a slope in the tests is at
/// most 0.02 / sqrt(4.8) = 0.009, for five points spaced by a factor of 2,
/// and the rms residual is 0.02. The bound 0.05 is more than five standard
/// errors of the slope and 2.5 times the noise.
pub const NOISY_FIT: f64 = 0.05;
