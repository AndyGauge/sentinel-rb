# Changelog

## [0.3.1] - 2026-03-29

### Added
- **Class method support (`def self.method_name`)**: Singleton methods are now detected and emitted as `def self.name: sig` in RBS output.
- **`class << self` block support**: Methods defined inside singleton class blocks are emitted as class methods.
- **`# @rbs type` alias support**: Single-line and multi-line type alias declarations are parsed and emitted as `type name = definition` in RBS output.
- **`attr_reader`/`attr_writer`/`attr_accessor` support**: Typed attribute annotations using `#:` are detected and emitted (e.g., `attr_reader name: String`). Supports multiple symbols per call.

### Changed
- Refactored class body traversal into `flatten_children()` + `scan_body()` to support all new node types without duplication.
- Files containing only type aliases or attributes are no longer skipped during `init` and `check`.
- `is_balanced()` uses stack-based delimiter matching instead of a simple depth counter.

## [0.3.0] - 2026-03-26

### Added
- **`sentinel check` command**: Read-only verification that generated RBS signatures are up to date. Exits with code 1 if any files are missing or stale. Designed for CI pipelines and Git pre-commit hooks.

## [0.2.2] - 2026-03-18

### Added
- **`.sentinel.toml` config file**: Sentinel now reads watched folders and output path from a `.sentinel.toml` file at the project root. Created automatically on first `init` or `watch` with `app` as the default folder.
- **`sentinel add <folder>`**: Add a folder to the watch list from the CLI.
- **`sentinel remove <folder>`**: Remove a folder from the watch list.
- **`sentinel list`**: Display current watched folders and output path.
- **Multi-folder support**: `init` and `watch` now process all configured folders, not just `./app`.

### Changed
- `serde` and `toml` added as dependencies for config serialization.
- Output path (`sig/generated`) is now configurable via the `output` key in `.sentinel.toml`.

## [0.2.1] - 2026-03-17

### Added
- **`sentinel init` command**: Parallel batch RBS generation using rayon. Scans all `.rb` files in `./app` and generates signatures across all CPU cores.
- **Init-on-watch**: `sentinel watch` (the default) now runs a full init before starting the file watcher, ensuring `sig/generated` is always complete on startup.
- **Skip unannotated files**: Files with no `#:` annotations are skipped entirely, avoiding unnecessary I/O. On a ~5k file codebase, this reduced init time from ~1.4s to ~0.4s.

### Changed
- **Gem renamed to `rbs-sentinel`**: The gem is now `gem 'rbs-sentinel'` on rubygems.org. The `bundle exec sentinel` command is unchanged.
- Added `rayon` and `walkdir` dependencies for parallel file processing.

## [0.2.0] - 2026-03-16

### Fixed
- **Fully-qualified class names**: Sentinel now emits proper nested `module`/`class` declarations instead of bare class names. Classes like `Top::Middle::Set` are wrapped in `module Top; module Middle; class Set` rather than emitting a flat `class Set` that collides across namespaces.
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
