# PROGRESS: Todo Lens (Zed extension)

## Goal
A Zed extension that highlights TODO / BUG / FIXME / NOTE in user-chosen colors and lists all of them in the project, searchable, with locations.

## Rules / constraints
- Zed extensions cannot add custom UI (panels, menus, color pickers), as of Zed 1.25 (2026-10). So we use built-in UI:
  semantic tokens (colors, via `global_lsp_settings.semantic_token_rules` in settings.json),
  `workspace/symbol` (cmd-t picker), `documentSymbol` (outline), optional hint diagnostics (cmd-shift-m).
- Extension-provided semantic token rules only apply to languages the extension itself defines. So tags are sent as the standard `keyword` type (text after them as `comment`) with a per-tag modifier (`todo`, `bug`, ...). That way the theme's keyword color works with zero config, and users override with `{token_type: keyword, token_modifiers: [todo]}` in global settings.json. Idea taken from the existing `todo-highlight-language-server` extension.
- Semantic tokens require `"semantic_tokens": "combined"` in Zed settings.
- Token legend (modifiers) is fixed at initialize, built from the `tags` setting (max 32). Changing tags needs a server restart.
- Positions are UTF-16 (Zed uses PointUtf16 for semantic tokens).
- `lsp-server`'s `initialize_finish` consumes the `initialized` notification. Do post-init work at the start of the main loop.

## Marketplace
- Extension ID `todo-lens-lsp` (registry requires an `-lsp`/`-language-server` suffix for LSP-only extensions; the ID can't contain `zed`/`extension`). Language server name stays `todo-lens` (the settings key).
- Registry rules: no duplicate functionality. `todo-highlight-language-server` (shionit/zed-todo-highlight) already does highlighting; our distinguishing feature is the project-wide list (workspace/symbol), outline and diagnostics.
- Submission = PR to zed-industries/extensions adding submodule `extensions/todo-lens-lsp` + `extensions.toml` entry, then `pnpm sort-extensions`. Must be tested in Zed at the submitted commit.

## Architecture
- `extension.toml`, `src/lib.rs`: WASM extension (zed_extension_api 0.7, target wasm32-wasip2). Binary lookup order: `lsp.todo-lens.binary.path` → `todo-lens-ls` on PATH → cached download → GitHub release `Lenni707/zed-todo-lens` asset `todo-lens-ls-<arch>-<os>.tar.gz|zip`.
- `server/`: `todo-lens-ls` (lsp-server 0.10, lsp-types 0.97, ignore, regex). Full text sync; background parallel workspace scan; file watcher registration `**/*` filtered by root `.gitignore` + `exclude` + hidden.

## Checklist
- [x] Scanner + tests (comment heuristics, author, utf16, prose files, custom tags)
- [x] Index + workspace scan + PathFilter + fuzzy search + tests
- [x] LSP: semantic tokens, workspace/document symbols, diagnostics, watched files, config changes
- [x] Scripted LSP smoke test (scratchpad python client) passes
- [x] Extension compiles to wasm32-wasip2
- [x] README, release workflow
- [ ] Verified inside Zed GUI (Install Dev Extension). Needs the user
- [ ] First tagged release (repo must be public for the downloader)

## Log
- 2026-10-24: v0.1.1: keyword/comment tokens with per-tag modifiers (works without color rules), ID renamed to todo-lens-lsp, release workflow builds a draft first and publishes when all binaries are uploaded.
- 2026-10-24: Initial implementation. All unit tests pass (`cd server && cargo test`), LSP smoke test passes.
