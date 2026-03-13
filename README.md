# deadcheck

> Fast dead code detector for JavaScript and TypeScript projects — written in Rust.

`deadcheck` scans your project, builds a dependency graph, and finds code that is never reachable from your entry points: unused files, unused exports, and unused npm dependencies.

```
Dead Code Analysis
  Scanned 1,284 files  •  6 dead files  •  14 unused exports  •  3 unused dependencies

Dead Files

  HIGH confidence
  • src/components/OldNavbar.tsx
  • src/utils/legacyAuth.ts

  MEDIUM confidence
  • src/hooks/useUserData.ts

Unused Exports

  src/auth/session.ts
    • createLegacyToken
    • validateOldSession

Unused npm Dependencies
  (not imported in any source file)
  • moment
  • lodash
  • uuid
```

## Features

- **Unused file detection** — files not reachable from any entry point
- **Unused export detection** — symbols exported but never imported
- **Unused dependency detection** — npm packages listed but never imported
- **Confidence-based output** — HIGH / MEDIUM / LOW so you know what's safe to remove
- **Safe delete** — `--fix` moves files to `.deadcode/` with a manifest (fully reversible)
- **Framework-aware** — detects Next.js (App & Pages Router), Vite, Remix, CRA entry points automatically
- **Path alias support** — reads `tsconfig.json` `paths` so `@/components/...` resolves correctly
- **JSON output** — machine-readable via `--json` for CI pipelines
- **Dependency graph export** — `--graph` writes a Graphviz DOT file
- **Watch mode** — `--watch` re-runs analysis on every file change
- **Fast** — parallel parsing via Rayon; handles 2 000+ file projects in under 2 seconds

## Installation

### curl (macOS and Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/starc007/deadcheck/main/install.sh | bash
```

To install a specific version:

```bash
DEADCHECK_VERSION=v0.1.0 curl -fsSL https://raw.githubusercontent.com/starc007/deadcheck/main/install.sh | bash
```

### From source (requires Rust 1.88+)

```bash
git clone https://github.com/starc007/deadcheck
cd deadcheck
cargo install --path .
```

### Homebrew (coming soon)

```bash
brew install deadcheck
```

## Usage

```bash
# Scan current directory
deadcheck

# Scan a specific project
deadcheck /path/to/your/project

# Only show HIGH confidence dead code (safest)
deadcheck --min-confidence high

# Safe delete — moves dead files to .deadcode/
deadcheck --fix

# Machine-readable JSON output (great for CI)
deadcheck --json

# Export dependency graph (Graphviz DOT)
deadcheck --graph

# Watch mode — re-analyzes on every change
deadcheck --watch

# Add extra entry points on top of auto-detected ones
deadcheck --entry src/workers/background.ts

# Exclude generated code
deadcheck --ignore "src/generated/**" --ignore "**/*.stories.tsx"
```

## Options

| Flag | Short | Description |
|------|-------|-------------|
| `--fix` | `-f` | Move dead files to `.deadcode/` (reversible) |
| `--json` | `-j` | Machine-readable JSON output |
| `--graph` | `-g` | Export `dependency-graph.dot` (Graphviz) |
| `--watch` | `-w` | Re-run on file changes |
| `--min-confidence` | `-c` | Minimum level to show: `high`, `medium`, `low` (default: `low`) |
| `--entry <FILE>` | `-e` | Add an extra entry point |
| `--ignore <PATTERN>` | `-i` | Glob pattern to exclude |
| `--config <FILE>` | | Path to `deadcheck.config.json` |

## Configuration file

Create `deadcheck.config.json` in your project root for persistent settings:

```json
{
  "entryPoints": ["src/workers/sw.ts", "src/iframe.ts"],
  "ignore": ["src/generated/**", "**/*.stories.tsx", "**/*.test.ts"],
  "minConfidence": "medium",
  "ignoreDependencies": ["eslint", "prettier", "typescript"]
}
```

## How it works

1. **Scan** — find all `.js`, `.ts`, `.jsx`, `.tsx` files (respects `.gitignore`)
2. **Parse** — extract every `import` / `export` from each file in parallel using [SWC](https://swc.rs/)
3. **Resolve** — turn specifiers into absolute paths (handles relative paths, extensions, `tsconfig.json` aliases)
4. **Graph** — build a directed dependency graph with [petgraph](https://docs.rs/petgraph)
5. **Traverse** — BFS from all entry points; unreachable files are dead
6. **Score** — assign confidence based on signals (dynamic imports, framework patterns, barrel files)
7. **Report** — display results or output JSON

### Confidence scoring

Each dead file is scored on a 0–100 scale based on signals:

| Signal | Effect |
|--------|--------|
| Not imported by any file | +40 |
| Not an entry point | +20 |
| No framework route match | +15 |
| Dynamic `import()` may reference it | −35 |
| Matches framework route pattern | −30 |
| Is a barrel file | −20 |
| Lives in `public/` or `assets/` | −25 |
| Appears to be a test file | −10 |
| Uses `export *` | −10 |

Score ≥ 75 → **HIGH** · Score 40–74 → **MEDIUM** · Score < 40 → **LOW**

### Safe delete (`--fix`)

`deadcheck --fix` never deletes files. It moves HIGH and MEDIUM confidence dead files to:

```
.deadcode/
  OldNavbar.tsx
  legacyAuth.ts
  manifest.json   ← records what was moved and when
```

To restore everything:

```bash
# Move all files back from .deadcode/
cat .deadcode/manifest.json | jq -r '.entries[].original_path' | xargs -I{} mv .deadcode/{} {}
```

### Entry point detection

Entry points are detected automatically (in priority order):

1. `--entry` CLI flags
2. `entryPoints` in `deadcheck.config.json`
3. `main` / `module` / `exports` fields in `package.json`
4. Common file names: `index.ts`, `main.ts`, `app.tsx`, `server.ts` (at root or `src/`)
5. **Next.js App Router** — `page.tsx`, `layout.tsx`, `route.ts`, `middleware.ts` inside `app/`
6. **Next.js Pages Router** — all files directly under `pages/`
7. **Remix** — files under `app/routes/`
8. **Vite** — `src/main.tsx` + `vite.config.ts`
9. Config files: `vite.config.ts`, `next.config.mjs`, etc.

### Path aliases

`deadcheck` reads `tsconfig.json` to resolve path aliases:

```json
{
  "compilerOptions": {
    "baseUrl": "./src",
    "paths": {
      "@/*": ["./src/*"],
      "@components/*": ["./src/components/*"]
    }
  }
}
```

Imports like `import { Button } from "@/components/Button"` are resolved correctly.

## Limitations

- CommonJS `require()` calls are parsed as static imports where the specifier is a string literal. Variable-based `require()` calls are treated as unresolvable.
- Declaration files (`.d.ts`) are excluded from analysis — they contain no runtime code.
- The tool does not execute your project's bundler or TypeScript compiler. Module resolution is heuristic and may differ from your exact build setup in edge cases (conditional exports, complex `moduleResolution` settings).
- Monorepo workspace support is experimental (Phase 2+).

## Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/my-feature`)
3. Make your changes with tests
4. Run `cargo test` and `cargo clippy -- -D warnings`
5. Submit a pull request

## License

DO WHATEVER YOU WANT WITH THIS
