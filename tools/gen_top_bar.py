"""Generate src/themes/top-bar.json (Windows only: text widths come from GDI).

Run from the repository root:  python tools/gen_top_bar.py
It rewrites src/themes/top-bar.json, reading the shipped languages from
src/localization/locales/*.toml. Needs Python 3.11+ and the Segoe UI Variable
font. Commit the regenerated JSON together with any change to this file.

Look: a flat web-app design language. Flat surfaces, a 1px
ring plus a 1px drop for depth, grey labels, loud numbers, and status colours
on tinted pills. Every colour is a light/dark token pair chosen by `system.dark`.
"""
import glob
import json
import math
import os
import tomllib
import ctypes
from ctypes import wintypes

gdi = ctypes.windll.gdi32
user = ctypes.windll.user32
gdi.CreateFontW.restype = wintypes.HFONT
gdi.CreateCompatibleDC.restype = wintypes.HDC
gdi.CreateCompatibleDC.argtypes = [wintypes.HDC]
gdi.SelectObject.argtypes = [wintypes.HDC, wintypes.HGDIOBJ]
gdi.SelectObject.restype = wintypes.HGDIOBJ
gdi.DeleteObject.argtypes = [wintypes.HGDIOBJ]
gdi.DeleteDC.argtypes = [wintypes.HDC]
gdi.GetTextExtentPoint32W.argtypes = [wintypes.HDC, wintypes.LPCWSTR, ctypes.c_int, ctypes.c_void_p]
class SIZE(ctypes.Structure):
    _fields_ = [("cx", ctypes.c_long), ("cy", ctypes.c_long)]
def measure(text, size, weight, family="Segoe UI Variable Text"):
    dc = gdi.CreateCompatibleDC(None)
    font = gdi.CreateFontW(-size, 0, 0, 0, weight, 0, 0, 0, 1, 7, 0, 4, 0, family)
    old = gdi.SelectObject(dc, font)
    s = SIZE()
    gdi.GetTextExtentPoint32W(dc, text, len(text), ctypes.byref(s))
    gdi.SelectObject(dc, old); gdi.DeleteObject(font); gdi.DeleteDC(dc)
    return s.cx


FONT = "Segoe UI Variable Text"
SM, PAD, R = 4, 16, 16           # shadow margin, card side padding, card corner radius
TOP, HEAD, CH, BOTTOM = 6, 20, 80, 8
# The status pill sits in the header row over the last CLI's cells, so the
# cells start a clear gap below it rather than touching its bottom edge.
STATUS_H, ROW_GAP = 18, 5
CELLS_Y = TOP + (HEAD + STATUS_H) // 2 + ROW_GAP
CELL, CGAP, BGAP = 112, 16, 40
UNIT = CELL + CGAP

# (light, dark) colour tokens.
SURFACE = ("#FFFFFF", "#19191C")
RING = ("#ECECEC", "#FFFFFF14")
DROP = ("#28282814", "#00000066")
LINE = ("#EBEBEB", "#2A2A2F")
TEXT = ("#171717", "#EDEDEF")
TEXT2 = ("#5C5C5C", "#B4B4BA")
TEXT3 = ("#6B6B6B", "#9B9BA2")
FAINT = ("#A3A3A3", "#6B6B73")
TB = ("#F0F0F0", "#242428")
IDLE = ("#C7C7C7", "#55555C")
GREEN, GREEN_TINT, GREEN_TEXT = ("#18C669", "#2FD27F"), ("#E4FCEF", "#2FD27F1F"), ("#0F7A4A", "#4ADE94")
AMBER, AMBER_TINT, AMBER_TEXT = ("#FFA100", "#FFB020"), ("#FFF4E0", "#FFB0201F"), ("#8A5600", "#FFC35C")
RED, RED_TINT, RED_TEXT = ("#EF4444", "#F26B6B"), ("#FEF0EF", "#F26B6B1F"), ("#B42318", "#FF8F8F")
WHITE = ("#FFFFFF", "#FFFFFF")


