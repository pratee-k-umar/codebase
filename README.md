# Codebase Intel

A Windows desktop app that maps a repository: clone it or open a folder, and it
draws the files, their symbols, and the import relations between them.

Double-click the executable and the window is the whole interface — no terminal,
no command line.

## Features

- **Desktop UI**: start screen with clone / open folder / settings, and a graph canvas
- **Clone and analyze**: enter a repository URL and pick where it lands
- **Multi-language support**: TypeScript/JavaScript, Python, Go, Rust
- **Symbol extraction**: functions, methods, classes, interfaces, types, structs, variables
- **Import resolution**: turns module specifiers into real file-to-file relations
- **Two views**: an interactive relation graph and a structural block diagram
- **Scoping**: select files, expand by 1-3 relations, jump from entry points
- **Graph view**: directory clusters, language-coloured boxes, symbol rows, pan/zoom/drag, filtering, click to inspect
- **Explicit ignore handling**: honours `.gitignore`, `.ignore` and `.git/info/exclude`, and reports every skipped file with its reason
- **Export**: write the full analysis result to JSON

## What gets analyzed

Discovery is one explicit, reportable pass over the tree. A file is skipped when
it matches, in order:

| # | Rule | Reported reason |
|---|------|-----------------|
| 1 | The analysis cache file | `cache_file` |
| 2 | A configured `exclude` glob | `config_exclude` |
| 3 | `.git/info/exclude` | `git_exclude` |
| 4 | `.ignore` | `ignore_file` |
| 5 | `.gitignore` (nested files and `!` negation included) | `gitignore` |
| 6 | Not a supported/`enabled` language | counted as *unsupported* |
| 7 | Larger than the per-language size limit | `too_large` |

`.git` itself is pruned with reason `git_metadata`, and directories matching the
configured excludes are pruned without descending into them.

Notes:

- `.gitignore` is applied even outside a git checkout, so exported archives and
  vendored snapshots behave the same as a clone.
- Only `.gitignore` files inside the scanned root are read; a `.gitignore` in a
  parent directory is not consulted.
- Global git excludes (`core.excludesFile`) are not read.
- `max_file_size_mb = 0` in the config means "no limit".

Every scan records a discovery summary. It is shown in the details panel after a
scan and written into the exported JSON, so you can always explain why a file is
missing from the graph.

## Installation

Requires Rust (stable) and Git on `PATH` for cloning.

```bash
cargo build --release
```

The app is `target/release/codebase-intel.exe`. Copy it anywhere and
double-click; it is a single file with no installer and no runtime to install.

## Usage

Double-click `codebase-intel.exe`, or run it from a terminal. There is no
command line: the window is the whole interface.

### Start screen

Three ways in:

| Option | What it does |
|---|---|
| **Clone a repository** | Asks for a repository URL and a destination folder (with a native folder picker), clones it shallowly, then analyzes it |
| **Open a local folder** | Analyzes a repository that is already on disk |
| **Settings** | Languages, extra exclude patterns, analysis cache |

The last analyzed repository appears as a shortcut under **Recent**.

### After analysis

Two tabs share the same selection:

**Graph** — a node-link map:

- Each **box** is a file, grouped into directory clusters
- The **coloured header** is the file's language
- **Rows inside a box** are the file's symbols (kind tag + name); long files show the first 12 with a `+N more` row
- **Lines between boxes** are resolved import relations; the arrow points at the imported file
- **Relations are drawn at every zoom level.** Zoomed out they are straight lines; zoomed in they become routed with arrowheads and labels

**Block diagram** — a document-style structural view:

- One **card per file**, with classes/structs and their methods **nested inside**
- Import arrows between cards
- Better than the graph when you want to read what is inside a few files

### Scope

The left panel decides what is drawn. It is the cure for a 2,000-file hairball.

| Control | Effect |
|---|---|
| **All / Near / Only** | Everything · ticked files plus neighbours · ticked files only |
| **hops** (1-3) | How far the neighbourhood reaches |
| Filter + checkboxes | Tick the files you care about; the first tick switches to *Near* automatically |
| **Select all** / **Clear** | Bulk selection |
| **Starting points** | Files nothing imports but which import others; click one to jump to it. Marked with a green dot in both views |

On a large repository the graph opens focused on the most-connected entry point
rather than zooming out to an unreadable overview — press **Fit** to see
everything.

