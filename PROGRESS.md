# PROGRESS: Todo Lens (Zed extension)

## Goal
A Zed extension that highlights TODO / BUG / FIXME / NOTE in user-chosen colors and lists all of them in the project, searchable, with locations.

## Rules / constraints
- Zed extensions cannot add custom UI (panels, menus, color pickers), as of Zed 1.25 (2026-10). So we use built-in UI:
  semantic tokens (colors, via `global_lsp_settings.semantic_token_rules` in settings.json),
  `workspace/symbol` (cmd-t picker), `documentSymbol` (outline), optional hint diagnostics (cmd-shift-m).
- Extension-provided semantic token rules only apply to languages the extension itself defines, so colors must live in the user's settings.json (snippet in README).
- Semantic tokens require `"semantic_tokens": "combined"` in Zed settings.
- Token legend is fixed at initialize, built from the `tags` setting. Changing tags needs a server restart.
- Positions are UTF-16 (Zed uses PointUtf16 for semantic tokens).
- `lsp-server`'s `initialize_finish` consumes the `initialized` notification. Do post-init work at the start of the main loop.

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
- 2026-10-24: Initial implementation. All unit tests pass (`cd server && cargo test`), LSP smoke test passes.