def tok(pair):
    light, dark = pair
    return light if light == dark else f'if(system.dark, "{dark}", "{light}")'


def quoted(pair):
    """A token as an expression operand (bare colours must be quoted)."""
    expression = tok(pair)
    return expression if expression.startswith("if(") else f'"{expression}"'


def by_level(pct, alarm, normal, amber, red, alarming):
    """Colour by usage level: 70%+ amber, 90%+ red, almost spent the alarm colour."""
    return (f"if({alarm}, {quoted(alarming)}, if({pct} >= 90, {quoted(red)}, "
            f"if({pct} >= 70, {quoted(amber)}, {quoted(normal)})))")


# GDI widths at 96 DPI (measure below): names are 12px semibold, status 12px regular.
NAME_W = {"claude": 42, "codex": 38, "antigravity": 69, "copilot": 45, "cursor": 40, "grok": 29, "opencode": 64}
PILL_DOT, PILL_TEXT, PILL_RIGHT = 8, 18, 6  # status pill: dot inset, text inset, right padding


def slack(width):
    """Room for hinting: at 125-200% GDI draws this text up to 4.1% wider than at 96 DPI."""
    return f"(({width}) * 1.045 + 1)"


# Status text and countdown suffixes are localized (src/localization/locales),
# so the widths above, measured for English, grow per language. EN_* are the
# English widths the layout is drawn for; each shipped language's measured
# width beyond them is added through `lang_extra`, rounded up to LANG_STEP px.
LANG_STEP = 4
EN_SUFFIX = {"day": 7, "hour": 7, "minute": 10}                  # "d" "h" "m"
EN_STATUS = {"waiting": 119, "live": 21, "stale": 25, "live_age": 100, "stale_age": 104}
STATUS_KEYS = {"waiting": "status_waiting", "live": "status_live", "stale": "status_stale",
               "live_age": "status_live_updated", "stale_age": "status_stale_updated"}


def load_locales():
    found = {}
    for path in glob.glob(os.path.join(os.path.dirname(__file__), "..", "src", "localization", "locales", "*.toml")):
        with open(path, "rb") as f:
            data = tomllib.load(f)
        found[data["code"]] = data["strings"]
    return found


LOCALES = load_locales()
assert len(LOCALES) == 14, sorted(LOCALES)


def text_w(value):
    return measure(value, 12, 400)


def suffix_w(strings):
    return {unit: text_w(strings[f"{unit}_suffix"]) for unit in EN_SUFFIX}


def static_w(strings, state):
    return text_w(strings[STATUS_KEYS[state]].replace("{time}", ""))


def countdown_extra(strings):
    """Widest the countdown grows beyond English: the ranges "<1m", "Nm", "Nh Nm" and "Nd Nh"."""
    w = {unit: width - EN_SUFFIX[unit] for unit, width in suffix_w(strings).items()}
    return max(w["minute"], w["hour"] + w["minute"], w["day"] + w["hour"])


def status_extra(strings, state):
    """Pixels the status text of this state grows beyond English in this language."""
    grow = static_w(strings, state) - static_w(LOCALES["en"], state)
    grow += countdown_extra(strings) if state.endswith("_age") else 0
    return max(0, math.ceil(grow / LANG_STEP) * LANG_STEP)


def lang_pick(by_language):
    """An expression choosing a number by `i18n.locale`; the commonest value is the fallback."""
    values = sorted(set(by_language.values()))
    groups = {v: sorted(c for c, x in by_language.items() if x == v) for v in values}
    fallback = max(values, key=lambda v: (len(groups[v]), -v))
    if len(values) == 1:
        return str(fallback)
    expression = str(fallback)
    for v in sorted((v for v in values if v != fallback), reverse=True):
        test = " || ".join(f'i18n.locale == "{c}"' for c in groups[v])
        expression = f"if({test}, {v}, {expression})"
    return expression


