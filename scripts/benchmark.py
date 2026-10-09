#!/usr/bin/python3
"""Measure release CLI cache-only latency against the offline sandbox."""
from pathlib import Path
import statistics
import subprocess
import time
root=Path(__file__).resolve().parents[1]
command=[str(root/'target/release/ai-quota'),'status','--compact','--config',str(root/'sandbox/config.json'),'--cache',str(root/'sandbox/cache/status.json')]
times=[]
for _ in range(100):
    start=time.perf_counter();subprocess.run(command,check=True,stdout=subprocess.DEVNULL);times.append((time.perf_counter()-start)*1000)
print(f'100 cache-only prompts: median={statistics.median(times):.2f}ms p95={sorted(times)[94]:.2f}ms max={max(times):.2f}ms')
