#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Build crates/editor/data/ja-dict.txt.gz from Mozc's open-source dictionary.

    for i in 00 01 02 03 04 05 06 07 08 09; do
      curl -O https://raw.githubusercontent.com/google/mozc/master/src/data/dictionary_oss/dictionary$i.txt
    done
    python3 scripts/build-kana-kanji-dict.py dictionary*.txt

The Mozc dictionary is BSD-3-Clause, Copyright Google Inc. (see
THIRD_PARTY_NOTICES.md). Each line is: reading, left id, right id, cost,
surface. Only the readings and surfaces are kept: the candidates for a
reading, cheapest first, at most MAX_CANDS each, leaving out the very
unlikely ones.
"""
import gzip
import sys
from collections import defaultdict

MAX_CANDS = 5
MAX_COST = 7500

by_reading = defaultdict(dict)
for path in sys.argv[1:]:
    for line in open(path, encoding="utf-8"):
        f = line.rstrip("\n").split("\t")
        if len(f) != 5:
            continue
        reading, _, _, cost, surface = f
        cost = int(cost)
        if cost > MAX_COST:
            continue
        old = by_reading[reading].get(surface)
        if old is None or cost < old:
            by_reading[reading][surface] = cost

out = []
for reading in sorted(by_reading):
    cands = sorted(by_reading[reading].items(), key=lambda kv: (kv[1], kv[0]))
    surfaces = [s for s, _ in cands[:MAX_CANDS]]
    out.append(reading + "\t" + "\t".join(s.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n") for s in surfaces))
data = ("\n".join(out) + "\n").encode()
with gzip.GzipFile("crates/editor/data/ja-dict.txt.gz", "wb", 9, mtime=0) as g:
    g.write(data)
print(len(out), "readings,", len(data), "bytes")
