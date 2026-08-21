#!/usr/bin/env python3
"""Generate the synthetic "Acme Labs" memory corpus + question set.

Deterministic (--seed). Every memory is 2-3 sentences (>100 chars, so the
daemon's semantic-layer embedding path applies) and carries one or more
distinctive answer keys (full names, INC ids, ports, dates, codenames) that
make grading a case-insensitive contains-match against the model's reply.
Questions never contain their own answer keys.

Usage:
    python3 generate.py --k 100 --questions 20 --out bench/data
"""

import argparse
import json
import random
import sys
from collections import Counter
from pathlib import Path

FIRST = ["Wendy", "Idris", "Petra", "Soren", "Mira", "Dex", "Yara", "Colm",
         "Anya", "Felix", "Nora", "Ravi", "Tessa", "Bram", "Livia", "Otto",
         "Serena", "Hugo", "Zara", "Milan", "Freya", "Cass", "Iris", "Jonas"]
LAST = ["Quill", "Vane", "Solis", "Kade", "Oswald", "Ferro", "Lindqvist",
        "Marsh", "Taggart", "Okoye", "Brenner", "Delacroix", "Ishida", "Crane",
        "Novak", "Hale", "Sarto", "Wren", "Aalto", "Moreau", "Faraday",
        "Beck", "Tremblay", "Aoki"]
ROLES = ["platform lead", "SRE", "staff engineer", "QA lead",
         "security engineer", "data engineer", "backend engineer",
         "mobile engineer", "design systems lead", "ML engineer",
         "database administrator", "support lead", "site reliability lead",
         "network engineer"]
SERVICES = ["billing-api", "auth-service", "search-indexer",
            "payment-webhooks", "cdn-edge", "inventory-sync",
            "recommendation-feed", "notifications"]
ROOT_CAUSES = [
    "an unbounded connection pool after a deploy",
    "a bad flag rollout that doubled write load",
    "a DNS TTL misconfiguration during failover",
    "an index rebuild that locked the write path",
    "a misconfigured load balancer health check",
    "a leaked feature flag that enabled an unfinished code path",
    "a timezone bug in cron scheduling",
    "a memory leak in the ingest worker",
]
FEATURES = ["batch refunds", "sso group sync", "typed search filters",
            "dead-letter replay", "per-tenant rate cards", "edge cache tags",
            "async export jobs", "realtime usage dashboards"]
DECISIONS = [
    ("database standardization",
     "standardized on Postgres 16, dropping the MySQL legacy shards",
     "Postgres 16"),
    ("queueing layer", "moved from Kafka to NATS to cut self-hosting costs",
     "NATS"),
    ("observability", "adopted OpenTelemetry and retired the old statsd stack",
     "OpenTelemetry"),
    ("deploy cadence", "switched to trunk-based daily deploys behind flags",
     "trunk-based daily deploys"),
    ("frontend framework",
     "migrated the vault UI from Svelte to vanilla ES modules for CSP safety",
     "vanilla ES modules"),
    ("cache policy", "standardized on Redis Cluster with a 300-second TTL cap",
     "300-second"),
    ("incident policy",
     "made every SEV1 require a postmortem within 48 hours and a follow-up owner",
     "48 hours"),
    ("storage tiering",
     "moved analytics archives to S3-compatible cold storage after 90 days",
     "90 days"),
]
BIRDS = ["Nightjar", "Kingfisher", "Magpie", "Kestrel", "Bittern", "Lapwing",
         "Osprey", "Cormorant"]
PRODUCTS = ["mobile payments", "EU expansion", "API v3", "usage metering",
            "AI copilot", "offline sync"]
HOST_PREFIXES = ["acme-cdn-eu", "acme-cdn-us", "acme-db-prod", "acme-k8s-ctl",
                 "acme-vault-01", "acme-mq-eu", "acme-batch-us", "acme-edge-jp"]
BUDGET_ITEMS = ["the observability stack", "GPU capacity", "the support tool",
                "the EU data center", "contractor onboarding",
                "the security audit"]
EVENT_TYPES = ["offsite", "board review", "war room", "capacity review",
               "retrospective", "launch go/no-go"]
