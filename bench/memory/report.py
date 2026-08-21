#!/usr/bin/env python3
"""Aggregate benchmark results into recall/token/latency tables + markdown.

Usage:
    python3 report.py --tag dryrun
Writes bench/runs/<tag>/report.md and prints a console summary.
"""

import argparse
import json
import statistics
from pathlib import Path

BENCH = Path(__file__).resolve().parent.parent
RUNS = BENCH / "runs"

COND_NAMES = {
    "a1": "Native A1 (CLAUDE.md, all injected)",
    "a2": "Native A2 (index + topic files)",
    "b": "Engram (MCP vault)",
    "c": "Control (no memory)",
}


def pct(n, d):
    return "—" if d == 0 else f"{100.0 * n / d:.0f}%"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--tag", required=True)
    args = ap.parse_args()
    tag_dir = RUNS / args.tag
    rows = [json.loads(l) for l in
            (tag_dir / "results.jsonl").read_text().splitlines() if l.strip()]
    meta = json.loads((tag_dir / "meta.json").read_text())
    # Conditions from the data, not meta (partial re-runs overwrite meta).
    conds = [c for c in ("a1", "a2", "b", "c") if c in {r["cond"] for r in rows}]

    lines = []
    def out(s=""):
        lines.append(s)
        print(s)

    out(f"# Memory benchmark — {meta['tag']} "
        f"(k={meta['k']}, {meta['questions']} questions, {meta['reps']} reps, "
        f"claude {meta['claude_version']})")
    out()
    out("## Recall by condition")
    out()
    out("| Condition | Correct | Total | Recall |")
    out("|---|---|---|---|")
    for cond in conds:
        cr = [r for r in rows if r["cond"] == cond]
        ok = sum(1 for r in cr if r["ok"])
        out(f"| {COND_NAMES.get(cond, cond)} | {ok} | {len(cr)} | "
            f"{pct(ok, len(cr))} |")

    out()
    out("## Cost per condition (mean per question)")
    out()
    out("| Condition | In tokens | Out tokens | Cost USD | Wall s | "
        "Turns |")
    out("|---|---|---|---|---|---|")
    for cond in conds:
        cr = [r for r in rows if r["cond"] == cond]
        if not cr:
            continue

        def avg(key):
            vals = [r[key] for r in cr if r.get(key) is not None]
            return f"{statistics.mean(vals):.0f}" if vals else "—"

        cost = sum(r.get("cost") or 0 for r in cr)
        out(f"| {COND_NAMES.get(cond, cond)} | {avg('in_tokens')} | "
            f"{avg('out_tokens')} | {cost:.2f} | {avg('wall_s')} | "
            f"{avg('num_turns')} |")

    out()
    out("## Recall by question type")
    out()
    qtypes = sorted({r["qtype"] for r in rows})
    out("| Type | " + " | ".join(conds) + " |")
    out("|" + "---|" * (len(conds) + 1))
    for qt in qtypes:
        cells = []
        for cond in conds:
            cr = [r for r in rows if r["cond"] == cond and r["qtype"] == qt]
            cells.append(pct(sum(1 for r in cr if r["ok"]), len(cr)))
        out(f"| {qt} | " + " | ".join(cells) + " |")

    b = [r for r in rows if r["cond"] == "b"]
    if b:
        adopted = sum(1 for r in b if r.get("mcp_tools"))
        calls = {t: sum(1 for r in b if t in r.get("mcp_tools", []))
                 for t in sorted({t for r in b for t in r.get("mcp_tools", [])})}
        out()
        out("## Engram tool adoption (condition b)")
        out()
        out(f"Questions where Claude called ≥1 Engram tool: "
            f"{adopted}/{len(b)} ({pct(adopted, len(b))})")
        out()
        out("| Tool | Questions used |")
        out("|---|---|")
        for t, n in sorted(calls.items(), key=lambda kv: -kv[1]):
            out(f"| {t} | {n} |")

    out()
    out("## Misses (condition b + best native) — what retrieval dropped")
    out()
    focus = sorted(set(r["cond"] for r in rows) - {"c"})
    for cond in focus:
        misses = [r for r in rows if r["cond"] == cond and not r["ok"]]
        if not misses:
            continue
        out(f"- **{COND_NAMES.get(cond, cond)}**: {len(misses)} misses")
        for m in misses[:8]:
            ans = (m.get("result") or "NO ANSWER")[:120].replace("\n", " ")
            out(f"  - q{m['qid']} keys={m['keys']} got: {ans}")
    out()
    out(f"Results: `{tag_dir}/results.jsonl` · regen: "
        f"`python3 bench/memory/report.py --tag {meta['tag']}`")

    (tag_dir / "report.md").write_text("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
