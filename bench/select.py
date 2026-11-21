import statistics, sys
from collections import defaultdict
times = defaultdict(list)
for line in open(sys.argv[1]):
    seed, variant, seconds, rss = line.split()
    times[seed, variant].append(float(seconds))
# Select using the median end-to-end time across repeats at each seed.
for variant in ("parser", "radix", "click", "time"):
    ratios = [statistics.median(times[s, variant]) / statistics.median(times[s, "base"]) for s in ("42", "98")]
    if statistics.mean(ratios) < 0.995 and max(ratios) < 1.025:
        print(variant)
