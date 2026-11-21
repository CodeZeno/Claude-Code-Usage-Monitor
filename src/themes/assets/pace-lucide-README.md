# Compact Fluent Pace icons

The `pace-lucide-*.png` files are transparent 56 by 56 pixel renders of Lucide's
`trending-down`, `gauge`, and `trending-up` icons. The theme displays them at
11.2 logical pixels. Each render uses a `0 0 24 24` viewBox, no fill, stroke
width 2, and round line caps and joins.

| State | SVG paths |
| --- | --- |
| Underpace | `M16 17h6v-6`, `m22 17-8.5-8.5-5 5L2 7` |
| In pace | `m12 14 4-4`, `M3.34 19a10 10 0 1 1 17.32 0` |
| Overpace | `M16 7h6v6`, `m22 7-8.5 8.5-5-5L2 17` |

| Provider | Dark | Light |
| --- | --- | --- |
| Claude | `#D97757` | `#D97757` |
| Codex | `#3B82F6` | `#2563EB` |
| Copilot | `#EC4899` | `#DB2777` |

Source: [Lucide icons](https://github.com/lucide-icons/lucide/tree/main/icons).
See [pace-lucide-LICENSE.txt](pace-lucide-LICENSE.txt) for the Lucide ISC license
and inherited Feather MIT license. Both accompany the installed theme assets.

To regenerate the guide's dark and light previews with the native renderer and
dummy usage data, run on Windows:

```powershell
$env:CCUM_PACE_PREVIEW_DIR = Join-Path $PWD 'docs/images'
cargo test --locked pace_renders_dark_and_light_at_multiple_scales
Remove-Item Env:\CCUM_PACE_PREVIEW_DIR
```
