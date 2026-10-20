#!/usr/bin/env python3
"""
Flame Graph SVG Analyzer for codefeed.
Parses flamegraph.svg and extracts human-readable text summaries,
categorized by project modules, system I/O, concurrency runtime, and hotspots.
"""

from __future__ import annotations

import argparse
import html
import re
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass
class FrameSample:
    name: str
    samples: int
    pct: float


def parse_flamegraph(svg_path: Path) -> tuple[int, list[FrameSample]]:
    if not svg_path.is_file():
        raise FileNotFoundError(f"Flame graph file not found: {svg_path}")

    content = svg_path.read_text(encoding="utf-8", errors="replace")

    total_samples = 0
    total_match = re.search(r'total_samples="(\d+)"', content)
    if total_match:
        total_samples = int(total_match.group(1))
    else:
        all_match = re.search(r"<title>all\s+\((\d+)\s+samples,", content)
        if all_match:
            total_samples = int(all_match.group(1))

    pattern = re.compile(r"<title>([^<]+?)\s+\((\d+)\s+samples,\s+([\d.]+)%\)</title>")
    frames: dict[str, FrameSample] = {}

    for match in pattern.finditer(content):
        raw_name = html.unescape(match.group(1).strip())
        # Strip Windows binary prefix (e.g. "codefeed.exe`")
        clean_name = re.sub(r"^[^`]+`", "", raw_name)

        samples = int(match.group(2))
        pct = float(match.group(3))

        if clean_name not in frames or samples > frames[clean_name].samples:
            frames[clean_name] = FrameSample(name=clean_name, samples=samples, pct=pct)

    frame_list = sorted(frames.values(), key=lambda f: f.samples, reverse=True)
    if total_samples == 0 and frame_list:
        total_samples = max(f.samples for f in frame_list)

    return total_samples, frame_list


def print_table(title: str, items: list[FrameSample], limit: int = 15):
    print(f"\n=== {title} ===")
    if not items:
        print("  (No matching samples found)")
        return

    print(f"{'Samples':>8}  {'Share':>7}  Function")
    print(f"{'-'*8}  {'-'*7}  {'-'*65}")
    for item in items[:limit]:
        print(f"{item.samples:>8}  {item.pct:>6.2f}%  {item.name}")
    if len(items) > limit:
        print(f"  ... and {len(items) - limit} more entries.")


def analyze(svg_path: Path, top_n: int, keyword_filter: str | None = None):
    total_samples, frames = parse_flamegraph(svg_path)
    print(f"Flame Graph Summary: {svg_path}")
    print(f"Total Captured Samples: {total_samples}")

    if keyword_filter:
        filtered = [f for f in frames if keyword_filter.lower() in f.name.lower()]
        print_table(f"Filter: '{keyword_filter}'", filtered, limit=top_n)
        return

    # 1. Project Functions (codefeed::*)
    codefeed_frames = [
        f for f in frames
        if f.name.startswith("codefeed::")
        and not f.name.endswith("main()")
        and not f.name.endswith("main")
    ]
    print_table("codefeed Project Functions", codefeed_frames, limit=top_n)

    # 2. Filesystem & OS I/O Calls (word-boundary match to avoid 'thread' matching 'read')
    io_regex = re.compile(r"(fs::|File::|\bread\b|\bread_|\bopen\b|\blstat\b|\bstat\b|\bmetadata\b|\bwrite\b)", re.IGNORECASE)
    io_frames = [
        f for f in frames
        if io_regex.search(f.name.split("<")[0])
        and not f.name.startswith("codefeed::")
        and not any(tk in f.name for tk in ["thread", "panic", "unwind"])
    ]
    print_table("Filesystem & OS I/O Calls", io_frames, limit=10)

    # 3. Concurrency & Worker Runtime
    runtime_regex = re.compile(r"(ignore::walk|\bthread\b|\bpark\b|\bsync_channel\b|\bfutex\b|\bscope\b|\bWorker\b)", re.IGNORECASE)
    runtime_frames = [
        f for f in frames
        if runtime_regex.search(f.name.split("<")[0])
        and not f.name.startswith("codefeed::")
    ]
    print_table("Concurrency & Worker Runtime", runtime_frames, limit=10)

    # 4. Top Overall Hotspots (excluding entry stubs and Windows kernel DLL addresses)
    clean_top = [
        f for f in frames
        if f.name != "all"
        and not f.name.startswith("0x")
        and not f.name.startswith("`0x")
        and not f.name.endswith("main")
        and not f.name.endswith("main()")
        and "__scrt_common_main" not in f.name
    ]
    print_table(f"Top {top_n} Overall Application Hotspots", clean_top, limit=top_n)


def main():
    parser = argparse.ArgumentParser(
        description="Analyze flamegraph.svg and produce human-readable performance summaries."
    )
    parser.add_argument(
        "file",
        nargs="?",
        default="flamegraph.svg",
        help="Path to the flamegraph.svg file (default: flamegraph.svg in current dir)",
    )
    parser.add_argument(
        "-n", "--top",
        type=int,
        default=15,
        help="Number of top functions to display per table (default: 15)",
    )
    parser.add_argument(
        "-f", "--filter",
        type=str,
        default=None,
        help="Filter functions matching this keyword",
    )

    args = parser.parse_args()
    svg_path = Path(args.file)

    try:
        analyze(svg_path, top_n=args.top, keyword_filter=args.filter)
    except FileNotFoundError as err:
        print(f"Error: {err}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
