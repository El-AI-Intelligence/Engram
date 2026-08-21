#!/usr/bin/env python3
"""Run the memory comparison benchmark.

Conditions (each gets its own cwd; fresh `claude -p` session per question):
  a1  Claude Code native, best case: every memory injected wholesale via
      project CLAUDE.md (fits the ≤200-line guidance at small K).
  a2  Claude Code native, scale case: CLAUDE.md holds only a topic index;
      memories live in memories/<category>.md files the model must find
      and Read proactively (no retrieval layer — recall = guess + Read).
  b   Engram: memories only in a local vault; model reaches them through
      the engramd-mcp MCP server (engram_search/engram_get/...). Measures
      both recall AND tool adoption (whether Claude calls the tools at all).
  c   Control: no memory anywhere — baseline hallucination/refusal rate.

Usage:
    python3 run.py --k 25 --questions 5 --reps 1 --conditions a1,a2,b,c \
        --tag dryrun
Results append to bench/runs/<tag>/results.jsonl (resumable: completed
(condition, rep, question) rows are skipped on re-run).
"""

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

BENCH = Path(__file__).resolve().parent.parent
ROOT = BENCH.parent
RUNS = BENCH / "runs"  # results (persistent, gitignored)
# Condition working dirs live under /tmp so Read/Glob/Grep can't stumble
# onto the corpus or sibling conditions' files (the dry-run proved they do).
WORK = Path("/tmp/engram-bench")
MCP_BINARY = ROOT / "target" / "release" / "engramd-mcp"
DAEMON_PORT = 18789
DAEMON_URL = f"http://127.0.0.1:{DAEMON_PORT}"
EMPTY_MCP = '{"mcpServers":{}}'

CONDITIONS = {
    "a1": "native CLAUDE.md wholesale injection",
    "a2": "native CLAUDE.md index + topic files (proactive Read)",
    "b": "Engram vault via engramd-mcp",
    "c": "control (no memory)",
}


def clean_env():
    return {k: v for k, v in os.environ.items() if not k.startswith("ENGRAM")}


def claude_version():
    return subprocess.run(["claude", "--version"], capture_output=True,
                          text=True).stdout.strip()


def namespace_for(cwd):
    """~/.claude/projects/<ns> encoding: slashes become dashes."""
    return str(cwd).replace("/", "-")


def transcript_path(cwd, session_id):
    return (Path.home() / ".claude" / "projects" / namespace_for(cwd)
            / f"{session_id}.jsonl")


def setup_condition(cond, cwd, memories):
    cwd.mkdir(parents=True, exist_ok=True)
    if cond == "a1":
        lines = ["# Acme project context", "",
                 "Company memory — everything the team knows about Acme:",
                 ""]
        lines += [f"- {m['content']}" for m in memories]
        (cwd / "CLAUDE.md").write_text("\n".join(lines) + "\n")
    elif cond == "a2":
        by_cat = {}
        for m in memories:
            by_cat.setdefault(m["category"], []).append(m)
        files = cwd / "memories"
        files.mkdir(exist_ok=True)
        for cat, ms in sorted(by_cat.items()):
            (files / f"{cat}.md").write_text(
                "\n".join(f"- {m['content']}" for m in ms) + "\n")
        (cwd / "CLAUDE.md").write_text(
            "# Acme project context\n\n"
            "Project memories are stored in topic files under memories/ in "
            "this project. Read the relevant file(s) before answering "
            "questions about Acme.\n")
    elif cond == "b":
        (cwd / "CLAUDE.md").write_text("# Acme project\n")
    elif cond == "c":
        pass  # empty dir
    else:
        raise ValueError(cond)


def mcp_config_path():
    cfg = BENCH / "mcp-engram.json"
    if not MCP_BINARY.exists():
        print(f"building {MCP_BINARY} ...", flush=True)
        subprocess.run(["cargo", "build", "-p", "engramd-mcp", "--release"],
                       cwd=ROOT, check=True)
    cfg.write_text(json.dumps({
        "mcpServers": {
            "engram": {
                "command": str(MCP_BINARY),
                "args": ["--engramd-url", DAEMON_URL],
            }}}))
    return str(cfg)


def ask(cond, cwd, question, mcp_arg):
    """One fresh print-mode session. Returns (json, transcript_text)."""
    cmd = ["claude", "-p", question, "--output-format", "json",
           "--permission-mode", "bypassPermissions",
           "--strict-mcp-config", "--mcp-config", mcp_arg]
    if cond in ("b", "c"):
        # b: Engram MCP tools only — "--tools ''" disables all built-ins
        # (Read/Glob/Grep/Bash) while MCP tools stay available (verified
        # empirically; --allowedTools does NOT restrict, it only adds).
        # c: no tools at all, nothing to find.
        cmd += ["--tools", ""]
    else:
        cmd += ["--tools", "Read,Glob,Grep"]  # whitelist; kills Bash
    for attempt in range(2):
        try:
            r = subprocess.run(cmd, cwd=cwd, env=clean_env(),
                               capture_output=True, text=True, timeout=300)
            j = json.loads(r.stdout)
            return j, _transcript(j, cwd)
        except Exception:
            if attempt == 0:
                time.sleep(2)
                continue
            return None, None
    return j, None


def _transcript(j, cwd):
    sid = j.get("session_id")
    if not sid:
        return ""
    tp = transcript_path(cwd, sid)
    return tp.read_text(errors="replace") if tp.exists() else ""


