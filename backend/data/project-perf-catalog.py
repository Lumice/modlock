#!/usr/bin/env python3
"""Write Modlock's performance catalog from a local copy of the pinned upstream JSON."""

import argparse
import hashlib
import json
from pathlib import Path
import sys


UPSTREAM_SHA256 = "f7a36be0a023e71d2830ad3a85253ec4d721c3fd6ea80ba1d63120cfbc1e8280"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("upstream_catalog", type=Path)
    args = parser.parse_args()
    upstream = args.upstream_catalog.read_bytes()
    if hashlib.sha256(upstream).hexdigest() != UPSTREAM_SHA256:
        parser.error("catalog bytes do not match the upstream revision in perf-PROVENANCE.md")

    catalog = json.loads(upstream)
    descriptions = json.loads(Path(__file__).with_name("perf-descriptions.json").read_bytes())
    preset_convars = {
        entry["path"][1].lower()
        for preset in catalog["presets"]
        for entry in preset["entries"]
        if entry["path"][0].lower() == "convars"
    }
    dumped_help = {row["help"] for row in catalog["convars"] if row.get("help")}
    needed_descriptions = {
        row["name"]
        for row in catalog["convars"]
        if (row["name"].lower() in preset_convars and row.get("help") and not row.get("description"))
        or row.get("description") in dumped_help
    }
    if needed_descriptions != descriptions.keys():
        parser.error("the authored descriptions must cover exactly the preset help entries")

    for row in catalog["convars"]:
        row.pop("help", None)
        row.pop("enumValues", None)
        if row["name"] in descriptions:
            row["description"] = descriptions[row["name"]]
    catalog["community"] = []
    sys.stdout.write(json.dumps(catalog, ensure_ascii=False, separators=(",", ":")) + "\n")


if __name__ == "__main__":
    main()
