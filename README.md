# Todo Lens for Zed

Highlights `TODO`, `BUG`, `FIXME` and `NOTE` comments in the colors you choose, and lists every one of them in your project in Zed's searchable symbol picker.

```rust
// TODO: handle the empty case        ← "TODO" in your TODO color
let x = 1; // BUG(lenni): off by one  ← "BUG(lenni):" in your BUG color
/* FIXME: leaks on error */
```

## Features

| What | Where in Zed |
|---|---|
| Colored keywords (and optionally the text after them) | in the editor, through semantic tokens |
| Every tag in the project, fuzzy-searchable, with `file:line` | **Project Symbols**: `cmd-t` (macOS) / `ctrl-t`, then type e.g. `todo`, `bug parser`, `fixme src/api` |
| The tags in the current file | **Outline**: `cmd-shift-o`, or the Outline Panel |
| A panel with every tag (optional) | **Project Diagnostics**: `cmd-shift-m`, once `"diagnostics": true` is set |

Zed extensions can't add their own panels or menus yet, so Todo Lens uses these built-in views. Tags use your theme's keyword color by default. You can pick your own colors in `settings.json` (see below).

The project is scanned in the background when the server starts. The scan respects `.gitignore` and skips hidden files, binaries and files over 1 MB. Open buffers update as you type, and other files update when they change on disk.

## Setup

### 1. Install

**From source (local development):**
```sh
cargo install --path server      # puts todo-lens-ls on your PATH
```
Then in Zed: **Extensions → Install Dev Extension** and choose this folder.

**From a release:** if `todo-lens-ls` isn't on `PATH`, the extension downloads the server binary from this repo's latest GitHub release. This only works when the repo is public.

### 2. Enable semantic tokens

Add this one line to your **global** Zed `settings.json` (`cmd-,`):

```jsonc
{
  "semantic_tokens": "combined"
}
```

Tags are now highlighted in your theme's keyword color. Zed only asks language servers for semantic tokens when this is on, and extensions can't change it for you.

`"combined"` also turns on semantic highlighting from your other language servers, such as rust-analyzer. If you only want it for some languages, set it per language instead: `"languages": { "Rust": { "semantic_tokens": "combined" } }`.

### Choose your colors (optional)

Each tag is sent as a `keyword` token (the keyword itself) and a `comment` token (the text after it), with the tag name in lowercase as a modifier. Add rules for the ones you want to recolor. These go in the **global** `settings.json`; Zed doesn't allow `global_lsp_settings` in a project's `.zed/settings.json`.

```jsonc
{
  "global_lsp_settings": {
    "semantic_token_rules": [
      // The keyword itself
      { "token_type": "keyword", "token_modifiers": ["todo"],  "foreground_color": "#FFB000", "font_weight": "bold" },
      { "token_type": "keyword", "token_modifiers": ["bug"],   "foreground_color": "#FF4D4D", "font_weight": "bold" },
      { "token_type": "keyword", "token_modifiers": ["fixme"], "foreground_color": "#FF7A00", "font_weight": "bold" },
      { "token_type": "keyword", "token_modifiers": ["note"],  "foreground_color": "#4DA6FF", "font_weight": "bold" },

      // Optional: the text after the keyword
      { "token_type": "comment", "token_modifiers": ["todo"],  "foreground_color": "#FFB000" },
      { "token_type": "comment", "token_modifiers": ["bug"],   "foreground_color": "#FF4D4D" },
      { "token_type": "comment", "token_modifiers": ["fixme"], "foreground_color": "#FF7A00" },
      { "token_type": "comment", "token_modifiers": ["note"],  "foreground_color": "#4DA6FF" }
    ]
  }
}
```

Each rule also accepts `background_color`, `font_style: "italic"`, `underline` (`true` or a color) and `strikethrough`. These rules only affect Todo Lens tokens, because other language servers don't use these modifiers.

### 3. Options (optional)

```jsonc
{
  "lsp": {
    "todo-lens": {
      "settings": {
        "tags": ["TODO", "BUG", "FIXME", "NOTE", "HACK"], // case-sensitive
        "comment_only": true,       // only match inside comments (prose files like .md always match)
        "diagnostics": false,       // also show tags in Project Diagnostics
        "exclude": ["vendor/**", "*.min.js"],
        "max_file_size_kb": 1024
      }
    }
  }
}
```

A custom tag `HACK` gets the modifier `hack`; add rules with `"token_modifiers": ["hack"]` to color it. Up to 32 tags are supported. After changing `tags`, run **editor: restart language server**.

## How comments are detected

Todo Lens doesn't depend on any one language. A tag counts when it follows a comment marker on the same line: `//`, `#` (but not `#[`), `-- `, a `;` or `%` at the start of a line, or when it's inside `/* */`, `<!-- -->`, `{- -}` or `(* *)`. The marker detection doesn't understand strings, so `"// TODO"` inside a string literal is matched too. Set `"comment_only": false` to match tags anywhere.

## Troubleshooting

- **No colors:** check that `semantic_tokens` is `"combined"` (or `"full"`), then run **editor: restart language server**. **dev: open highlights tree view** shows which tokens are applied.
- **Tags missing from `cmd-t`:** the server starts once you open a file of a supported language. **dev: open language server logs → todo-lens** shows the scan result.
- **A language isn't covered:** add it to `languages` in `extension.toml`.

## Development

```
extension.toml / src/lib.rs   Zed extension (WASM): finds or downloads the server and starts it
server/                       todo-lens-ls, the language server (lsp-server + ignore + regex)
  src/scanner.rs              tag + comment detection (unit tested)
  src/index.rs                workspace scan, path filter, fuzzy search
  src/main.rs                 LSP: semantic tokens, workspace/document symbols, diagnostics
```

```sh
cd server && cargo test
```

To release, push a `v*` tag. `.github/workflows/release.yml` builds `todo-lens-ls-<target>.tar.gz/.zip` for macOS, Linux and Windows.
