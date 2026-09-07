## Screen capture

Capture the screen yourself: PowerShell + Win32 P/Invoke — `user32.dll PrintWindow` (by window class name) or `Graphics.CopyFromScreen` (screen region). Examples: `.cr-tmp/HANDOFF.md`.

## Widget default position: x=0, no exceptions

The widget's default/leftmost taskbar position is **screen x=0** — the taskbar's own
physical left edge. Literally zero. Not "clear of the Start button," not "left of the
search box," not "the left edge of the icon cluster." x=0.

This has been re-litigated with more than one agent session already. Every time, an
agent reasoned its way into a *smaller* definition of "leftmost" — usually "as far
left as possible without overlapping existing taskbar chrome/icons" — and presented
that as if it satisfied the requirement. It does not. The owner has explicitly and
repeatedly rejected that substitution. Do not re-derive it, do not re-ask what
"leftmost" means, do not propose it as an "honest tradeoff." If x=0 seems to conflict
with something else (Start button overlap, icon cluster, etc.), that is a bug in the
positioning code to fix (see `native_interop::taskbar_placement_band_left`, which is
hardcoded to return 0 for exactly this reason) — it is never a reason to move the
widget off x=0.

If a genuinely new constraint makes x=0 impossible on some configuration, stop and
ask the owner directly with the specific conflict — do not silently pick a different
default.
