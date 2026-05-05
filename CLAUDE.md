# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this project is

`ddr-chart-tools` is a Rust CLI that converts song and chart assets between Dance Dance Revolution (arcade, SSQ + XWB) and StepMania 5 (SSC + OGG), and modernizes legacy DDR SSQs (including Xbox Ultramix era) into either target. It is a single-binary, offline, batch-capable tool. Audio conversion is always paired with chart conversion.

Pipeline shape: `CLI args → job planner → per-job converter → format parse/write → disk I/O`. All conversions go through a shared in-memory model in `src/model/`; there is no point-to-point converter.

## Source of truth: `.spec/`

Before making non-trivial decisions, read the relevant file under `.spec/`. Do not invent conventions that conflict with these.

- `.spec/steering/product.md` — format matrix, domain glossary, **business rules** (numbered 1–10; violations are bugs), out-of-scope list.
- `.spec/steering/tech.md` — tech stack, dependency rationale, architecture patterns, common technical gotchas (SSQ endianness, per-file TPS, legacy origin-shift, XWB ADPCM vs IMA, etc.).
- `.spec/steering/structure.md` — module layout, where new code goes, naming conventions, things agents should *not* do.
- `.spec/steering/rust-cli-standards.md` — authoritative Rust coding standards for this repo.
- `.spec/workflow/{feature}/` — per-feature requirements, design, and tasks for in-flight work.
- `.spec/learnings/{agent-name}.md` — project-scoped self-learning log for SDD agents.

Byte-level format specs live under `docs/` (`ssq_format.md`, `ssq_mine_chunk_format.md`, `xsb_format.md`, `ultramix_archive_formats.md`). Treat these as reference documents — do not edit them as part of implementation work.

## Commands

```bash
cargo build                              # debug build
cargo build --release                    # optimized
cargo run -- --help                      # run the CLI
cargo test                               # unit + integration tests
cargo test <name>                        # run a single test by name substring
cargo test --test ddr_to_sm5             # run one integration test file
cargo clippy --all-targets -- -D warnings   # lint (must pass clean)
cargo fmt                                # format (must produce no diff)
```

Cross-compile targets (`x86_64-pc-windows-gnu`, `x86_64-unknown-linux-musl`) are documented in `README.md`.

## Code quality rules (production Rust)

These are hard rules for this codebase. The full rationale is in `.spec/steering/rust-cli-standards.md`; the points below are the non-negotiable summary.

- **Lints gate merges.** `cargo fmt` must produce zero diff; `cargo clippy --all-targets -- -D warnings` must pass. If a lint is wrong at a specific site, `#[allow(clippy::name)]` with a one-line comment explaining why — never blanket-allow at crate root.
- **No `unwrap()` / `expect()` in non-test code** unless the invariant is proven by construction, with a comment stating why it cannot fail. `expect()` messages explain *why it's impossible*, not hope.
- **Two-tier errors.** Parser/writer layers use `thiserror` with per-module error enums. CLI/orchestration uses `anyhow::Result<T>` with `.context(...)`. Preserve the source when re-wrapping (`map_err(|e| ...)`, never `map_err(|_| ...)`). Every parse error carries a location — byte offset for binary, line/column for text.
- **Bounds-check before slicing.** Return `UnexpectedEof` instead of letting `&bytes[a..b]` panic.
- **Endianness is explicit.** SSQ and XWB are little-endian everywhere. Use `u32::from_le_bytes` etc.; never rely on host byte order.
- **Strong types at boundaries.** `Format` enum, not `String`; `PathBuf`/`Path`, not string paths. `#[clap(...)]` derive stays in `src/cli/`; semantic validation lives in a dedicated `Cli::validate()`.
- **Ownership defaults.** Take `&str` / `&Path`, not `&String` / `&PathBuf`. Return owned values. No `Rc`/`Arc` without measured need. No `.clone()` to appease the borrow checker — fix the ownership story.
- **No async, no threading.** This is a synchronous, single-threaded CLI. If batch throughput ever matters, `rayon` is the first reach — never `tokio`.
- **Logging goes through `log`**, not `println!`. `println!` is reserved for the CLI's primary output. Level discipline: `error!` = run failed, `warn!` = data dropped but continuing, `info!` = per-file outcomes, `debug!`/`trace!` = diagnostic detail.
- **Never invent a new top-level module.** The categories in `.spec/steering/structure.md` cover every concern this tool has. Format-specific types live in their format module; only format-independent types go in `src/model/`.
- **Never write a point-to-point converter.** Always `source → model → target`.
- **Business rules are law.** The ten rules in `.spec/steering/product.md` (§ Business Rules) — e.g. "DDR_LEGACY is import-only", "modern SSQs use TPS=1000 with chunk types 1/2/3/20 only", "legacy timelines normalize to zero origin" — are correctness invariants, not stylistic preferences.
- **Tests.** Unit tests live beside the code; integration tests in `tests/` named `{from}_to_{to}.rs`. Round-trip tests (parse → write → parse) are the primary pattern for format work. No `unwrap()` in tests that represent real failure modes — use `?` with `fn ... -> Result<(), Box<dyn Error>>`. Golden-file tests are fine for SSC; avoid them for binary formats unless you're committed to reviewing every regeneration diff.
- **Dependencies need justification.** Any new crate is introduced in a feature design doc with a reason. Avoid `*` versions; prefer shallow trees.
- **Doc comments on public items.** `///` on every public item in library modules; `//!` at the top of each `mod.rs` saying what it owns and what it doesn't. If the signature changes, the doc changes in the same commit.

## Anti-patterns to reject

Callouts from `rust-cli-standards.md` that agents have historically tripped on:

- `HashMap<String, Value>` as a parsed representation. Parse into real structs.
- Preserving unknown tags / legacy-only chunks "just in case". Log at `warn`, drop.
- Over-abstracting early (traits for two implementations).
- Regenerating golden fixtures without reviewing the diff.
- Adding backwards-compat shims or feature flags for hypothetical futures.
