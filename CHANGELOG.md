# Changelog

## [0.2.0] - 2026-03-16

### Fixed
- **Fully-qualified class names**: Sentinel now emits proper nested `module`/`class` declarations instead of bare class names. Classes like `Top::Middle::Set` are wrapped in `module Tool; module IdleRuleHandlers; class Set` rather than emitting a flat `class Set` that collides across namespaces.
- **Compact namespace support**: Classes using `class Proxy::Set` syntax are now recognized (previously emitted `class UnknownClass`).
- **Multiple method signatures**: All `#:` annotated methods in a class are now emitted. Previously only the first method was captured; subsequent annotations inside `body_statement` were silently dropped.
- **Editor temp file filtering**: Sentinel no longer attempts to transpile `sed` temp files (`.!PID!filename.rb`) or editor swap files, which caused spurious "No such file or directory" errors.
- **Watcher debounce**: Replaced destructive event drain with a collect-then-process debounce. The old logic discarded pending events indiscriminately, causing ~1 in 3 saves to be missed.

### Added
- **Angle bracket lint plugin**: Warns when RBS output contains `Array<Hash>` style generics instead of the correct `Array[Hash]` square bracket syntax.
- **Linux platform binaries**: Gem now ships `aarch64-linux` and `x86_64-linux` binaries in addition to the existing macOS builds.
- **Release script**: `scripts/release.sh` cross-compiles all 4 platform binaries and packages the gem.

### Changed
- Removed unused dependencies (`reqwest`, `serde`, `serde_json`, `lsp-types`), reducing binary size from ~5MB to ~3MB.
- Replaced tree-sitter query-based method extraction with direct AST walking for correctness.

## [0.1.0] - 2026-03-16

### Added
- Initial release.
- Tree-sitter based Ruby parser that extracts `#:` (rbs_inline) type annotations and generates `.rbs` signature files.
- File watcher daemon that monitors `./app` and auto-generates signatures into `./sig/generated`.
- **VoidArgument lint plugin**: Warns on `void` used as a method parameter type.
- **TypeCase lint plugin**: Warns on lowercase primitive types (`string`, `integer`, etc.).
- Ruby gem wrapper (`sentinel`) with platform-specific Rust binary dispatch.
- macOS binaries (aarch64, x86_64).
