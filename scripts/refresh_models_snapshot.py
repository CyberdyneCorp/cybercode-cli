#!/usr/bin/env python3
"""Refresh the bundled offline model catalog (provider-catalog → Offline and pinned catalog).

Downloads https://models.dev/api.json (or reads a local file given as the first argument),
keeps providers whose SDK maps to a P0 adapter, drops descriptions, and writes a
reproducible gzip to crates/cyber-llm/snapshot/models.json.gz.
"""

import gzip
import io
import json
import sys
import urllib.request
from pathlib import Path

SUPPORTED_NPM = {
    "@ai-sdk/openai",
    "@ai-sdk/anthropic",
    "@ai-sdk/openai-compatible",
    "@openrouter/ai-sdk-provider",
}
OUT = Path(__file__).resolve().parents[1] / "crates/cyber-llm/snapshot/models.json.gz"


def load(source: str | None) -> dict:
    if source:
        return json.loads(Path(source).read_text())
    with urllib.request.urlopen("https://models.dev/api.json", timeout=30) as resp:
        return json.load(resp)


def trim(catalog: dict) -> dict:
    kept = {}
    for pid in sorted(catalog):
        provider = catalog[pid]
        if provider.get("npm") not in SUPPORTED_NPM:
            continue
        for model in provider.get("models", {}).values():
            model.pop("description", None)
        kept[pid] = provider
    return kept


def main() -> int:
    data = trim(load(sys.argv[1] if len(sys.argv) > 1 else None))
    raw = json.dumps(data, separators=(",", ":"), sort_keys=True).encode()
    buf = io.BytesIO()
    with gzip.GzipFile(fileobj=buf, mode="wb", mtime=0, compresslevel=9) as gz:
        gz.write(raw)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(buf.getvalue())
    models = sum(len(p.get("models", {})) for p in data.values())
    print(f"{OUT}: {len(data)} providers, {models} models, {len(buf.getvalue())} bytes gzipped")
    return 0


if __name__ == "__main__":
    sys.exit(main())
