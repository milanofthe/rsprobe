"""Thresholds and sizes of the tool, each with the reason for its value."""

# Bootstrap resamples per interval. Percentile intervals need on the order of
# a thousand (Efron and Tibshirani, An Introduction to the Bootstrap, ch. 13);
# 2000 keeps the Monte Carlo error of a 2.5 percent quantile at about a tenth
# of its spacing and costs milliseconds per node in Python.
BOOTSTRAP = 2000

# Seed of the bootstrap, so that the same records give the same verdict.
SEED = 1

# Two-sided confidence of the intervals.
CONFIDENCE = 0.95

# Smallest relative change reported as slower or faster. The default noise
# threshold of criterion; below it, rebuilding the same code moves timings by
# as much through code layout (Mytkowicz et al., ASPLOS 2009).
MIN_EFFECT = 0.01

# Fewest records per set for a verdict on a noisy measure: a median of two
# is their mean, and the bootstrap of fewer than three values has fewer than
# ten distinct resamples.
MIN_ROUNDS = 3

# Share of the run a node needs before its slowdown fails the gate: a slower
# node below 5 percent moves the run by less than its own timing noise.
GATE_SHARE = 0.05

# A node is marked when the estimated scope cost exceeds this share of its
# recorded time: beyond it the corrected time is set mostly by the
# calibration constants, not by the measurement.
OVERHEAD_MARK = 0.10

# Fewest distinct parameter values for a fit of c n^p log^q(n): two points
# fit any two-parameter model exactly and leave no residual.
MIN_FIT_POINTS = 3

# Powers of log(n) tried in the fit.
LOG_POWERS = (0, 1)

# A node's exponent departs from the run's when the local exponents at the
# largest n differ by more than this. Three points over a factor of 10 at
# 5 percent timing noise give a slope error of 0.03; 0.1 is three of them.
DEPARTURE = 0.1
