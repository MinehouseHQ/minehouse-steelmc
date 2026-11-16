#!/usr/bin/env python3
"""Average the per-phase fields of Steel's slow-tick warnings in a server log.

Usage: phases.py LOG [SKIP_SECONDS] [MESSAGE]
MESSAGE is "Game tick slow" (default, world phases) or "Server tick slow"
(game-loop steps). Warnings in the first SKIP_SECONDS (default 120) are skipped.
"""
import re,sys,collections
log=open(sys.argv[1],errors='ignore').read(); skip=float(sys.argv[2]) if len(sys.argv)>2 else 120
def ms(v):
    m=re.match(r'([\d.]+)(ns|µs|ms|s)$',v)
    if not m: return None
    return float(m[1])*{'ns':1e-6,'µs':1e-3,'ms':1,'s':1000}[m[2]]
acc=collections.defaultdict(list)
for line in log.splitlines():
    m=re.search(r'([\d.]+)s \[Warn\] '+(sys.argv[3] if len(sys.argv)>3 else 'Game tick slow'),line)
    if not m or float(m[1])<skip: continue
    for k,v in re.findall(r'\((\w+): ([^)]+)\)',line):
        x=ms(v)
        if x is not None: acc[k].append(x)
n=max([len(v) for v in acc.values()] or [0])
print('slow ticks',n)
for k,v in sorted(acc.items(),key=lambda kv:-sum(kv[1])):
    s=sorted(v); 
    if s[-1]<0.5: continue
    print(f"{k:28s} avg {sum(v)/len(v):7.1f}  p95 {s[int(len(s)*.95)-1]:7.1f}  max {s[-1]:7.1f}")
print('avg MSPT samples:', re.findall(r'Average time per tick: ([\d.]+)ms',log))