LANG_EXTRA = {state: lang_pick({c: status_extra(strings, state) for c, strings in LOCALES.items()})
              for state in EN_STATUS}
# The last block holds the widest status of any state.
LANG_RESERVE = lang_pick({c: math.ceil(max(status_extra(strings, state) for state in EN_STATUS) * 1.045)
                          for c, strings in LOCALES.items()})
# Cell countdowns grow by the language's widest suffix combination.
LANG_COUNTDOWN = lang_pick({c: max(0, math.ceil(countdown_extra(strings) / LANG_STEP) * LANG_STEP)
                            for c, strings in LOCALES.items()})

# Whole pixels, so a bar at its natural width never rounds into shrinking.
STATUS_RESERVE = PILL_TEXT + math.ceil(148 * 1.045 + 1) + PILL_RIGHT  # "Stale · updated 23h 59m ago"
FABLE_W = 31                     # "Fable "
VALUE_PAD = 7                    # level pill padding either side of the percentage
LABEL_GAP = 6                    # least room between a label and its value
RESETS_W = 45                    # "Resets in" at 11px
SCOPE_W = 36                     # "weekly" at 12px
# The alarm's "Snooze 1h" button, left of the status pill: a bell-off glyph
# (Segoe MDL2 Assets, on Windows 10 and 11) and 12px semibold text (GDI 62).
SNOOZE_ICON, SNOOZE_ICON_W = "", 12
SNOOZE_PAD, SNOOZE_SPACE, SNOOZE_GAP = 7, 4, 6
SNOOZE_W = SNOOZE_PAD + SNOOZE_ICON_W + SNOOZE_SPACE + math.ceil(62 * 1.045 + 1) + SNOOZE_PAD

WEEKLY = '{if(%s.weekly.label == "30d", "Monthly", if(%s.weekly.label == "API", "API", "Weekly"))}'


def windows(p):
    """(id, binding base, label template, render condition) per limit, left to right."""
    if p == "claude":
        return [("five_hour", "claude.five_hour", "5-hour", "1"),
                ("weekly", "claude.weekly", "Weekly", "1"),
                ("fable", "claude.model.fable", "Fable", "claude.model.fable.available")]
    if p == "codex":
        return [("five_hour", "codex.five_hour", "5-hour", "1"),
                ("weekly", "codex.weekly", WEEKLY % (p, p), "1")]
    session = "Auto" if p == "cursor" else "5-hour"
    result = [("five_hour", f"{p}.five_hour", session, f"{p}.five_hour.available"),
              ("weekly", f"{p}.weekly", WEEKLY % (p, p), "1")]
    if p == "opencode":
        result.append(("monthly", "opencode.monthly", "Monthly", "opencode.monthly.available"))
    return result


# Identity dots, toned to sit with the status colours in both modes.
PROVIDERS = [("claude", "Claude", "#D97757"), ("codex", "Codex", "#5B5FD6"),
             ("antigravity", "Antigravity", "#14A3A0"), ("copilot", "Copilot", "#8B5CF6"),
             ("cursor", "Cursor", "#E0569B"), ("grok", "Grok", "#8A8A93"),
             ("opencode", "OpenCode", "#6AA81A")]
KEYS = [p for p, _, _ in PROVIDERS]


def en(p):
    return f"providers.{p}.enabled"


def av(p):
    return f"{p}.available"


def cells(p):
    conds = [c for _, _, _, c in windows(p)]
    fixed = sum(1 for c in conds if c == "1")
    return " + ".join([str(fixed)] + [c for c in conds if c != "1"])


def earlier(p):
    """Some CLI to the left is enabled, so this block follows a divider."""
    before = KEYS[:KEYS.index(p)]
    return f"({' || '.join(en(q) for q in before)})" if before else "0"


def last(p):
    later = KEYS[KEYS.index(p) + 1:]
    return f"!({' || '.join(en(q) for q in later)})" if later else "1"


