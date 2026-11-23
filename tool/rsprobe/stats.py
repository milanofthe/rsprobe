"""Medians and bootstrap intervals."""

import random

from .constants import BOOTSTRAP, CONFIDENCE, SEED


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
