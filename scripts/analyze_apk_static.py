#!/usr/bin/env python3
"""Bounded, static inventory and string-indicator analysis for an authorized APK.

This script does not execute the APK, bypass protections, or contact discovered endpoints.
It records evidence and candidate indicators for manual review.
"""
from __future__ import annotations
import collections
import hashlib
import json
import pathlib
import re
import sys
import zipfile

MAX_ENTRIES = 250_000
MAX_EXPANDED = 12 * 1024**3
MAX_ENTRY_SCAN = 64 * 1024**2
MAX_TOTAL_SCAN = 1024**3
STRING_MIN = 5

URL_RE = re.compile(rb'(?i)\b(?:https?|wss?)://[A-Z0-9._~:/?#\[\]@!$&\'()*+,;=%-]{4,512}')
DOMAIN_RE = re.compile(rb'(?i)\b(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+(?:com|net|org|io|gg|co|app|dev|cloud|online|game|games|eu|uk|de|fr|ru|cn|jp|kr|tv|me|info|tech|services|xyz)\b')
IP_RE = re.compile(rb'\b(?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d)\b')
PATH_RE = re.compile(rb'(?i)(?:/api/[A-Za-z0-9_./{}?=&%-]{2,180}|/v[0-9]+/[A-Za-z0-9_./{}?=&%-]{2,180})')
KEYWORDS = re.compile(
    r'(?i)(https?://|wss?://|api|endpoint|graphql|websocket|socket|protobuf|grpc|'
    r'auth|oauth|token|login|session|certificate|pinning|trustmanager|keystore|'
    r'telemetry|analytics|sentry|crashlytics|firebase|adjust|appsflyer|'
    r'cloudflare|playfab|photon|unity|il2cpp|addressables|assetbundle|'
    r'httpclient|curl|openssl|boringssl|tls|ssl|inventory|market|guild|chat|'
    r'combat|character|world|zone|map|item|silver|auction|fishing|resource)'
)
def extract_strings(data: bytes):
    # Printable ASCII and UTF-16LE strings are useful across native binaries and Unity assets.
    for m in re.finditer(rb'[\x20-\x7e]{%d,}' % STRING_MIN, data):
        yield m.group().decode('ascii', 'replace')
    for m in re.finditer(rb'(?:[\x20-\x7e]\x00){%d,}' % STRING_MIN, data):
        yield m.group()[::2].decode('ascii', 'replace')

def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: analyze_apk_static.py APK OUTPUT_DIR")
    apk, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(apk) as z:
        infos = z.infolist()
        if len(infos) > MAX_ENTRIES:
            raise SystemExit(f"Too many APK entries: {len(infos)}")
        expanded = sum(max(0, i.file_size) for i in infos)
        if expanded > MAX_EXPANDED:
            raise SystemExit(f"Expanded APK exceeds safety limit: {expanded}")
        rows = []
        extensions = collections.Counter()
        top_dirs = collections.Counter()
        native, dex, unity, configs = [], [], [], []
        for i in infos:
            p = i.filename
            suffix = pathlib.PurePosixPath(p).suffix.lower() or "[no extension]"
            extensions[suffix] += 1
            top_dirs[p.split("/", 1)[0]] += 1
            row = {"path": p, "size": i.file_size, "compressed_size": i.compress_size,
                   "crc32": f"{i.CRC:08x}"}
            rows.append(row)
            pl = p.lower()
            if pl.endswith((".so", ".dll", ".dylib")): native.append(row)
            if pl.endswith((".dex", ".odex", ".vdex")): dex.append(row)
            if "assets/bin/data" in pl or "global-metadata.dat" in pl or "addressable" in pl or "assetbundle" in pl: unity.append(row)
            if pl.endswith((".json", ".xml", ".properties", ".ini", ".cfg", ".config", ".plist", ".txt", ".yaml", ".yml")): configs.append(row)
        (out / "apk-files.json").write_text(json.dumps(rows, indent=2), encoding="utf-8")
        (out / "apk-files.txt").write_text("\n".join(r["path"] for r in rows) + "\n", encoding="utf-8")
        (out / "apk-extension-summary.json").write_text(json.dumps(
            {"entry_count": len(rows), "expanded_bytes": expanded,
             "extensions": extensions.most_common(), "top_level_directories": top_dirs.most_common()},
            indent=2), encoding="utf-8")
        (out / "apk-largest-files.json").write_text(json.dumps(sorted(rows, key=lambda r:r["size"], reverse=True)[:300], indent=2), encoding="utf-8")
        (out / "native-libraries.json").write_text(json.dumps(native, indent=2), encoding="utf-8")
        (out / "dex-files.json").write_text(json.dumps(dex, indent=2), encoding="utf-8")
        (out / "unity-assets-and-metadata.json").write_text(json.dumps(unity, indent=2), encoding="utf-8")
        (out / "configuration-and-text-files.json").write_text(json.dumps(configs, indent=2), encoding="utf-8")
        indicators = collections.defaultdict(set)
        scanned, files_scanned = 0, 0
        for i in infos:
            if scanned >= MAX_TOTAL_SCAN:
                break
            if i.is_dir() or i.file_size <= 0 or i.file_size > MAX_ENTRY_SCAN:
                continue
            pl = i.filename.lower()
            # Prioritize code, config, metadata, and native libraries; also sample ordinary assets.
            relevant = (pl.endswith((".so", ".dex", ".json", ".xml", ".txt", ".dat", ".dll", ".dylib", ".bytes", ".asset", ".config", ".properties", ".bin"))
                        or any(x in pl for x in ("manifest", "metadata", "config", "network", "addressable", "resource")))
            if not relevant:
                continue
            try:
                with z.open(i) as f:
                    data = f.read(min(i.file_size, MAX_ENTRY_SCAN) + 1)
                if len(data) > MAX_ENTRY_SCAN:
                    data = data[:MAX_ENTRY_SCAN]
            except Exception as e:
                indicators["scan_errors"].add(f"{i.filename}: {type(e).__name__}: {e}")
                continue
            scanned += len(data)
            files_scanned += 1
            found = set()
            for raw in URL_RE.findall(data): found.add(("url", raw.decode("ascii", "replace").rstrip(".,;:)")))
            for raw in DOMAIN_RE.findall(data): found.add(("domain", raw.decode("ascii", "replace")))
            for raw in IP_RE.findall(data): found.add(("ipv4_candidate", raw.decode("ascii", "replace")))
            for raw in PATH_RE.findall(data): found.add(("api_path_candidate", raw.decode("ascii", "replace")))
            for s in extract_strings(data):
                if KEYWORDS.search(s):
                    found.add(("keyword_string", s[:500]))
            for kind, value in found:
                indicators[kind].add(f"{i.filename}\t{value}")
        result = {k: sorted(v) for k, v in sorted(indicators.items())}
        (out / "static-string-indicators.json").write_text(json.dumps(
            {"files_scanned": files_scanned, "bytes_scanned": scanned, "scan_limit_bytes": MAX_TOTAL_SCAN,
             "indicators": result}, indent=2), encoding="utf-8")
        # Human-readable index, useful on mobile.
        lines = [f"entries={len(rows)}", f"expanded_bytes={expanded}",
                 f"files_scanned={files_scanned}", f"bytes_scanned={scanned}", ""]
        for kind, values in sorted(result.items()):
            lines += [f"## {kind} ({len(values)})"]
            lines += ["- " + v for v in values[:300]]
            lines.append("")
        (out / "static-string-indicators.txt").write_text("\n".join(lines), encoding="utf-8")
if __name__ == "__main__":
    main()