def na_width(p):
    return 17 + NAME_W[p] + 6


def reserve(p):
    """The last block also holds the status pill, and the Snooze button beside it during an alarm."""
    return f"({17 + NAME_W[p] + 16 + STATUS_RESERVE} + {LANG_RESERVE} + alarm.active * {SNOOZE_W + SNOOZE_GAP})"


def span(p):
    return f"(({cells(p)}) * {UNIT} - {CGAP})"


DIVIDERS = "(providers.count - 1)"
NATURAL = f"{2 * (SM + PAD)} + {DIVIDERS} * {BGAP + 1} + " + " + ".join(
    f"{en(p)} * max({av(p)} * {span(p)} + (1 - {av(p)}) * {na_width(p)}, {last(p)} * {reserve(p)})"
    for p in KEYS)
# Cells and spacing shrink by one factor k when every enabled CLI no longer
# fits. Width is B + k*V + max(0, R_last - content_last(k)); solve both
# branches of the max and take the smaller k (both are increasing in k).
BASE = f"{2 * (SM + PAD)} + {DIVIDERS} + " + " + ".join(
    f"{en(p)} * (1 - {av(p)}) * {na_width(p)}" for p in KEYS)
VARIABLE = f"{DIVIDERS} * {BGAP} + " + " + ".join(f"{en(p)} * {av(p)} * {span(p)}" for p in KEYS)
RESERVE = " + ".join(f"{en(p)} * {last(p)} * ({reserve(p)} - (1 - {av(p)}) * {na_width(p)})" for p in KEYS)
LAST_VARIABLE = " + ".join(f"{en(p)} * {last(p)} * {av(p)} * {span(p)}" for p in KEYS)
SHRINK = (f"clamp(min((canvas.width - ({BASE})) / max(1, {VARIABLE}), "
          f"if(({VARIABLE}) - ({LAST_VARIABLE}) > 0, (canvas.width - ({BASE}) - ({RESERVE})) / "
          f"max(1, ({VARIABLE}) - ({LAST_VARIABLE})), 1)), 0.2, 1)")
# The provider row's gap is half the scaled block spacing (a divider sits in
# the middle), so every block reads the shrink factor as parent.gap / 20.
K = f"parent.gap / {BGAP // 2}"


STALE = "data.has_error || " + " || ".join(f"({en(p)} && {p}.stale)" for p in KEYS)
SHOWN = [f"{en(p)} * {base}.available * {base}.percentage" for p in KEYS for _, base, _, _ in windows(p)]
WORST = SHOWN[-1]
for term in reversed(SHOWN[:-1]):
    WORST = f"max({term}, {WORST})"
HANDLE = (f"if(data.loading || {STALE}, {quoted(IDLE)}, if({WORST} >= 90, {quoted(RED)}, "
          f"if({WORST} >= 70, {quoted(AMBER)}, {quoted(TEXT3)})))")


def digits(v):
    """Pixel width of a one- or two-digit number at 12px ("1" is narrower)."""
    return f"(({v}) >= 10) * if(floor(({v}) / 10) == 1, 5, 6) + if(({v}) % 10 == 1, 5, 6)"


def percent_width(v):
    """Pixel width of "<v>%" at 13px semibold, up to three digits ("1" is narrower)."""
    return (f"(({v}) >= 100) * if(floor(({v}) / 100) == 1, 6, 8) + "
            f"(({v}) >= 10) * if(floor(({v}) / 10) % 10 == 1, 6, 8) + if(({v}) % 10 == 1, 6, 8) + 12")


def countdown_width(s, d=EN_SUFFIX["day"], h=EN_SUFFIX["hour"], m=EN_SUFFIX["minute"]):
    """Pixel width of "<s>:countdown" at 12px regular: "<1" 13, a space 3, and the suffixes d, h, m
    (English 7, 7, 10)."""
    return (f"if({s} < 60, {13 + m}, if({s} < 3600, {digits(f'floor({s} / 60)')} + {m}, "
            f"if({s} < 86400, {digits(f'floor({s} / 3600)')} + {digits(f'floor({s} % 3600 / 60)')} + {h + m + 3}, "
            f"{digits(f'floor({s} / 86400)')} + {digits(f'floor({s} % 86400 / 3600)')} + {d + h + 3})))")


