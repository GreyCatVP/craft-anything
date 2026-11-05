# UI localisation

The desktop app offers **Čeština** (`cs`), **Deutsch** (`de`), **한국어** (`ko`), **Русский** (`ru`), **简体中文** (`zh-hans`)
and **繁體中文** (`zh-hant`), alongside English, French and Japanese, in **Settings → General → Language**. The choice is saved in the
existing `language` setting. **System** uses the operating system's preferred
supported language; if none matches, it uses English. An explicit choice takes
precedence over the system language.

Chinese locales without a script, including `zh`, `zh-CN` and `zh-SG`, select
Simplified Chinese. `zh-TW`, `zh-HK` and `zh-MO` select Traditional Chinese.
An explicit `Hans` or `Hant` script takes precedence over the region. Locale
matching accepts underscores, case differences and Unix encoding suffixes.

## German

**Deutsch** (`de`) uses the informal "du". `de`, `de-DE`, `de-AT` and `de-CH` select it, as does the
system language when it is German. `crates/ui-egui/src/i18n/de.tsv` has the same keys as the Chinese
catalogs (a unit test compares them), written from the English source strings, not from the Chinese.
It was drafted with an AI and not yet reviewed by a native speaker; corrections are welcome.
Product and technology names, tool ids and shortcuts keep their spelling. The kind detection that reads
a request ("a game", "a website") also knows the German words *Spiel*, *Webseite* and *Internetseite*,
so a translated suggestion keeps its project kind.

## Scope and format

`crates/ui-egui/src/i18n/cs.tsv`, `de.tsv`, `ko.tsv`, `ru.tsv`, `zh-hans.tsv` and `zh-hant.tsv` contain original
translations under the repository's MIT OR Apache-2.0 licence. Each UTF-8 line
has two columns: `English source<TAB>translation`. Comments begin with `#`;
`\n` represents a newline. The language registry in `i18n.rs` embeds the catalogs.

The catalogs cover first run, new projects, project controls, Settings, the
workbench, tool titles, tool groups, target descriptions, tooltips and
accessibility labels. The command palette searches translated tool titles as
well as English titles and stable tool IDs. Missing translations fall back to
the original English text. Product and technology names retain their spelling.

User text, AI responses, evidence and raw tool results, tool summaries and schema
descriptions, logs and exported reports retain their original text. The CLI,
MCP, tool IDs, stored project data and the separate browser viewer are unchanged.

Keep placeholders such as `{name}` and `{n}` intact, along with command IDs,
shortcuts, escapes and trailing `…`. Update every catalog (`cs`, `de`, `ko`, `ru`, `zh-hans`, `zh-hant`)
when English source keys change; the tests require matching key sets. Common terms include 净室 / 淨室 (Clean room), 深度 (Deep),
构建 / 建置 (Build), 证据 / 證據 (Evidence), and 项目 / 專案 (Project).
The Korean catalog `ko.tsv` uses 클린룸 (Clean room), 심층 (Deep), 분석 / 만들기 / 검사 / 완성
(Study / Build / Check / Ready), 근거 (Evidence) and 프로젝트 (Project); short labels are nouns and
sentences use the formal 합니다 style.

## Right-to-left text

egui 0.36 joins Arabic letters but doesn't reorder bidirectional text: it places each word left
to right, so an Arabic sentence showed its words in reverse order, mixed Arabic/English lines in
a jumble, and the caret, selection and mouse in a text field all worked from that reversed
layout. `ui-egui/src/bidi.rs` breaks text into lines itself (egui gives an Arabic word more
glyphs than letters, so a galley's glyph counts can't say where rows break), reorders each line
with the Unicode Bidirectional Algorithm (`unicode-bidi`), and aligns right-to-left paragraphs to
the right. A paragraph's direction is its first strong letter's, as in browsers.

Where it's used: every `ui.label`, wrapping and truncated label (a cut right-to-left line keeps
its start and puts "…" on the left), text painted directly (sidebar project names, the workbench,
the palette), and the prompt box, where `bidi` draws the text, caret and selection over egui's
hidden ones and places the caret from mouse clicks and drags. Text with no right-to-left letters
takes egui's usual path, unchanged.

Limits: right-to-left labels aren't selectable (a selection would copy the visual order; messages
have a copy button) and skip the glossary underlines; inside one Arabic word the caret and
selection edges are approximate (a part of a word is measured on its own); other text fields
(renaming, settings), buttons, menus and tooltips still draw right-to-left text left to right.

## Validation

Run:

```sh
cargo test -p craft-anything-ui-egui
cargo test -p xtask
cargo clippy -p craft-anything-ui-egui -p xtask --all-targets -- -D warnings
cargo xtask i18n-coverage
```

Catalog tests check duplicate and empty entries, matching key sets, placeholders
and ellipses. The coverage command checks literal translation calls, common
widget labels and registered engine tool titles and groups; it also runs in
`cargo xtask ci`. It does not prove coverage of arbitrary runtime text or labels
held in tables, so review those when adding or changing UI code.

The existing snapshot example accepts any catalog code with `--lang`, such as
`--lang zh-hant` or `--lang ko`, for example:

```sh
cargo run -p craft-anything-ui-egui --example snapshot -- --lang zh-hant --only settings --out-dir target/snapshots/zh-hant
```

Chinese and Korean glyphs use the existing craft-fonts fallback, which release and CI builds
already include. Local builds must set `CRAFT_FONTS_DIR` to a craft-fonts checkout
before compiling to get the same glyph coverage. The catalogs add no font assets.
Check each script, clipping, narrow windows and native file dialogs visually
on each platform; catalog coverage alone does not verify layout or fonts.
