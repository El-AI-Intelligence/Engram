#!/usr/bin/env python3
"""Bench daemon lifecycle: start engramd on a bench port, seed, wipe, stop.

The bench vault lives at bench/vault (persistent, so the MiniLM embedding
model under vault/.engram/models downloads once). Everything else in the
vault is wiped between reps. The daemon runs in dev mode: no ENGRAMD_API_KEY,
no passphrase, loopback only — and this module strips any ENGRAM* env vars
from the child environment so the user's real credentials can never leak in.

Usage (library):
    from daemon import BenchDaemon
    d = BenchDaemon(vault=..., port=18789, binary=...)
    d.start(); d.seed(memories); ...; d.stop()
    d.wipe()   # stop, delete vault contents except .engram (model cache)
"""

import json
import os
import shutil
import signal
import subprocess
import time
import urllib.request
from pathlib import Path

BENCH_DIR = Path(__file__).resolve().parent.parent  # bench/
VAULT_DIR = BENCH_DIR / "vault"
LOG_FILE = BENCH_DIR / "daemon.log"
PORT = 18789
BASE_URL = f"http://127.0.0.1:{PORT}"


def _clean_env():
    """Child env with all ENGRAM* vars removed (no real creds leak)."""
    return {k: v for k, v in os.environ.items() if not k.startswith("ENGRAM")}


def find_binary():
    for cand in [BENCH_DIR.parent / "target" / "release" / "engramd",
                 "engramd"]:
        if cand != "engramd" and Path(cand).exists():
            return str(cand)
    if shutil.which("engramd"):
        return shutil.which("engramd")
    raise FileNotFoundError("engramd binary not found (target/release or PATH)")


class BenchDaemon:
    def __init__(self, vault=VAULT_DIR, port=PORT, binary=None):
        self.vault = Path(vault)
        self.port = port
        self.base_url = f"http://127.0.0.1:{port}"
        self.binary = binary or find_binary()
        self.proc = None

    def start(self):
        if self.proc is not None:
            return
        self._kill_port_owner()  # stale daemon from a previous crashed run
        self.vault.mkdir(parents=True, exist_ok=True)
        log = open(LOG_FILE, "ab")
        self.proc = subprocess.Popen(
            [self.binary, "--vault", str(self.vault),
             "--bind", f"127.0.0.1:{self.port}"],
            stdout=log, stderr=subprocess.STDOUT, env=_clean_env(),
            preexec_fn=os.setsid)
        deadline = time.time() + 60
        while time.time() < deadline:
            if self.proc.poll() is not None:
                raise RuntimeError(
                    f"engramd exited early ({self.proc.returncode}); "
                    f"see {LOG_FILE}")
            try:
                with urllib.request.urlopen(
                        f"{self.base_url}/health", timeout=2) as r:
                    if r.status == 200:
                        return
            except Exception:
                time.sleep(0.5)
        raise TimeoutError(f"daemon did not become healthy; see {LOG_FILE}")

    def seed(self, memories):
        """POST each memory to /memories (semantic layer so embeddings apply)."""
        for m in memories:
            body = json.dumps({"content": m["content"], "layer": "semantic",
                               "project": "acme"}).encode()
            req = urllib.request.Request(
                f"{self.base_url}/memories", data=body,
                headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(req, timeout=60) as r:
                if r.status != 200:
                    raise RuntimeError(f"seed failed: {r.status}")
        print(f"seeded {len(memories)} memories into {self.base_url}",
              flush=True)

    def count(self):
        """Total via the list path (query '' -> list; total = len, capped
        at the route's 100-result clamp — a sanity check, not a census)."""
        body = json.dumps({"query": "", "limit": 100}).encode()
        req = urllib.request.Request(
            f"{self.base_url}/memories/search", data=body,
            headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.load(r).get("total", "?")

    def _kill_port_owner(self):
        """Kill whatever process listens on our port (stale bench daemons)."""
        subprocess.run(["fuser", "-k", f"{self.port}/tcp"],
                       capture_output=True, timeout=10)
        time.sleep(0.3)

    def stop(self):
        if self.proc is None:
            return
        try:
            os.killpg(os.getpgid(self.proc.pid), signal.SIGTERM)
        except (ProcessLookupError, PermissionError):
            pass
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(os.getpgid(self.proc.pid), signal.SIGKILL)
            self.proc.wait(timeout=5)
        self.proc = None
        self._kill_port_owner()  # belt-and-suspenders for orphaned daemons

    def wipe(self):
        """Stop and clear the vault, keeping .engram (the model cache)."""
        self.stop()
        if self.vault.exists():
            for child in self.vault.iterdir():
                if child.name == ".engram":
                    continue
                shutil.rmtree(child) if child.is_dir() else child.unlink()


if __name__ == "__main__":
    import sys
    d = BenchDaemon()
    if sys.argv[1:] == ["up"]:
        d.start()
        print(f"daemon up at {d.base_url}, count={d.count()}")
    elif sys.argv[1:] == ["down"]:
        d.stop()
        print("daemon stopped")