# Disjoint pools for overflow synthesis: picking from these instead of the
# static pools keeps question discriminators (role, event type) unique so a
# question can never match two different memories.
ROLES_EXTRA = ["frontend engineer", "devops engineer", "product manager",
               "technical writer", "release manager", "data analyst",
               "security analyst", "support engineer"]
EVENT_TYPES_EXTRA = ["all-hands", "security drill", "QBR", "hackathon",
                     "OKR review", "partner summit"]


def make_rng(seed):
    return random.Random(seed)


def rand_date(rng, start=(2025, 10), end=(2026, 8)):
    import datetime
    d0 = datetime.date(*start, 1)
    d1 = datetime.date(*end, 1)
    days = (d1 - d0).days
    return (d0 + datetime.timedelta(days=rng.randrange(days))).isoformat()


def mk_memories(rng, k):
    mems = []
    pools = dict(
        employees=[(f"{rng.choice(FIRST)} {rng.choice(LAST)}", r)
                   for r in rng.sample(ROLES, len(ROLES))],
        incidents=[(svc, rng.choice(ROOT_CAUSES), rng.choice(["SEV1", "SEV2", "SEV2"]))
                   for svc in rng.sample(SERVICES, len(SERVICES))],
        releases=[(svc, f"v{rng.randrange(2,9)}.{rng.randrange(0,9)}.{rng.randrange(0,9)}",
                   rng.choice(FEATURES)) for svc in rng.sample(SERVICES, len(SERVICES))],
        decisions=list(DECISIONS),
        infra=[(f"{rng.choice(HOST_PREFIXES)}-{rng.randrange(10, 99):02d}",
                f"10.44.{rng.randrange(0, 16)}.{rng.randrange(2, 254)}",
                rng.randrange(20000, 30000)) for _ in range(10)],
        events=[(ev, rand_date(rng)) for ev in
                rng.sample(EVENT_TYPES, len(EVENT_TYPES))],
        budgets=[(item, rng.choice([47, 63, 88, 120, 165, 240]) * 1000)
                 for item in BUDGET_ITEMS],
        products=[(f"Project {bird}", product) for bird, product in
                  zip(rng.sample(BIRDS, 6), rng.sample(PRODUCTS, 6))],
    )
    mems = []
    mid = 0
    for name, role in pools["employees"]:
        mems.append(dict(id=f"m{mid:03d}", category="employee",
            content=(f"{name} is the Acme {role}. They have been with the "
                     f"company since early 2024 and are the first point of "
                     f"contact for anything related to their area."),
            keys=[name], name=name, role=role))
        mid += 1
    for svc, cause, sev in pools["incidents"]:
        date = rand_date(rng)
        inc = f"INC-{rng.randrange(4000, 5999)}"
        mems.append(dict(id=f"m{mid:03d}", category="incident",
            content=(f"On {date} at 14:32 UTC the {svc} service went down for "
                     f"41 minutes, tracked as {inc} ({sev}). The postmortem "
                     f"blamed {cause}."),
            keys=[inc, cause, date], svc=svc, cause=cause, inc=inc, date=date))
        mid += 1
    for svc, ver, feat in pools["releases"]:
        date = rand_date(rng)
        mems.append(dict(id=f"m{mid:03d}", category="release",
            content=(f"{svc} shipped version {ver} on {date}. The headline "
                     f"feature was {feat}, gated behind a rollout flag and "
                     f"announced in the weekly changelog."),
            keys=[ver, feat, date], svc=svc, ver=ver, feat=feat, date=date))
        mid += 1
    for topic, what, term in pools["decisions"]:
        date = rand_date(rng)
        mems.append(dict(id=f"m{mid:03d}", category="decision",
            content=(f"At the {date} architecture review the platform team "
                     f"decided on the {topic}: {what}. The change is tracked "
                     f"in ADR-{rng.randrange(100, 900)} and takes effect next "
                     f"quarter."),
            keys=[term], topic=topic, date=date, term=term))
        mid += 1
    for host, ip, port in pools["infra"]:
        mems.append(dict(id=f"m{mid:03d}", category="infra",
            content=(f"Acme hosts {host} at {ip}. Its admin SSH port is "
                     f"{port}, and it runs nightly backups to the primary "
                     f"storage cluster."),
            keys=[host, ip, str(port)], host=host, ip=ip, port=port))
        mid += 1
    for ev, date in pools["events"]:
        mems.append(dict(id=f"m{mid:03d}", category="event",
            content=(f"The quarterly {ev} took place on {date}. Attendees "
                     f"agreed on a follow-up check-in two weeks later, and "
                     f"notes were filed in the shared workspace."),
            keys=[date], ev=ev, date=date))
        mid += 1
    for item, amt in pools["budgets"]:
        mems.append(dict(id=f"m{mid:03d}", category="budget",
            content=(f"Acme allocated EUR {amt:,} to {item} for the current "
                     f"half. The finance team flagged it as an investment, "
                     f"not an operational cost."),
            keys=[f"EUR {amt:,}", f"{amt:,}"], item=item, amt=amt))
        mid += 1
    for codename, product in pools["products"]:
        mems.append(dict(id=f"m{mid:03d}", category="product",
            content=(f"{codename} is the internal name for the {product} "
                     f"initiative. Leadership reviews its progress every "
                     f"second Tuesday of the month."),
            keys=[codename], codename=codename, product=product))
        mid += 1
    # Overflow synthesis for large K: deterministic round-robin variants.
    while len(mems) < k:
        cat = ["incident", "infra", "release", "employee",
               "decision", "event"][len(mems) % 6]
        if cat == "incident":
            svc, cause = rng.choice(SERVICES), rng.choice(ROOT_CAUSES)
            date, inc = rand_date(rng), f"INC-{rng.randrange(4000, 5999)}"
            mems.append(dict(category=cat,
                content=(f"On {date} at 09:15 UTC the {svc} service degraded "
                         f"for 27 minutes, tracked as {inc} (SEV3). The "
                         f"postmortem blamed {cause}."),
                keys=[inc, cause, date], svc=svc, cause=cause, inc=inc,
                date=date))
        elif cat == "infra":
            host = f"{rng.choice(HOST_PREFIXES)}-{rng.randrange(10, 99):02d}"
            ip = f"10.44.{rng.randrange(0, 16)}.{rng.randrange(2, 254)}"
            port = rng.randrange(20000, 30000)
            mems.append(dict(category=cat,
                content=(f"Acme hosts {host} at {ip}. Its admin SSH port is "
                         f"{port}, and it runs nightly backups to the primary "
                         f"storage cluster."),
                keys=[host, ip, str(port)], host=host, ip=ip, port=port))
        elif cat == "release":
            svc = rng.choice(SERVICES)
            ver = (f"v{rng.randrange(2,9)}.{rng.randrange(0,9)}."
                   f"{rng.randrange(0,9)}")
            feat, date = rng.choice(FEATURES), rand_date(rng)
            mems.append(dict(category=cat,
                content=(f"{svc} shipped version {ver} on {date}. The "
                         f"headline feature was {feat}, gated behind a "
                         f"rollout flag and announced in the weekly "
                         f"changelog."),
                keys=[ver, feat, date], svc=svc, ver=ver, feat=feat,
                date=date))
        elif cat == "employee":
            name = f"{rng.choice(FIRST)} {rng.choice(LAST)}"
            role = rng.choice(ROLES_EXTRA)
            mems.append(dict(category=cat,
                content=(f"{name} is the Acme {role}. They have been with "
                         f"the company since early 2024 and are the first "
                         f"point of contact for anything related to their "
                         f"area."),
                keys=[name], name=name, role=role))
        elif cat == "decision":
            topic, what, term = rng.choice(DECISIONS)
            date = rand_date(rng)
            mems.append(dict(category=cat,
                content=(f"At the {date} architecture review the platform "
                         f"team decided on the {topic}: {what}. The change "
                         f"is tracked in ADR-{rng.randrange(100, 900)} and "
                         f"takes effect next quarter."),
                keys=[term], topic=topic, date=date, term=term))
        elif cat == "event":
            ev, date = rng.choice(EVENT_TYPES_EXTRA), rand_date(rng)
            mems.append(dict(category=cat,
                content=(f"The quarterly {ev} took place on {date}. "
                         f"Attendees agreed on a follow-up check-in two "
                         f"weeks later, and notes were filed in the shared "
                         f"workspace."),
                keys=[date], ev=ev, date=date))
        mems[-1]["id"] = f"m{mid:03d}"
        mid += 1
    rng.shuffle(mems)
    return mems[:k]