def normalize(text):
    """Strip markdown emphasis/code + collapse whitespace for matching."""
    for ch in "*_`~":
        text = text.replace(ch, "")
    return " ".join(text.split()).casefold()


def grade(result, q):
    if not result:
        return False
    low = normalize(result)
    keys = [normalize(k) for k in q["keys"]]
    # Cross-reference questions require id AND date: a binary guess alone
    # (50% floor) no longer passes.
    op = all if q.get("type") == "crossref" else any
    return op(k in low for k in keys)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--k", type=int, required=True)
    ap.add_argument("--questions", type=int, default=20)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--max-reps", type=int, default=0,
                    help="cap reps actually run (0 = use --reps); lets a "
                         "half-scale resume skip the later reps")
    ap.add_argument("--conditions", default="a1,a2,b,c")
    ap.add_argument("--tag", required=True)
    ap.add_argument("--seed", type=int, default=42)
    args = ap.parse_args()

    conds = args.conditions.split(",")
    data_dir = BENCH / f"data-k{args.k}"
    memories_file = data_dir / "memories.json"
    questions_file = data_dir / "questions.json"
    need_gen = not memories_file.exists() or not questions_file.exists()
    if not need_gen:
        # Regenerate if the corpus was built with different parameters.
        need_gen = (len(json.loads(memories_file.read_text())) != args.k or
                    len(json.loads(questions_file.read_text())) != args.questions)
    if need_gen:
        subprocess.run([sys.executable, str(BENCH / "memory" / "generate.py"),
                        "--k", str(args.k), "--questions", str(args.questions),
                        "--seed", str(args.seed), "--out", str(data_dir)],
                       check=True)
    memories = json.loads(memories_file.read_text())
    questions = json.loads(questions_file.read_text())

    tag_dir = RUNS / args.tag
    results_file = tag_dir / "results.jsonl"
    results_file.parent.mkdir(parents=True, exist_ok=True)
    done = set()
    if results_file.exists():
        for line in results_file.read_text().splitlines():
            try:
                row = json.loads(line)
                done.add((row["cond"], row["rep"], row["qid"]))
            except Exception:
                pass

    max_reps = args.max_reps or args.reps
    meta = {"tag": args.tag, "k": args.k, "questions": len(questions),
            "reps": min(args.reps, max_reps), "conditions": conds,
            "seed": args.seed,
            "claude_version": claude_version(),
            "started_at": time.strftime("%Y-%m-%dT%H:%M:%S")}
    (tag_dir / "meta.json").write_text(json.dumps(meta, indent=2))

    daemon = None
    if "b" in conds:
        sys.path.insert(0, str(BENCH / "memory"))
        from daemon import BenchDaemon
        daemon = BenchDaemon(vault=BENCH / "vault", port=DAEMON_PORT)
        daemon.start()
        daemon.seed(memories)
        mcp_arg = mcp_config_path()
        print(f"condition b: daemon up at {DAEMON_URL}, "
              f"seeded {len(memories)}", flush=True)
    else:
        mcp_arg = None

    out = open(results_file, "a")
    try:
        for cond in conds:
            mcp = mcp_arg if cond == "b" else EMPTY_MCP
            for rep in range(1, min(args.reps, max_reps) + 1):
                # Unique parent per condition: Grep/Glob of a condition's
                # own tree cannot see sibling conditions' files (a tainted
                # b session once Grep'd the shared parent and read a1's
                # injected CLAUDE.md).
                cwd = WORK / f"{args.tag}-{cond}" / f"r{rep}"
                setup_condition(cond, cwd, memories)
                for q in questions:
                    key = (cond, rep, q["id"])
                    if key in done:
                        continue
                    t0 = time.time()
                    j, tx = ask(cond, cwd, q["text"], mcp)
                    row = {"cond": cond, "rep": rep, "qid": q["id"],
                           "qtype": q["type"], "text": q["text"],
                           "keys": q["keys"]}
                    if j is None:
                        row.update(result=None, ok=False, error="no-json")
                    else:
                        usage = j.get("usage", {})
                        row.update(
                            result=j.get("result"),
                            ok=grade(j.get("result"), q),
                            is_error=j.get("is_error"),
                            num_turns=j.get("num_turns"),
                            in_tokens=usage.get("input_tokens"),
                            out_tokens=usage.get("output_tokens"),
                            cost=j.get("total_cost_usd"),
                            duration_ms=j.get("duration_ms"),
                            session_id=j.get("session_id"))
                    if tx:
                        row["mcp_tools"] = sorted(
                            set(w for w in _tool_names(tx)
                                if w.startswith("mcp__")))
                    row["wall_s"] = round(time.time() - t0, 1)
                    out.write(json.dumps(row) + "\n")
                    out.flush()
                    print(f"[{cond} r{rep} {q['id']}] "
                          f"{'OK ' if row['ok'] else 'MISS'} "
                          f"{row.get('mcp_tools') or ''}", flush=True)
    finally:
        out.close()
        if daemon:
            daemon.stop()

    print(f"\nresults -> {results_file}", flush=True)


def _tool_names(transcript_text):
    """Yield tool names from a session transcript (lazy scan)."""
    import re
    for m in re.finditer(r'"name":"([^"]+)"', transcript_text):
        yield m.group(1)


if __name__ == "__main__":
    main()
