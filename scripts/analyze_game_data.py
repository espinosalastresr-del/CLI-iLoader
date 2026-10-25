#!/usr/bin/env python3
"""Inventory Unity GameData blobs without assuming their serialization format.

Reports file names, sizes, hashes, entropy, common prefixes, printable strings,
regional variants, and semantic filename groups. Never modifies the APK or assets.
"""
from __future__ import annotations
import collections
import hashlib
import json
import math
import pathlib
import re
import sys

MAX_FILE = 128 * 1024 * 1024
STRING_MIN = 5
REGION_RE = re.compile(r"_(asia|europe)$", re.I)
CATEGORIES = {
    "items/equipment": re.compile(r"item|weapon|armor|equipment|mountskin|wardrobe|armory|carriable", re.I),
    "gathering/resources": re.compile(r"fish|harvest|resource|gather|ore|fiber|hide|wood|stone", re.I),
    "loot/rewards": re.compile(r"loot|treasure|reward|chest|drop|silver|currency", re.I),
    "world/maps/clusters": re.compile(r"cluster|world|map|minimap|tile|biome|region|territor|outpost|hideout", re.I),
    "mobs/combat": re.compile(r"mob|character|boss|combat|spell|ability|damage|arena|gvg|hellgate", re.I),
    "crafting/economy": re.compile(r"craft|recipe|market|trade|currency|refine|economy|building", re.I),
    "localization": re.compile(r"localization|language|translation|text", re.I),
    "templates/scenes": re.compile(r"template|\.template|\.cluster", re.I),
}
def entropy(data: bytes) -> float:
    if not data:
        return 0.0
    counts = collections.Counter(data)
    n = len(data)
    return round(-sum((c / n) * math.log2(c / n) for c in counts.values()), 4)
def strings(data: bytes):
    found = set()
    for m in re.finditer(rb"[\x20-\x7e]{%d,}" % STRING_MIN, data):
        found.add(m.group().decode("ascii", "replace"))
    for m in re.finditer(rb"(?:[\x20-\x7e]\x00){%d,}" % STRING_MIN, data):
        found.add(m.group()[::2].decode("ascii", "replace"))
    return sorted(found)
def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: analyze_game_data.py GAME_DATA_DIR OUTPUT_DIR")
    root, out = map(pathlib.Path, sys.argv[1:])
    out.mkdir(parents=True, exist_ok=True)
    if not root.is_dir():
        (out / "game-data-analysis-status.txt").write_text("GameData directory not found; Apktool may have failed.\n", encoding="utf-8")
        return 0
    files = sorted(p for p in root.rglob("*") if p.is_file())
    rows, prefix_counts, hash_groups, regional = [], collections.Counter(), collections.defaultdict(list), collections.defaultdict(dict)
    string_rows, scanned_bytes = [], 0
    for p in files:
        rel = p.relative_to(root).as_posix()
        size = p.stat().st_size
        if size > MAX_FILE:
            rows.append({"path": rel, "size": size, "skipped": "per-file scan limit"})
            continue
        data = p.read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        prefix = data[:8].hex()
        prefix_counts[prefix] += 1
        hash_groups[digest].append(rel)
        stem = p.stem
        region_match = REGION_RE.search(stem)
        base = REGION_RE.sub("", stem) if region_match else stem
        if region_match:
            regional[base][region_match.group(1).lower()] = {"path": rel, "sha256": digest, "size": size}
        category = next((name for name, pattern in CATEGORIES.items() if pattern.search(rel)), "other")
        row = {
            "path": rel, "size": size, "suffix": p.suffix.lower() or "[no extension]",
            "sha256": digest, "prefix_8_hex": prefix, "entropy_bits_per_byte": entropy(data),
            "category": category, "printable_string_count": 0,
        }
        if len(data) >= 8 and data[:8] == bytes.fromhex("adbaca946fdb30f7"):
            row["format_note"] = "matches repeated 8-byte prefix adbaca946fdb30f7; format/encryption not identified"
        if scanned_bytes < 256 * 1024 * 1024:
            ss = strings(data)
            row["printable_string_count"] = len(ss)
            for s in ss[:200]:
                if len(string_rows) < 20000:
                    string_rows.append({"file": rel, "string": s[:500]})
            scanned_bytes += len(data)
        rows.append(row)
    dupes = [{"sha256": h, "files": paths} for h, paths in hash_groups.items() if len(paths) > 1]
    variants = []
    for base, regions in sorted(regional.items()):
        if len(regions) > 1:
            variants.append({"base_name": base, "variants": regions, "identical": len({v["sha256"] for v in regions.values()}) == 1})
    categories = collections.Counter(r.get("category", "other") for r in rows)
    summary = {
        "root": str(root), "file_count": len(rows), "total_bytes": sum(r.get("size", 0) for r in rows),
        "categories": categories.most_common(), "common_8_byte_prefixes": prefix_counts.most_common(30),
        "files_matching_adbaca946fdb30f7_prefix": sum(1 for r in rows if r.get("prefix_8_hex") == "adbaca946fdb30f7"),
        "duplicate_content_groups": len(dupes), "regional_variant_groups": len(variants),
        "strings_scanned_bytes": scanned_bytes,
        "interpretation": [
            "File names are useful clues, not proof of file semantics.",
            "High entropy and a repeated prefix may indicate compression, encryption, or a custom container; this report does not identify which.",
            "No data files are modified, decrypted, or executed."
        ]
    }
    (out / "game-data-inventory.json").write_text(json.dumps(rows, indent=2), encoding="utf-8")
    (out / "game-data-summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    (out / "game-data-duplicate-files.json").write_text(json.dumps(dupes, indent=2), encoding="utf-8")
    (out / "game-data-regional-variants.json").write_text(json.dumps(variants, indent=2), encoding="utf-8")
    (out / "game-data-printable-strings.json").write_text(json.dumps(string_rows, indent=2), encoding="utf-8")
    lines = ["# Unity GameData static inventory", "", f"- Files: {len(rows)}", f"- Total bytes: {summary['total_bytes']}", f"- Files with prefix adbaca946fdb30f7: {summary['files_matching_adbaca946fdb30f7_prefix']}", f"- Duplicate-content groups: {len(dupes)}", f"- Regional variant groups: {len(variants)}", "", "## Categories"]
    lines += [f"- {name}: {count}" for name, count in categories.most_common()]
    lines += ["", "## Largest files"]
    lines += [f"- {r.get('size', 0):,} bytes — {r.get('path')}" for r in sorted(rows, key=lambda r: r.get("size", 0), reverse=True)[:50]]
    lines += ["", "## Regional variants (first 100)"]
    lines += [f"- {v['base_name']}: " + ", ".join(f"{region}={'same' if v['identical'] else 'different'}" for region in v["variants"]) for v in variants[:100]]
    lines += ["", "## Interpretation", *[f"- {x}" for x in summary["interpretation"]], ""]
    (out / "game-data-summary.md").write_text("\n".join(lines), encoding="utf-8")
    print(f"GameData files: {len(rows)}; total bytes: {summary['total_bytes']}; common-prefix files: {summary['files_matching_adbaca946fdb30f7_prefix']}")
if __name__ == "__main__":
    main()