def mk_questions(rng, mems, n):
    by_cat = {}
    for m in mems:
        by_cat.setdefault(m["category"], []).append(m)
    # Ambiguity guards: a question whose discriminator (role, event type)
    # matches more than one memory has multiple valid answers. Skip those.
    role_n = Counter(m["role"] for m in by_cat.get("employee", []))
    ev_n = Counter(m["ev"] for m in by_cat.get("event", []))
    used = set()
    qs = []

    def add(text, targets, keys, qtype, overlap=False):
        if not overlap and any(t in used for t in targets):
            return False
        for t in targets:
            used.add(t)
        qs.append(dict(id=f"q{len(qs):03d}", text=text, targets=targets,
                       keys=keys, type=qtype))
        return True

    def take(cat):
        pool = [m for m in by_cat.get(cat, []) if m["id"] not in used]
        return pool[0] if pool else None

    # One question per memory, category templates. Never include the key.
    for m in mems:
        t = None
        if m["category"] == "employee":
            if role_n[m["role"]] > 1:
                continue  # two employees share this role -> ambiguous
            t = (f"Who is the Acme {m['role']}?", [m["id"]], [m["name"]], "lookup")
        elif m["category"] == "incident":
            t = (f"What did the postmortem blame for the {m['svc']} "
                 f"outage of {m['date']}?",
                 [m["id"]], [m["cause"]], "lookup")
        elif m["category"] == "release":
            t = (f"What was the headline feature in {m['svc']} version "
                 f"{m['ver']}?", [m["id"]], [m["feat"]], "lookup")
        elif m["category"] == "decision":
            t = (f"What did the platform team decide about the "
                 f"{m['topic']} at the {m['date']} architecture review?",
                 [m["id"]], [m["term"]], "lookup")
        elif m["category"] == "infra":
            t = (f"What is the admin SSH port of {m['host']}?",
                 [m["id"]], [str(m["port"])], "precise")
        elif m["category"] == "event":
            if ev_n[m["ev"]] > 1:
                continue  # two events of this type -> ambiguous
            t = (f"When did the quarterly {m['ev']} take place?",
                 [m["id"]], [m["date"]], "precise")
        elif m["category"] == "budget":
            t = (f"How much did Acme allocate to {m['item']} for the current "
                 f"half?", [m["id"]], [f"EUR {m['amt']:,}"], "precise")
        elif m["category"] == "product":
            t = (f"What is the internal name of the {m['product']} "
                 f"initiative?", [m["id"]], [m["codename"]], "lookup")
        if t:
            add(*t)

    # Cross-reference questions (2 memories, same category).
    incs = by_cat.get("incident", [])
    for i in range(0, len(incs) - 1, 2):
        a, b = incs[i], incs[i + 1]
        if a["svc"] == b["svc"]:
            continue  # "the X outage or the X outage" reads as ambiguous
        first = a if a["date"] < b["date"] else b
        add(f"Which came first, the {a['svc']} outage or the {b['svc']} "
            f"outage? Answer with the INC id and date of the earlier one.",
            [a["id"], b["id"]], [first["inc"], first["date"]], "crossref",
            overlap=True)

    rng.shuffle(qs)
    cross = [q for q in qs if q["type"] == "crossref"][: max(1, n // 10)]
    rest = [q for q in qs if q not in cross]
    return (cross + rest)[:n]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--k", type=int, default=100)
    ap.add_argument("--questions", type=int, default=20)
    ap.add_argument("--seed", type=int, default=42)
    ap.add_argument("--out", default="bench/data")
    args = ap.parse_args()
    rng = make_rng(args.seed)
    mems = mk_memories(rng, args.k)
    qs = mk_questions(rng, mems, args.questions)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "memories.json").write_text(json.dumps(mems, indent=2))
    (out / "questions.json").write_text(json.dumps(qs, indent=2))
    lens = [len(m["content"]) for m in mems]
    print(f"k={len(mems)} memories (min/mean content len "
          f"{min(lens)}/{sum(lens)//len(lens)}), questions={len(qs)}")
    for q in qs[:3]:
        print("  e.g.", q["text"], "->", q["keys"])
    assert all(len(m["content"]) > 100 for m in mems), "short content"


if __name__ == "__main__":
    main()