# The status pill is drawn for English and grows per language (LANG_EXTRA).
COUNTDOWN_W = countdown_width("data.updated.seconds")
# Cells fit "Resets in" beside the countdown in the user's language.
CELL_COUNTDOWN = lambda s: f"{countdown_width(s)} + {LANG_COUNTDOWN}"


def obj(id, name, parent=None, render="1", x="0", y="0", w="100", h="20", **extra):
    """A layer with every engine default omitted, to keep the embedded theme small."""
    o = {"id": id, "name": name}
    if render != "1":
        o["render"] = render
    visibility = extra.pop("visibility", "100")
    if visibility != "100":
        o["visibility"] = visibility
    if parent:
        o["parent"] = parent
    if "anchor" in extra:
        o["anchor"] = extra.pop("anchor")
    for key, value in [("x", x), ("y", y)]:
        if str(value) != "0":
            o[key] = str(value)
    o["width"], o["height"] = str(w), str(h)
    for key, default in [("background", None), ("border", None), ("mouse_events", None), ("radius", "0"),
                         ("layout", "freeform"), ("gap", "0"), ("content", None)]:
        value = extra.pop(key, default)
        if value is not None and str(value) != str(default):
            o["corner_radius" if key == "radius" else key] = (
                value if key in ("background", "border", "mouse_events", "content") else str(value))
    assert not extra, extra
    return o


def paint(c, opacity="1"):
    """A colour literal, a (light, dark) token pair, or a colour expression."""
    return {"color": tok(c) if isinstance(c, tuple) else c, "opacity": str(opacity)}


def colour(c, opacity="1"):
    return {"type": "colour", "colour": paint(c, opacity)}


def text(template, size, weight, c, align="left", opacity="1", family=None):
    o = {"type": "text", "template": template, "font_size": str(size), "contrast": "1.2"}
    if family:
        o["font_family"] = family
    if weight != "regular":
        o["weight"] = weight
    if align != "left":
        o["align"] = align
    o["color"] = paint(c, opacity)
    return o


layers = []
# Depth is a 1px drop under a 1px ring (`0 1px 2px`, `0 0 0 1px`), no soft shadow.
for i, opacity in [(2, "0.5"), (1, "1")]:
    layers.append(obj(f"drop-{i}", f"Drop {i}", x=SM - i + 1, y=-R, w=f"canvas.width - {2 * (SM - i + 1)}",
                      h=CH + R + i, background=colour(DROP, opacity), radius=R + i - 1))
layers.append(obj("card", "Card", x=SM, y=-R, w=f"canvas.width - {2 * SM}", h=CH + R,
                  background=colour(SURFACE), radius=R, border={"color": paint(RING), "width": "1"}))
layers.append(obj("providers", "Enabled CLIs", x=SM + PAD, w=f"canvas.width - {2 * (SM + PAD)}",
                  h=CH, layout="row", gap=f"{BGAP // 2} * {SHRINK}"))

