"""Medians, bootstrap intervals, the scaling fit and the serial fraction."""

import math
import random

from .constants import BOOTSTRAP, CONFIDENCE, LOG_POWERS, SEED


def median(xs):
    s = sorted(xs)
    k = len(s) // 2
    return s[k] if len(s) % 2 else (s[k - 1] + s[k]) / 2


def ratio_ci(a, b):
    """median(b) / median(a) and its bootstrap percentile interval at
    CONFIDENCE: a and b resampled independently BOOTSTRAP times with a fixed
    seed. Resamples with a median of zero in a are dropped."""
    rng = random.Random(SEED)
    rs = []
    for _ in range(BOOTSTRAP):
        ma = median(rng.choices(a, k=len(a)))
        mb = median(rng.choices(b, k=len(b)))
        if ma > 0:
            rs.append(mb / ma)
    rs.sort()
    tail = (1 - CONFIDENCE) / 2
    lo = rs[int(tail * len(rs))]
    hi = rs[min(len(rs) - 1, int((1 - tail) * len(rs)))]
    return median(b) / median(a), lo, hi


def line(x, y):
    """Least-squares a + b x through the points; (a, b, rms residual)."""
    k = len(x)
    mx, my = sum(x) / k, sum(y) / k
    sxx = sum((u - mx) ** 2 for u in x)
    b = sum((u - mx) * (w - my) for u, w in zip(x, y)) / sxx
    a = my - b * mx
    res = math.sqrt(sum((w - a - b * u) ** 2 for u, w in zip(x, y)) / k)
    return a, b, res


def power_fit(n, y):
    """The model c n^p log^q(n) fitted to y > 0 by least squares in log
    space, log y - q log log n = log c + p log n, for each q in LOG_POWERS
    (q > 0 only where every n > 1); the q with the smallest residual.
    Returns (c, p, q, rms residual of log y)."""
    best = None
    for q in LOG_POWERS:
        if q and min(n) <= 1:
            continue
        ly = [math.log(w) - q * math.log(math.log(u)) for u, w in zip(n, y)]
        a, p, res = line([math.log(u) for u in n], ly)
        if best is None or res < best[3]:
            best = (math.exp(a), p, q, res)
    return best


def local_exponent(p, q, n):
    """d log y / d log n of c n^p log^q(n) at n."""
    return p + q / math.log(n)


def karp_flatt(speedup, p):
    """The serial fraction e = (1/S - 1/p) / (1 - 1/p) of a speedup S on p
    times the base thread count (Karp and Flatt, CACM 1990)."""
    return (1 / speedup - 1 / p) / (1 - 1 / p)
