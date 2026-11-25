# Built-in themes

Each `.json` file here is a built-in theme; `assets/` holds their images.

- Most fields are **expressions**, not fixed numbers: `"width": "canvas.width - 8"`,
  `"render": "claude.session.percentage > 80"`. Text layers use templates such as
  `"{claude.session.percentage:0}% used"`.
- [docs/theme-reference.md](../../docs/theme-reference.md) lists every field, value,
  operator, function, format and mouse action, with results computed from the sample
  usage. It is generated from `src/theme_engine/theme_language.rs`; `cargo test` fails
  when it is out of date, and `--theme-reference` prints it.
- Check a theme with `claude-code-usage-monitor --render-theme <theme.json> --out <folder>`.
  It renders PNGs and prints unknown fields and failing expressions.
- Changing a built-in theme changes its snapshots in `tests/snapshots/themes/`; see
  [CONTRIBUTING.md](../../CONTRIBUTING.md#theme-changes).
