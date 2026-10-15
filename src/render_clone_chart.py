#!/usr/bin/env python3
"""Render a compact, plot-only clone history SVG from Repo Growth history."""

import json
from datetime import datetime
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
HISTORY = ROOT / ".github/repo-growth/history.json"
OUTPUT = ROOT / "assets/repo-growth-clones.svg"

WIDTH = 800
HEIGHT = 292
LEFT = 62
RIGHT = 782
TOP = 24
BOTTOM = 220


def esc(value):
    return (str(value).replace("&", "&amp;").replace("<", "&lt;")
            .replace(">", "&gt;").replace('"', "&quot;"))


def main():
    data = json.loads(HISTORY.read_text(encoding="utf-8"))
    points = [p for p in data.get("points", []) if p.get("clones") is not None]
    if not points:
        raise SystemExit("No clone history points found")

    values = [int(p["clones"]) for p in points]
    dates = [p["date"] for p in points]
    data_low, data_high = min(values), max(values)
    if data_low == data_high:
        data_low = max(0, data_low - 1)
        data_high += 1
    span = data_high - data_low
    # A little headroom keeps the line from touching the chart's top edge.
    low = max(0, data_low - span * 0.08)
    high = data_high + span * 0.08

    def x_at(i):
        return LEFT if len(points) == 1 else LEFT + i * (RIGHT - LEFT) / (len(points) - 1)

    def y_at(value):
        return BOTTOM - (value - low) * (BOTTOM - TOP) / (high - low)

    path = " ".join(
        ("M" if i == 0 else "L") + f"{x_at(i):.1f} {y_at(v):.1f}"
        for i, v in enumerate(values)
    )
    # Three value ticks and three date labels keep the plot easy to scan.
    tick_values = [data_high, (data_high + data_low) / 2, data_low]
    grid = []
    for value in tick_values:
        y = y_at(value)
        label = str(max(0, round(value)))
        grid.append(
            f'<line x1="{LEFT - 5}" y1="{y:.1f}" x2="{LEFT}" y2="{y:.1f}" '
            'stroke="#f4f4f5" stroke-width="2" />'
            f'<text x="{LEFT - 13}" y="{y + 5:.1f}" text-anchor="end" '
            'fill="#f4f4f5" font-size="15">'
            f'{esc(label)}</text>'
        )

    date_indices = sorted(set((0, (len(points) - 1) // 2, len(points) - 1)))
    date_labels = []
    for i in date_indices:
        dt = datetime.strptime(dates[i], "%Y-%m-%d")
        anchor = "start" if i == 0 else "end" if i == len(points) - 1 else "middle"
        date_labels.append(
            f'<text x="{x_at(i):.1f}" y="{BOTTOM + 25}" text-anchor="{anchor}" '
            'fill="#f4f4f5" font-size="15">'
            f'{dt.strftime("%b %-d, %Y")}</text>'
        )

    updated_at = data.get("updatedAt", "")
    updated = ""
    if updated_at:
        try:
            updated_date = datetime.fromisoformat(updated_at.replace("Z", "+00:00"))
            updated = updated_date.strftime("Updated on %b %-d, %Y")
        except ValueError:
            updated = "Updated"

    svg = f'''<svg xmlns="http://www.w3.org/2000/svg" width="800" height="292" viewBox="0 0 800 292" role="img" aria-labelledby="title desc">
  <title id="title">Hyprly clone history</title>
  <desc id="desc">Cumulative Git clone history over time.</desc>
  <rect width="800" height="292" fill="#0d1015" />
  {''.join(grid)}
  <line x1="{LEFT}" y1="{TOP}" x2="{LEFT}" y2="{BOTTOM}" stroke="#f4f4f5" stroke-width="2" />
  <line x1="{LEFT}" y1="{BOTTOM}" x2="{RIGHT}" y2="{BOTTOM}" stroke="#f4f4f5" stroke-width="2" />
  <path d="{path}" fill="none" stroke="#ff686f" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" />
  {''.join(date_labels)}
  <text x="{(LEFT + RIGHT) / 2}" y="274" text-anchor="middle" fill="#f4f4f5" font-size="14">Date</text>
  <text x="18" y="{(TOP + BOTTOM) / 2}" transform="rotate(-90 18 {(TOP + BOTTOM) / 2})" text-anchor="middle" fill="#f4f4f5" font-size="14">Git clones</text>
  <text x="{LEFT}" y="289" fill="#a1a1aa" font-size="10">{esc(updated)}</text>
</svg>
'''
    OUTPUT.write_text(svg, encoding="utf-8")


if __name__ == "__main__":
    main()
