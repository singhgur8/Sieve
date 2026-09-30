"""Runs eval.sh with SIEVE_DUMP=1 for variants (Sieve/ACR PPMs in $P2/out/<variant>).

    python3 dump_variants.py <list> <variant,...>
"""
import os, subprocess, sys
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
lst, vs = sys.argv[1], sys.argv[2].split(",")
env = dict(os.environ, SIEVE_DUMP="1")


def run(v):
    out = subprocess.run([os.path.join(HERE, "eval.sh"), lst, v], env=env, capture_output=True, text=True).stdout
    return v, [l for l in out.splitlines() if "==" in l]


with ThreadPoolExecutor(int(os.environ.get("JOBS", "2"))) as ex:
    for v, lines in ex.map(run, vs):
        print(v, *lines)
