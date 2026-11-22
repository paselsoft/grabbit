#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Rigenera assets/licenses.txt: le licenze delle librerie Rust di Grabbit, raggruppate.

Da lanciare dopo ogni cambio delle dipendenze (il test `licenses_cover_every_crate` lo ricorda):

    python3 scripts/licenses.py
"""
import collections
import json
import pathlib
import subprocess

root = pathlib.Path(__file__).resolve().parent.parent
meta = json.loads(
    subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked"], cwd=root
    )
)
own = set(meta["workspace_members"])
groups = collections.defaultdict(set)
for p in meta["packages"]:
    if p["id"] in own:
        continue
    lic = (p.get("license") or "vedi il pacchetto").replace("/", " OR ")
    groups[lic].add(f"{p['name']} {p['version']}")

lines = [
    "# Licenze delle librerie Rust incluse in Grabbit (generato da scripts/licenses.py).",
    "# Una riga per licenza: licenza<TAB>librerie separate da virgole.",
]
for lic in sorted(groups, key=lambda l: (-len(groups[l]), l)):
    lines.append(f"{lic}\t{', '.join(sorted(groups[lic]))}")
(root / "assets" / "licenses.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")
print(f"{sum(len(v) for v in groups.values())} librerie, {len(groups)} licenze")