for p, name, accent in PROVIDERS:
    if earlier(p) != "0":
        layers.append(obj(f"{p}-divider", f"{name} divider", "providers", f"{en(p)} && {earlier(p)}",
                          y=TOP + 3, w=1, h=CH - TOP - BOTTOM - 3, background=colour(LINE)))
    box = f"{p}-block"
    layers.append(obj(box, f"{name} block", "providers", en(p), gap="parent.gap",
                      w=f"max({av(p)} * {K} * {span(p)} + (1 - {av(p)}) * {na_width(p)}, "
                        f"{last(p)} * {reserve(p)})", h=CH))
    # A 7px dot in a 3px soft ring, like a project tab.
    layers.append(obj(f"{p}-halo", f"{name} halo", box, x=0, y=TOP + HEAD / 2 - 6.5, w=13, h=13,
                      background=colour(accent, 0.18), radius=6.5))
    layers.append(obj(f"{p}-dot", f"{name} dot", box, x=3, y=TOP + HEAD / 2 - 3.5, w=7, h=7,
                      background=colour(accent), radius=3.5))
    layers.append(obj(f"{p}-name", f"{name} name", box, x=17, y=TOP, w="parent.width - 17", h=HEAD,
                      content=text(name, 12, "semibold", TEXT)))
    layers.append(obj(f"{p}-na", f"{name} unavailable", box, f"!{av(p)}", y=CELLS_Y, w="parent.width",
                      h=18, content=text("n/a", 13, "semibold", FAINT)))
    row = f"{p}-cells"
    n = cells(p)
    layers.append(obj(row, f"{name} limits", box, av(p), y=CELLS_Y, w=f"{K} * {span(p)}",
                      h=CH - CELLS_Y - BOTTOM, layout="row",
                      gap=f"parent.width * {CGAP} / max(1, ({n}) * {UNIT} - {CGAP})"))
    for key, base, label, cond in windows(p):
        cell = f"{p}-{key}"
        a, pct, shown, alarm = f"{base}.available", f"{base}.percentage", f"{base}.display", f"{base}.alarm"
        layers.append(obj(cell, f"{name} {key}", row, cond,
                          w=f"(parent.width - (({n}) - 1) * parent.gap) / max(1, {n})", h=42))
        # 70%+ and 90%+ sit on a tinted pill; an almost spent limit on solid red.
        flagged = f"{a} && ({pct} >= 70 || {alarm})"
        shown_w = percent_width(f"max(0, round({shown}))")
        pill_w = f"{shown_w} + {2 * VALUE_PAD}"
        # Labels end before the value or its pill ("n/a" is 22), so a squeezed
        # cell ellipsizes the label instead of running it into the number.
        label_room = f"parent.width - if({a}, {shown_w} + if({pct} >= 70 || {alarm}, {2 * VALUE_PAD}, 0), 22) - {LABEL_GAP}"
        label_colour = f"if({alarm}, {quoted(RED_TEXT)}, if({a}, {quoted(TEXT3)}, {quoted(FAINT)}))"
        # Under 10px nothing readable survives, only a sliver of an ellipsis.
        layers.append(obj(f"{cell}-label", f"{name} {key} label", cell, w=label_room, h=18,
                          visibility=f"if({label_room} >= 10, 100, 0)",
                          content=text(label, 12, "regular", label_colour)))
        if key == "fable":
            # "weekly" shows only where all of it fits before the value.
            layers.append(obj(f"{cell}-label-scope", f"{name} {key} scope", cell, x=FABLE_W,
                              w=f"{label_room} - {FABLE_W}", h=18,
                              visibility=f"if({label_room} >= {FABLE_W} + {slack(SCOPE_W)}, 100, 0)",
                              content=text("weekly", 12, "regular", FAINT)))
        layers.append(obj(f"{cell}-pill", f"{name} {key} level pill", cell, flagged,
                          x=f"parent.width - ({pill_w})", w=pill_w, h=18, radius=8,
                          background=colour(by_level(pct, alarm, TB, AMBER_TINT, RED_TINT, RED))))
        layers.append(obj(f"{cell}-value", f"{name} {key} value", cell, a,
                          w=f"parent.width - if({pct} >= 70 || {alarm}, {VALUE_PAD}, 0)", h=18,
                          content=text(f"{{{shown}:0}}%", 13, "semibold",
                                       by_level(pct, alarm, TEXT, AMBER_TEXT, RED_TEXT, WHITE), "right")))
        layers.append(obj(f"{cell}-na", f"{name} {key} unavailable", cell, f"!{a}", w="parent.width", h=18,
                          content=text("n/a", 13, "semibold", FAINT, "right")))
        layers.append(obj(f"{cell}-track", f"{name} {key} track", cell, y=21, w="parent.width", h=4,
                          background=colour(LINE, f"if({a}, 1, 0.5)"), radius=2))
        layers.append(obj(f"{cell}-fill", f"{name} {key} fill", cell, f"{a} && {shown} > 0", y=21,
                          w=f"max(4, parent.width * min({shown}, 100) / 100)", h=4, radius=2,
                          background=colour(by_level(pct, alarm, TEXT, AMBER, RED, RED))))
        resets = f"{a} && {base}.reset.unix > 0"
        # "Resets in" gives way to the countdown when a squeezed cell cannot hold both.
        fits = f"{slack(RESETS_W)} + {LABEL_GAP} + {slack(CELL_COUNTDOWN(f'{base}.reset.seconds'))}"
        layers.append(obj(f"{cell}-resets", f"{name} {key} resets in", cell, resets, y=28,
                          w="parent.width", h=14, visibility=f"if(parent.width >= {fits}, 100, 0)",
                          content=text("Resets in", 11, "regular", TEXT3)))
        layers.append(obj(f"{cell}-countdown", f"{name} {key} countdown", cell, resets, y=28,
                          w="parent.width", h=14,
                          content=text(f"{{{base}.reset.seconds:countdown}}", 12, "regular", TEXT, "right")))

