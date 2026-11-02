# UI localisation

The desktop app offers **English**, **Français**, **日本語**, **简体中文** (`zh-hans`),
**繁體中文** (`zh-hant`) and **Español (México)** (`es-mx`), in **Settings → General → Language**.
The choice is saved in the existing `language` setting. **System** uses the operating system's
preferred supported language; if none matches, it uses English. An explicit choice takes
precedence over the system language.

Chinese locales without a script, including `zh`, `zh-CN` and `zh-SG`, select Simplified Chinese.
`zh-TW`, `zh-HK` and `zh-MO` select Traditional Chinese. An explicit `Hans` or `Hant` script takes
precedence over the region. Every Spanish locale (`es`, `es-MX`, `es-419`, `es-ES`) selects the
Mexican catalog while it is the only Spanish one. Locale matching accepts underscores, case
differences and Unix encoding suffixes.

## Scope and format

`crates/ui-egui/src/i18n/zh-hans.tsv`, `zh-hant.tsv` and `es-mx.tsv` contain original translations
under the repository's MIT OR Apache-2.0 licence. Each UTF-8 line has two columns:
`English source<TAB>translation`. Comments begin with `#`; `\n` represents a newline. The language
registry in `i18n.rs` embeds the catalogs.

The catalogs cover first run, new projects, project controls, Settings, the workbench, tool titles,
tool groups, target descriptions, tooltips and accessibility labels. The command palette searches
translated tool titles as well as English titles and stable tool IDs. Missing translations fall
back to the original English text. Product and technology names retain their spelling.

User text, AI responses, evidence and raw tool results, tool summaries and schema descriptions,
logs and exported reports retain their original text. The CLI, MCP, tool IDs, stored project data
and the separate browser viewer are unchanged.

Keep placeholders such as `{name}` and `{n}` intact, along with command IDs, shortcuts, escapes and
trailing `…`. Update every translated catalog when English source keys change. Common terms:
净室 / 淨室 / Sala limpia (Clean room), 深度 / Profundo (Deep), 构建 / 建置 / Construir (Build),
证据 / 證據 / Evidencia (Evidence), and 项目 / 專案 / Proyecto (Project).

## Validation

Run:

```sh
cargo test -p craft-anything-ui-egui
cargo test -p xtask
cargo clippy -p craft-anything-ui-egui -p xtask --all-targets -- -D warnings
cargo xtask i18n-coverage
```

Catalog tests check duplicate and empty entries, matching key sets, placeholders and ellipses. The
coverage command checks literal translation calls, common widget labels and registered engine tool
titles and groups against the Chinese catalogs; it also runs in `cargo xtask ci`. It does not prove
coverage of arbitrary runtime text or labels held in tables, so review those when adding or
changing UI code.

The existing snapshot example accepts `--lang zh-hans`, `--lang zh-hant`, `--lang fr`, `--lang ja`
or `--lang es-mx`, for example:

```sh
cargo run -p craft-anything-ui-egui --example snapshot -- --lang es-mx --only settings --out-dir target/snapshots/es-mx
```

Chinese glyphs use the existing craft-fonts fallback, which release and CI builds already include.
Local builds must set `CRAFT_FONTS_DIR` to a craft-fonts checkout before compiling to get the same
glyph coverage. Spanish uses the Latin glyphs already in `assets/fonts`; the catalog adds no font
assets. Check each language, clipping, narrow windows and native file dialogs visually on each
platform; catalog coverage alone does not verify layout or fonts.