Controls:

| Action | Result |
|---|---|
| Scroll | Zoom, centred on the pointer |
| Drag the background | Pan |
| Drag a box | Move that file |
| Click a box | Inspect it in the right-hand panel |
| `Esc` | Clear the selection |
| Filter box | Dim everything that does not match |
| **Fit** / **Re-layout** | Reset zoom, or recompute the directory layout |
| **Export JSON…** | Write the full analysis result to a file |

The right-hand panel shows, for the selected file: language, symbol list with
line numbers, and both directions of its relations (clickable, to jump).

With nothing selected it shows the scan summary: files, symbols, relations,
scan duration, what discovery skipped and why, and the per-language symbol
counts.

### What a "relation" means

An edge is drawn only when an import resolves to another file that was actually
scanned. Resolution understands:

| Language | Resolved |
|---|---|
| TypeScript / JavaScript | `./x`, `../y`, extensionless and `./x.js` → `x.ts`, directory `index.*` |
| Python | `from .mod`, `from ..pkg.mod`, `import a.b` (`a/b.py` or `a/b/__init__.py`) |
| Go | import paths matched against directory suffixes, so the module prefix can be dropped |
| Rust | `crate::`, `self::`, `super::` against `file.rs` and `mod.rs` |

External packages (`node_modules`, crates.io, Go modules outside the tree, `std`)
are deliberately left unresolved rather than guessed, so you never see invented
edges. Unresolved imports simply do not appear.

### Settings file

Settings live outside the repository:

```
%APPDATA%\codebase-intel\settings.json
```

They are written when you save the settings screen, start a clone, or start a
scan.

## Architecture

```
codebase-intel/
├── crates/
│   ├── core/                    # Analysis engine (no UI concerns)
│   │   ├── src/discovery.rs     # Which files to analyze, and why others are skipped
│   │   ├── src/resolve.rs       # Import specifiers → file relations
│   │   ├── src/parser.rs        # Language adapter registry (parallel parsing)
│   │   ├── src/parsers/         # tree-sitter adapters: TypeScript, Python, Go, Rust
│   │   ├── src/model.rs         # Symbols, references, graphs, metrics
│   │   ├── src/graph.rs         # Graph construction, SCC, coupling, dead code
│   │   ├── src/analyzer.rs      # Pipeline orchestration + progress reporting
│   │   ├── src/export.rs        # JSON, YAML, summary, Mermaid, GraphML, DOT, CSV
│   │   └── src/incremental.rs   # Hash-based cache
│   └── app/                     # Desktop app (eframe/egui)
│       ├── src/main.rs          # Window setup, icon, panic dialog
│       ├── src/app.rs           # Screens and state machine
│       ├── src/graph.rs         # Graph model, layout, canvas rendering
│       ├── src/repo.rs          # Clone + folder validation
│       ├── src/scan.rs          # Background analysis with progress
│       └── src/settings.rs      # Persisted settings
```

Pipeline: **discover → parse → build symbols → resolve imports → build graphs →
detect APIs/DB → compute metrics → render**. Discovery runs first and its report
travels with the result, so every stage downstream only sees files that passed
the ignore rules. The app never blocks: analysis runs on a worker thread and the
window polls for progress once per frame.

### Known limits

- Only imports that resolve inside the scanned tree become relations. TypeScript
  path aliases (`@app/*`) and workspace package names are not resolved yet.
- References and the call graph are still stubs, so the graph shows file-level
  imports only.
- Complexity metrics are not computed yet.

## Extending

### Add a new language

1. Add the tree-sitter grammar dependency in the workspace `Cargo.toml`
2. Add a variant to `Language` in `core/src/model.rs` (extension mapping + grammar)
3. Implement the `LanguageParser` trait in `core/src/parsers/<language>.rs`
4. Register the adapter in `ParserRegistry::new()`

### Add a new framework detector

Add detection logic in `analyzer.rs` in `detect_api_endpoint()` or `detect_database()`.

### Add a new import resolution rule

Extend `FileIndex` in `core/src/resolve.rs` with a per-language `resolve_*`
method and probe helpers. Keep unresolved specifiers unresolved rather than
guessing.

### Add a new ignore source

Extend `discover()` in `core/src/discovery.rs`: add a variant to `IgnoreReason`,
build a matcher (or predicate), and check it in the classification loop.

## License

MIT OR Apache-2.0