# Live / stale status as a small status pill, right-aligned in the header row
# above the last CLI. The block reserves its width beside the name, and the
# cells start ROW_GAP below it, so it never meets a name, label or level pill.
right = SM + PAD
status_y = TOP + (HEAD - STATUS_H) // 2
SHOWN_STATUS = {}
live = f"!data.loading && !({STALE})"
stale = f"!data.loading && ({STALE})"
for id, name, render, state, template, width, tint, ink, dot in [
    ("status-waiting", "Waiting", "data.loading", "waiting", "{i18n.status_waiting}",
     str(EN_STATUS["waiting"]), TB, TEXT2, IDLE),
    ("status-live", "Live", f"{live} && data.updated.available", "live_age", "{i18n.status_live_updated}",
     f"{EN_STATUS['live_age']} + {COUNTDOWN_W}", GREEN_TINT, GREEN_TEXT, GREEN),
    ("status-live-untimed", "Live (no time)", f"{live} && !data.updated.available", "live",
     "{i18n.status_live}", str(EN_STATUS["live"]), GREEN_TINT, GREEN_TEXT, GREEN),
    ("status-stale", "Stale", f"{stale} && data.updated.available", "stale_age",
     "{i18n.status_stale_updated}", f"{EN_STATUS['stale_age']} + {COUNTDOWN_W}", AMBER_TINT, AMBER_TEXT, AMBER),
    ("status-stale-untimed", "Stale (no time)", f"{stale} && !data.updated.available", "stale",
     "{i18n.status_stale}", str(EN_STATUS["stale"]), AMBER_TINT, AMBER_TEXT, AMBER),
]:
    # Wider where this language's words and suffixes are (LANG_EXTRA).
    room = slack(f"{width} + {LANG_EXTRA[state]}")
    SHOWN_STATUS[state] = room
    # The dot and the text are the pill's children, so the language-sized width
    # is written once. The text is centred in room measured with slack, so text
    # drawn wider at fractional scales than at 96 DPI never runs into the dot
    # or the pill's end.
    layers.append(obj(f"{id}-pill", f"{name} pill", None, render,
                      x=f"canvas.width - {right} - {room} - {PILL_TEXT + PILL_RIGHT}", y=status_y,
                      w=f"{room} + {PILL_TEXT + PILL_RIGHT}", h=STATUS_H, radius=8, background=colour(tint)))
    layers.append(obj(f"{id}-dot", f"{name} dot", f"{id}-pill", x=PILL_DOT, y=TOP + HEAD / 2 - 3 - status_y,
                      w=6, h=6, background=colour(dot), radius=3))
    layers.append(obj(id, f"{name} status", f"{id}-pill", x=PILL_TEXT, w=f"parent.width - {PILL_TEXT + PILL_RIGHT}",
                      h=STATUS_H, content=text(template, 12, "regular", ink, "center")))

