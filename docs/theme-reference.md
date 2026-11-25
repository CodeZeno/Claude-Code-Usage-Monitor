# Theme reference

<!-- Generated from src/theme_engine/theme_language.rs by src/theme_render/reference.rs. Do not edit; run `CCUM_UPDATE_SNAPSHOTS=1 cargo test --locked theme_reference` instead. -->

Theme files are JSON (see the [built-in themes](../src/themes/)). Most fields are **expressions**, not fixed numbers: `"width": "canvas.width - 8"` sizes a layer from its surface, and `"render": "claude.session.percentage > 80"` shows it only near the limit. Text layers use **templates** with `{expression}` tokens, and mouse events run **actions**. The [Fields](#fields) section says what each field takes.

Results below are computed with `tests/fixtures/theme-usage.json` on Stacked Rows' `main` surface, in dark mode, at 2027-01-15 08:00 UTC: the same inputs `--render-theme` uses. Check a theme with `claude-code-usage-monitor --render-theme <theme.json> --out <folder>`; it prints unknown fields and failing expressions.

## Expressions

Expressions combine values, numbers and text with operators and functions. A condition is true when it is not 0.

| Literal | Meaning | Example | Result |
| --- | --- | --- | --- |
| number | Decimal number | `0.5` | `0.5` |
| text | Text in double or single quotes; `\"` escapes a quote | `"#E5484D"` | `"#E5484D"` |
| true, false | 1 and 0; any non-zero value counts as true | `true` | `1` |
| pi, e | Constants | `pi` | `3.1416` |
| value name | A value from the lists below; names ignore case | `claude.session.percentage` | `42` |

### Operators

| Operator | Meaning | Example | Result |
| --- | --- | --- | --- |
| `&&` | And | `claude.available && codex.available` | `1` |
| `\|\|` | Or | `claude.available \|\| codex.available` | `1` |
| `!` | Not | `!data.has_error` | `1` |
| `==` | Equal | `providers.count == 2` | `1` |
| `!=` | Not equal | `claude.session.label != "5h"` | `0` |
| `>` | Greater than | `claude.session.percentage > 80` | `0` |
| `<` | Less than | `claude.session.reset.hours < 1` | `0` |
| `>=` | Greater or equal | `claude.weekly.percentage >= 50` | `1` |
| `<=` | Less or equal | `host.height <= 40` | `0` |
| `+` | Add | `canvas.width + 8` | `113` |
| `-` | Subtract | `canvas.width - 8` | `97` |
| `*` | Multiply | `canvas.width * claude.session.percentage / 100` | `44.1` |
| `/` | Divide | `canvas.height / 2` | `24` |
| `%` | Remainder | `time.local.second % 2` | `0` |
| `()` | Grouping | `(canvas.width - 8) / 2` | `48.5` |

### Functions

`get(owner, field)` reads `owner.field`; `get(self, width)` reads the current layer.

| Function | Meaning | Example | Result |
| --- | --- | --- | --- |
| `get(this, property)` | Value expression | `get(claude.session, percentage)` | `42` |
| `min(a, b)` | Smaller value | `min(claude.session.percentage, codex.session.percentage)` | `12` |
| `max(a, b)` | Larger value | `max(claude.session.percentage, claude.weekly.percentage)` | `63` |
| `clamp(value, min, max)` | Constrain a value | `clamp(claude.weekly.percentage * 2, 0, 100)` | `100` |
| `round(value)` | Nearest integer | `round(claude.session.reset.hours)` | `2` |
| `floor(value)` | Round down | `floor(claude.session.reset.hours)` | `2` |
| `ceil(value)` | Round up | `ceil(claude.session.reset.hours)` | `3` |
| `abs(value)` | Absolute value | `abs(claude.session.percentage - codex.session.percentage)` | `30` |
| `sqrt(value)` | Square root | `sqrt(claude.session.percentage)` | `6.4807` |
| `pow(base, power)` | Exponent | `pow(claude.session.percentage / 100, 2)` | `0.1764` |
| `if(condition, yes, no)` | Conditional value | `if(claude.session.percentage > 80, "#E5484D", "#46A758")` | `"#46A758"` |
| `lerp(start, end, amount)` | Linear interpolation | `lerp(4, canvas.width - 4, claude.session.percentage / 100)` | `44.74` |

## Templates

A text layer's `template` is literal text with `{expression}` or `{expression:format}` tokens, such as `"{claude.session.percentage:0}% used"`. `{{` writes a literal `{`. A token that cannot be evaluated shows `--`.

## Formats

| Format | Meaning | Example | Result |
| --- | --- | --- | --- |
| `0.##` | Default: two fixed decimals. With a `0`, each `0` or `#` after the point is a fixed decimal | `{canvas.width / 2}` | `52.50` |
| `#.##` | Without a `0`, decimals are optional: up to one per `#`, trailing zeros dropped | `{canvas.width / 2:#.##}` | `52.5` |
| `0` | Whole number | `{claude.session.reset.hours:0}` | `2` |
| `0.0` | Fixed decimals: one `0` per decimal place | `{canvas.width / 2:0.0}` | `52.5` |
| `#,##0` | Any format containing `,` groups thousands | `{host.width * 1000:#,##0}` | `1,920,000` |
| `percent` | Whole number followed by `%` | `{claude.session.display:percent}` | `42%` |
| `duration` | Seconds as the two largest units, such as `3d 4h` | `{claude.weekly.reset.seconds:duration}` | `3d 4h` |
| `duration_short` | Seconds as the largest unit | `{claude.weekly.reset.seconds:duration_short}` | `3d` |
| `usage_line` | A window's shown percentage and time to reset. Use the window name, or `.display` after it to follow the Usage direction setting | `{claude.session:usage_line}` | `42% · 2.5h` |
| `usage_badge` | A window's shown percentage only | `{claude.session:usage_badge}` | `42%` |
| `utc_` | Prefix for any timestamp format to use UTC instead of local time | `{claude.weekly.reset.unix:utc_iso_datetime}` | `2027-01-18T12:00:00Z` |
| `weekday_2` | Weekday, two letters | `{claude.weekly.reset.unix:weekday_2}` | `Mo` |
| `weekday_short` | Weekday, short | `{claude.weekly.reset.unix:weekday_short}` | `Mon` |
| `weekday_long` | Weekday, full | `{claude.weekly.reset.unix:weekday_long}` | `Monday` |
| `day` | Day of the month | `{claude.weekly.reset.unix:day}` | `18` |
| `day_2` | Day of the month, two digits | `{claude.weekly.reset.unix:day_2}` | `18` |
| `month` | Month number | `{claude.weekly.reset.unix:month}` | `1` |
| `month_2` | Month number, two digits | `{claude.weekly.reset.unix:month_2}` | `01` |
| `month_short` | Month name, short | `{claude.weekly.reset.unix:month_short}` | `Jan` |
| `month_long` | Month name, full | `{claude.weekly.reset.unix:month_long}` | `January` |
| `year_2` | Year, two digits | `{claude.weekly.reset.unix:year_2}` | `27` |
| `year` | Year | `{claude.weekly.reset.unix:year}` | `2027` |
| `date_short`, `date` | Short date in the app language | `{claude.weekly.reset.unix:date_short}` | `1/18/2027` |
| `date_long` | Long date in the app language | `{claude.weekly.reset.unix:date_long}` | `Monday, January 18, 2027` |
| `time_short`, `time` | Time in the app language | `{claude.weekly.reset.unix:time_short}` | `12:00 PM` |
| `time_seconds` | Time with seconds in the app language | `{claude.weekly.reset.unix:time_seconds}` | `12:00:00 PM` |
| `time_24` | 24-hour time | `{claude.weekly.reset.unix:time_24}` | `12:00` |
| `time_24_seconds` | 24-hour time with seconds | `{claude.weekly.reset.unix:time_24_seconds}` | `12:00:00` |
| `time_12` | 12-hour time | `{claude.weekly.reset.unix:time_12}` | `12:00 PM` |
| `time_12_seconds` | 12-hour time with seconds | `{claude.weekly.reset.unix:time_12_seconds}` | `12:00:00 PM` |
| `am_pm` | AM or PM | `{claude.weekly.reset.unix:am_pm}` | `PM` |
| `datetime_short`, `datetime` | Short date and time | `{claude.weekly.reset.unix:datetime_short}` | `1/18/2027 12:00 PM` |
| `datetime_long` | Long date and time with seconds | `{claude.weekly.reset.unix:datetime_long}` | `Monday, January 18, 2027 12:00:00 PM` |
| `iso_date` | ISO 8601 date | `{claude.weekly.reset.unix:iso_date}` | `2027-01-18` |
| `iso_time` | ISO 8601 time | `{claude.weekly.reset.unix:iso_time}` | `12:00:00` |
| `iso_datetime` | ISO 8601 date and time | `{claude.weekly.reset.unix:iso_datetime}` | `2027-01-18T12:00:00` |

## Mouse actions

`mouse_events` fields hold scripts of one or more actions separated by `;` or new lines. A layer is `self` or its quoted id; layer properties are `render`, `visibility`, `x`, `y`, `width`, `height`, `rotation`. `set`, `increase` and `decrease` take an expression.

| Action | Meaning | Example |
| --- | --- | --- |
| `show_dashboard()` | Show dashboard | `show_dashboard()` |
| `toggle_dashboard()` | Toggle dashboard | `toggle_dashboard()` |
| `open_url("https://…")` | Open URL | `open_url("https://claude.ai/settings/usage")` |
| `show_context_menu("menu")` | Show context menu | `show_context_menu()` |
| `set(layer, property, value)` | Set property | `set("claude-popup", render, true)` |
| `toggle(layer, render)` | Toggle Render | `toggle("details", render)` |
| `reset(layer, property)` | Reset property | `reset(self, height)` |
| `increase(layer, property, amount)` | Increase value | `increase(self.height, 10)` |
| `decrease(layer, property, amount)` | Decrease value | `decrease(self, width, 5)` |

## Fields

`placement` and `tooltip` apply to surfaces; `parent`, `anchor`, `x` and `y` to layers. Other object fields apply to both. `background(gradient).angle` is the `angle` key of a `background` whose `type` is `gradient`. Fields not listed here are ignored, so `--render-theme` reports them.

### Theme

| Field | Takes | Meaning |
| --- | --- | --- |
| `schema_version` | integer | Theme format version; currently 1 |
| `id` | text | Unique theme id, lowercase with dashes |
| `name` | text | Name shown in Theme Studio |
| `surfaces` | objects | Root objects. Each is a native window: `main` is the widget; others are flyouts or `<provider>-tray-icon` tray artwork |

### Surfaces and layers

| Field | Takes | Meaning |
| --- | --- | --- |
| `id` | text | Unique id. Mouse actions and `parent` refer to objects by id |
| `name` | text | Name shown in the Layers panel |
| `render` | condition | Draw the object and its children while true. Default `true` |
| `visibility` | expression | Opacity percentage for the object and its children, 0 to 100. Default `100` |
| `parent` | text | Layers only: the id of the containing object. A surface's layers are stored as one flat list in drawing order; `parent` builds the tree |
| `placement` | group | Surfaces only: where the window goes on screen |
| `placement.reference` | group | What the surface is placed against |
| `placement.reference.region` | `monitor`, `taskbar`, `system_tray` | Screen area the anchors refer to |
| `placement.reference.display` | integer | Monitor index; 0 is the primary monitor |
| `placement.nest` | `auto`, `taskbar`, `tray_icon`, `desktop`, `floating` | Window host. `auto` is only for themes saved before hosts existed |
| `placement.horizontal` | `left`, `center`, `right` | Point of the reference region to align to |
| `placement.vertical` | `top`, `center`, `bottom` | Point of the reference region to align to |
| `placement.surface_horizontal` | `left`, `center`, `right` | Point of the surface placed there |
| `placement.surface_vertical` | `top`, `center`, `bottom` | Point of the surface placed there |
| `placement.offset_x` | integer | Horizontal offset in pixels |
| `placement.offset_x_expression` | expression | Replaces `offset_x` with an expression |
| `placement.offset_y` | integer | Vertical offset in pixels |
| `placement.offset_y_expression` | expression | Replaces `offset_y` with an expression |
| `anchor` | group | Layers only: the parent corner `x` and `y` start from |
| `anchor.horizontal` | `left`, `center`, `right` | Horizontal anchor |
| `anchor.vertical` | `top`, `center`, `bottom` | Vertical anchor |
| `x` | expression | Layers only: horizontal offset from the anchor, in pixels |
| `y` | expression | Layers only: vertical offset from the anchor, in pixels |
| `width` | expression | Width in pixels. A surface's width and height size its window |
| `height` | expression | Height in pixels |
| `rotation` | expression | Clockwise rotation in degrees |
| `background(none)` | type | No background |
| `background(colour)` | type | A solid colour |
| `background(colour).colour` | paint | The colour |
| `background(gradient)` | type | A linear gradient |
| `background(gradient).start` | paint | Start colour |
| `background(gradient).end` | paint | End colour |
| `background(gradient).angle` | expression | Direction in degrees; 0 runs left to right |
| `background(image)` | type | An image |
| `background(image).path` | text | Image file name in the theme's asset folder |
| `background(image).fit` | `contain`, `cover`, `stretch`, `original` | How the image fills the object |
| `border` | group | Optional outline |
| `border.color` | paint | Outline colour |
| `border.width` | expression | Outline width in pixels |
| `mouse_events` | group | Action scripts run by the mouse |
| `mouse_events.click` | actions | Left click |
| `mouse_events.double_click` | actions | Double click |
| `mouse_events.right_click` | actions | Right click |
| `mouse_events.mouse_enter` | actions | Pointer enters the object |
| `mouse_events.mouse_leave` | actions | Pointer leaves the object |
| `tooltip` | `usage_summary` | Surfaces only: hover popup listing every provider's usage |
| `corner_radius` | expression | Corner radius in pixels |
| `layout` | `freeform`, `row`, `column` | How children are arranged. `freeform` uses each child's anchor, `x` and `y` |
| `align` | `start`, `center`, `end` | Cross-axis alignment of children in a row or column |
| `gap` | expression | Space between children in a row or column |
| `content(none)` | type | Nothing drawn beyond the background |
| `content(text)` | type | Text |
| `content(text).template` | template | The text, with `{expression}` and `{expression:format}` tokens |
| `content(text).font_family` | text | Installed font family name |
| `content(text).font_size` | expression | Size in pixels |
| `content(text).weight` | `light`, `regular`, `medium`, `semibold`, `bold` | Font weight |
| `content(text).rendering` | `antialiased`, `clear_type`, `aliased` | Glyph smoothing |
| `content(text).contrast` | expression | Glyph contrast; higher is crisper. Default `1.4` |
| `content(text).align` | `left`, `center`, `right` | Alignment within the object |
| `content(text).color` | paint | Text colour |
| `content(progress)` | type | A progress bar |
| `content(progress).value` | expression | Filled percentage, 0 to 100, such as `claude.session.display` |
| `content(progress).direction` | `left_to_right`, `right_to_left`, `bottom_to_top`, `top_to_bottom` | Fill direction |
| `content(progress).fill` | paint | Filled part |
| `content(progress).track` | paint | Unfilled part |
| `content(progress).corner_radius` | expression | Bar corner radius |
| `content(progress).segments` | integer | Number of segments; 0 draws a continuous bar |
| `content(progress).segments_expression` | expression | Replaces `segments` with an expression |
| `content(progress).segment_gap` | expression | Space between segments in pixels |
| `content(provider_mark)` | type | A provider's logo, tinted and fitted into the object |
| `content(provider_mark).provider` | text | Provider key, such as `claude` or `codex` |
| `content(provider_mark).color` | paint | Logo colour |
| `children` | objects | Surfaces only: every layer of the surface, in drawing order |

### Paints

| Field | Takes | Meaning |
| --- | --- | --- |
| `color` | colour | The colour, or the dark-mode colour when `light_color` is set |
| `light_color` | colour | Optional colour used while Windows is in light mode |
| `opacity` | expression | Opacity multiplier, 0 to 1. Default `1` |

## Values

Names an expression can read. Text values work in templates and comparisons.

| Name | Meaning | Sample values |
| --- | --- | --- |
| `canvas.*` | Size of the surface being drawn | `canvas.height` = `48`<br>`canvas.width` = `105` |
| `parent.*` | Size of the object's parent; the surface at the root | `parent.height` = `48`<br>`parent.width` = `105` |
| `host.*` | Size of the area hosting the surface: the taskbar or the monitor | `host.height` = `48`<br>`host.width` = `1920` |
| `this.*` | Inside a layer: its own `x`, `y`, `width`, `height` and `gap`; `self` is a synonym in `get(self, ...)` and actions | (inside layers) |
| `object.*` | Inside a layer: its `x`, `y`, `width` and `height` | (inside layers) |
| `pi` | π | `pi` = `3.1416` |
| `e` | Euler's number | `e` = `2.7183` |
| `true` | 1 | `true` = `1` |
| `false` | 0 | `false` = `0` |
| `app.*` | App version as text (`app.version`) and numbers | `app.version` = `"X.Y.Z"`<br>`app.version.major` = `…`<br>`app.version.minor` = `…`<br>`app.version.patch` = `…` |
| `system.dark` | 1 while Windows uses dark mode; prefer `light_color` on paints | `system.dark` = `1` |
| `display.countdown` | 1 when the Usage direction setting is Remaining | `display.countdown` = `0` |
| `data.*` | Polling state: `poll_ok`, `has_error` and `loading` are 1 or 0 | `data.has_error` = `0`<br>`data.loading` = `0`<br>`data.poll_ok` = `1` |
| `i18n.*` | Labels and unit suffixes in the app language | `i18n.cursor_api_window` = `"API"`<br>`i18n.cursor_auto_window` = `"Auto"`<br>`i18n.day_suffix` = `"d"`<br>`i18n.hour_suffix` = `"h"`<br>`i18n.locale` = `"en"`<br>`i18n.minute_suffix` = `"m"`<br>`i18n.now` = `"now"`<br>`i18n.second_suffix` = `"s"`<br>`i18n.session_window` = `"5h"`<br>`i18n.weekly_window` = `"7d"` |
| `providers.*` | `providers.count` enabled providers; `providers.<key>.enabled` is 1 or 0 | `providers.antigravity.enabled` = `0`<br>`providers.claude.enabled` = `1`<br>`providers.codex.enabled` = `1`<br>`providers.copilot.enabled` = `0`<br>`providers.count` = `2`<br>`providers.cursor.enabled` = `0`<br>`providers.grok.enabled` = `0`<br>`providers.opencode.enabled` = `0` |
| `time.now.*` | Current time as Unix seconds and milliseconds | `time.now.milliseconds` = `1800000000000`<br>`time.now.unix` = `1800000000` |
| `time.local.*` | Current local date and time parts; `weekday` is 0 for Monday | `time.local.day` = `15`<br>`time.local.hour` = `8`<br>`time.local.minute` = `0`<br>`time.local.month` = `1`<br>`time.local.second` = `0`<br>`time.local.weekday` = `4`<br>`time.local.year` = `2027` |
| `time.utc.*` | Current UTC date and time parts | `time.utc.day` = `15`<br>`time.utc.hour` = `8`<br>`time.utc.minute` = `0`<br>`time.utc.month` = `1`<br>`time.utc.second` = `0`<br>`time.utc.weekday` = `4`<br>`time.utc.year` = `2027` |
| `antigravity.*` | Antigravity pools: `gemini.*` and `external.*` have the window fields below; `dual_mode`, `crossfade_opacity`, `reported` | `antigravity.crossfade_opacity` = `1`<br>`antigravity.dual_mode` = `0`<br>`antigravity.external.enabled` = `0`<br>`antigravity.gemini.enabled` = `0` |
| `accounts.*` | Named Claude Code and Codex accounts: `accounts.<provider>.<id>.` followed by the window fields below, plus `name`, `selected` and `has_error` | (inside layers) |

### Usage values

Usage values are `<owner>.<window>.<field>`, such as `claude.session.percentage`, or `<owner>.<field>` for the provider itself, such as `claude.available`. Missing values make an expression fail, so check `available` before using a window a provider may not report.

| Owner | Meaning |
| --- | --- |
| `claude` | Claude Code |
| `codex` | Codex |
| `antigravity` | Antigravity |
| `opencode` | OpenCode |
| `cursor` | Cursor |
| `grok` | Grok |
| `copilot` | Copilot |
| `antigravity.gemini` | Antigravity's Gemini pool |
| `antigravity.external` | Antigravity's external-model pool |
| `active` | The first enabled provider with data |

| Window | Meaning | In the sample |
| --- | --- | --- |
| (none) | The provider itself | every owner |
| `account` | The selected account | `antigravity`, `claude`, `codex`, `copilot`, `cursor`, `grok`, `opencode` |
| `session` | Short window: Claude Code and Codex 5 hours; Cursor Auto | every owner |
| `weekly` | Long window: 7 days; Cursor API; OpenCode's long window | every owner |
| `five_hour` | The five-hour window where the provider has one | every owner |
| `monthly` | Monthly window where reported | every owner |
| `credits` | Credit balance where reported | every owner |
| `headline` | Whichever allowance is closest to its limit; best for a single badge | every owner |
| `limits` | Every reported quota | every owner |
| `limits.<key>` | One reported quota by key, such as `weekly_scoped_fable` | `active`, `claude` |
| `model.<model>` | A model's weekly quota, when exactly one matches | `active`, `claude` |
| `scoped` | The single scoped quota the provider marks active | `active`, `claude` |

| Field | Meaning | Example | Sample value |
| --- | --- | --- | --- |
| `available` | 1 when reported; gate optional layers on it | `claude.session.available` | `1` |
| `stale` | 1 while showing earlier figures after a failed poll | `claude.stale` | `0` |
| `has_error` | 1 when the selected account's last poll failed | `claude.has_error` | (not in the sample) |
| `name` | Account name (text) | `claude.account.name` | `""` |
| `count` | Number of reported quotas | `claude.limits.count` | `1` |
| `percentage` | Used, 0 to 100 | `claude.session.percentage` | `42` |
| `remaining` | Remaining, 0 to 100 | `claude.session.remaining` | `58` |
| `display` | Used or remaining, following the Usage direction setting; prefer it for what the widget shows | `claude.session.display` | `42` |
| `label` | Short window name (text) | `claude.session.label` | `"5h"` |
| `balance` | Credits left | `claude.credits.balance` | `0` |
| `total` | Credits granted | `claude.credits.total` | `0` |
| `is_active` | 1 when the provider marks this quota active | `active.limits.weekly_scoped_fable.is_active` | `1` |
| `key` | Quota key (text) | `active.limits.weekly_scoped_fable.key` | `"weekly_scoped_fable"` |
| `kind` | Quota kind (text) | `active.limits.weekly_scoped_fable.kind` | `"weekly_scoped"` |
| `model_id` | Model id (text) | `active.limits.weekly_scoped_fable.model_id` | `""` |
| `scope` | Scope as JSON (text) | `active.limits.weekly_scoped_fable.scope` | `"{"model":{"display_name":"Fable"}}"` |
| `reset.unix` | Reset time as Unix seconds; use a timestamp format | `claude.session.reset.unix` | `1800008100` |
| `reset.seconds` | Seconds until reset; use `duration` or `duration_short` | `claude.session.reset.seconds` | `8100` |
| `reset.minutes` | Minutes until reset | `claude.session.reset.minutes` | `135` |
| `reset.hours` | Hours until reset | `claude.session.reset.hours` | `2.25` |
| `reset.days` | Days until reset | `claude.session.reset.days` | `0.0938` |
