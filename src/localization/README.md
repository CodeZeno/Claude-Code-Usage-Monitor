# Adding a language

Each supported language is defined by one TOML file in `locales/`. The build
script discovers these files and generates the Rust locale registry, language
selectors, locale matching, native menu command mapping, and optional Windows
font metadata.

To add a language:

1. Copy `locales/en.toml` to a file named for the new locale, such as
   `locales/it.toml`.
2. Set `code`, `native_name`, `locale_patterns`, and the next unused `order`.
3. Translate every value in `[strings]` and `[translations]`.
4. If the script needs a Windows font fallback, set both `windows_font_name`
   and `windows_font_file`.
5. Run `cargo build`.

No Rust source changes are required. The build fails with a focused error when
locale codes or orders are duplicated, keys are missing or unknown, values are
empty, font metadata is incomplete, or placeholders such as `{name}` and
`{version}` differ from English.

`en.toml` is the canonical catalogue. When UI text is added or removed, update
the English file and make the same key change in every other locale file.