# While the alarm holds the bar down, a "Snooze 1h" button left of whichever
# status pill shows does what the menu's "Snooze alarm (1 hour)" does. The
# last block reserves its width, so it never meets that CLI's name.
shown_status = (f"if(data.loading, {SHOWN_STATUS['waiting']}, if(({STALE}), if(data.updated.available, "
                f"{SHOWN_STATUS['stale_age']}, {SHOWN_STATUS['stale']}), if(data.updated.available, "
                f"{SHOWN_STATUS['live_age']}, {SHOWN_STATUS['live']})))")
snooze_x = f"canvas.width - {right} - ({shown_status}) - {PILL_TEXT + PILL_RIGHT} - {SNOOZE_GAP} - {SNOOZE_W}"
layers.append(obj("snooze-button", "Snooze button", None, "alarm.active", x=snooze_x, y=status_y,
                  w=SNOOZE_W, h=STATUS_H, radius=8, background=colour(RED_TINT),
                  border={"color": paint(RED, 0.45), "width": "1"},
                  mouse_events={"click": "snooze_alarm()"}))
layers.append(obj("snooze-button-icon", "Snooze button icon", None, "alarm.active",
                  x=f"{snooze_x} + {SNOOZE_PAD}", y=status_y, w=SNOOZE_ICON_W, h=STATUS_H,
                  content=text(SNOOZE_ICON, 11, "regular", RED_TEXT, "center", family="Segoe MDL2 Assets")))
layers.append(obj("snooze-button-text", "Snooze button text", None, "alarm.active",
                  x=f"{snooze_x} + {SNOOZE_PAD + SNOOZE_ICON_W + SNOOZE_SPACE}", y=status_y,
                  w=f"{SNOOZE_W - 2 * SNOOZE_PAD - SNOOZE_ICON_W - SNOOZE_SPACE}", h=STATUS_H,
                  content=text("Snooze 1h", 12, "semibold", RED_TEXT, "center")))

theme = {
    "schema_version": 1,
    "id": "top-bar",
    "name": "Top Bar",
    "surfaces": [{
        "id": "main",
        "name": "Top bar",
        "render": "1",
        "visibility": "100",
        "placement": {
            "reference": {"region": "monitor", "display": 0},
            "nest": "floating",
            "horizontal": "center",
            "vertical": "top",
            "surface_horizontal": "center",
            "surface_vertical": "top",
            "offset_x": 0,
            "offset_y": 0,
            "auto_hide": True,
            "handle_color": HANDLE,
            "handle_background": tok(SURFACE),
            # An almost spent limit keeps the bar down until it resets or is snoozed.
            "force_reveal": "alarm.active",
        },
        "width": f"min({NATURAL}, max(160, host.width - 16))",
        "height": str(CH + SM),
        # Transparent rather than none: the bar draws its own card, so
        # floating hosts must not add their automatic card behind it.
        "background": colour("#00000000"),
        "border": None,
        "mouse_events": {"double_click": "show_dashboard()",
                         "right_click": "show_context_menu(\"classic-v1\")"},
        "corner_radius": "0",
        "layout": "freeform",
        "align": "start",
        "gap": "0",
        "content": {"type": "none"},
        "children": layers,
    }],
}

out = os.path.join(os.path.dirname(__file__), "..", "src", "themes", "top-bar.json")
with open(out, "w", encoding="utf-8", newline="\n") as f:
    json.dump(theme, f, indent=2, ensure_ascii=False)
    f.write("\n")
print(len(layers), "layers,", os.path.getsize(out), "bytes")
