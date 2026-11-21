#!/usr/bin/env python3
"""Refresh read-only monthly counters and render the README activity chart."""

from datetime import datetime, timezone
from html import escape
import json
import math
from pathlib import Path
import sys
from urllib.request import Request, urlopen
from urllib.parse import urlencode

ROOT = Path(__file__).resolve().parent.parent
HISTORY = ROOT / ".github/activity/history.json"
OUTPUT = ROOT / "assets/count.svg"
README = ROOT / "README.md"
NAMESPACE = "patermars-hyprly"
ACTION = "monthly-connected"
LINK_START = "<!-- activity-counter:start -->"
LINK_END = "<!-- activity-counter:end -->"
BADGE_START = "<!-- activity-badge:start -->"
BADGE_END = "<!-- activity-badge:end -->"


def months_since(start, now):
    first = datetime.strptime(start, "%Y-%m")
    if first.strftime("%Y-%m") != start:
        raise ValueError("Invalid starting month")
    year, month = first.year, first.month
    while (year, month) <= (now.year, now.month):
        yield f"{year:04}-{month:02}"
        year, month = (year + 1, 1) if month == 12 else (year, month + 1)


def counter_url(month):
    # Never omit this flag: the default endpoint operation increments the counter.
    return f"https://counterapi.com/api/{NAMESPACE}/{ACTION}/{month}?readOnly=true"


def update_counter_link(text, now):
    # The README owner may remove this optional section during documentation cleanup.
    if LINK_START not in text and LINK_END not in text:
        return text
    month = now.strftime("%Y-%m")
    start = text.index(LINK_START) + len(LINK_START)
    end = text.index(LINK_END, start)
    link = f"[View the count for {month} (UTC)]({counter_url(month)})"
    return text[:start] + "\n" + link + "\n" + text[end:]


def update_activity_badge(text, now):
    if BADGE_START not in text and BADGE_END not in text:
        return text
    month = now.strftime("%Y-%m")
    query = urlencode({
        "ns": NAMESPACE, "action": ACTION, "key": month, "readOnly": "true",
        "label": "Active Users", "color": "ff686f", "style": "flat",
    })
    badge = (f'[![Active Users](https://counterapi.com/counter.svg?{query} '
             f'"Estimated monthly active installations ({month} UTC)")](#active-users)')
    start = text.index(BADGE_START) + len(BADGE_START)
    end = text.index(BADGE_END, start)
    return text[:start] + "\n" + badge + text[end:]


def fetch_count(month):
    request = Request(counter_url(month), headers={
        "User-Agent": "Hyprly-Stats/1", "Accept": "application/json",
    })
    with urlopen(request, timeout=8) as response:
        body = response.read(8193)
    if len(body) > 8192:
        raise ValueError("Counter response too large")
    data = json.loads(body)
    if (data.get("ns"), data.get("action"), data.get("key")) != (NAMESPACE, ACTION, month):
        raise ValueError("Counter identity mismatch")
    value = data.get("value")
    if type(value) is not int or value < 0:
        raise ValueError("Invalid counter value")
    return value


def refresh(history, now, fetch=fetch_count):
    # Refresh older months too: delayed app reports can revise their totals.
    # Build the complete snapshot before replacing any previously collected data.
    points = [{"month": m, "reports": fetch(m)}
              for m in months_since(history["startedMonth"], now)]
    return {"version": 1, "startedMonth": history["startedMonth"],
            "updatedAt": now.isoformat().replace("+00:00", "Z"), "points": points}


def render(history, now):
    points = history["points"]
    left, right, top, bottom = 62, 782, 24, 220
    parts = [
        '<svg xmlns="http://www.w3.org/2000/svg" width="800" height="292" viewBox="0 0 800 292" role="img" aria-labelledby="title desc">',
        '<title id="title">Hyprly activity history</title>',
        '<desc id="desc">Monthly connection reports, not cumulative users or users online now. The current month is incomplete.</desc>',
        '<rect width="800" height="292" fill="#0d1015"/>',
        '<g font-family="sans-serif" fill="#f4f4f5">',
        f'<path d="M{left} {top}V{bottom}H{right}" fill="none" stroke="#f4f4f5" stroke-width="2"/>',
    ]
    if points:
        peak = max(p["reports"] for p in points)
        step = 10 ** max(0, math.floor(math.log10(max(1, peak))))
        upper = max(2, math.ceil(peak * 1.15 / step) * step)

        def x(i):
            return (left + right) / 2 if len(points) == 1 else left + i * (right - left) / (len(points) - 1)

        def y(value):
            return bottom - value * (bottom - top) / upper

        for value in (0, upper // 2, upper):
            yy = y(value)
            parts.append(f'<path d="M{left - 5} {yy:.1f}H{left}" stroke="#f4f4f5" stroke-width="2"/>')
            parts.append(f'<text x="{left - 13}" y="{yy + 5:.1f}" text-anchor="end" font-size="15">{value}</text>')
        path = " ".join(f'{"M" if i == 0 else "L"}{x(i):.1f} {y(p["reports"]):.1f}' for i, p in enumerate(points))
        parts.append(f'<path d="{path}" fill="none" stroke="#ff686f" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>')
        # A lone point needs a marker; longer histories use only the clean line.
        if len(points) == 1:
            parts.append(f'<circle cx="{x(0):.1f}" cy="{y(points[0]["reports"]):.1f}" r="3" fill="#ff686f"/>')
        for i in sorted({0, (len(points) - 1) // 2, len(points) - 1}):
            point = points[i]
            label = datetime.strptime(point["month"], "%Y-%m").strftime("%b %Y")
            anchor = "middle" if len(points) == 1 else "start" if i == 0 else "end" if i == len(points) - 1 else "middle"
            parts.append(f'<text x="{x(i):.1f}" y="245" text-anchor="{anchor}" font-size="15">{escape(label)}</text>')
    else:
        parts.append('<text x="423" y="125" text-anchor="middle" fill="#a1a1aa" font-size="16">Awaiting the first successful weekly update</text>')
    parts.extend([
        '<text x="422" y="274" text-anchor="middle" font-size="14">Month</text>',
        '<text x="18" y="122" transform="rotate(-90 18 122)" text-anchor="middle" font-size="14">Active User Count</text>',
    ])
    stamp = history.get("updatedAt")
    updated = "" if not stamp else "Updated on " + datetime.fromisoformat(stamp.replace("Z", "+00:00")).strftime("%b %-d, %Y")
    parts.append(f'<text x="62" y="289" fill="#a1a1aa" font-size="10">{escape(updated)}</text>')
    parts.append("</g></svg>\n")
    return "\n".join(parts)


def main():
    now = datetime.now(timezone.utc)
    history = json.loads(HISTORY.read_text(encoding="utf-8"))
    try:
        fresh = refresh(history, now)
    except (OSError, ValueError, KeyError, TypeError) as error:
        # An unavailable provider must not erase history or invent zero counts.
        print(f"Activity refresh deferred; preserving previous data: {error}", file=sys.stderr)
    else:
        HISTORY.write_text(json.dumps(fresh, indent=2) + "\n", encoding="utf-8")
        history = fresh
    OUTPUT.write_text(render(history, now), encoding="utf-8")
    # Keep the public link current even when a provider outage defers the chart refresh.
    old_readme = README.read_text(encoding="utf-8")
    new_readme = update_activity_badge(update_counter_link(old_readme, now), now)
    if new_readme != old_readme:
        README.write_text(new_readme, encoding="utf-8")


if __name__ == "__main__":
    main()
