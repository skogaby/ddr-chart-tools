# Rust CLI Coding Standards

Conventions for writing Rust code in this project. These are pragmatic defaults drawn from widely-used Rust CLI projects (ripgrep, fd, bat, cargo itself) plus `clap` and `thiserror`/`anyhow` author recommendations. They apply to all Rust code in this repository.

## Toolchain

- **Edition**: 2021.
- **Minimum supported Rust version (MSRV)**: latest stable at time of each release. This is a hobby project — no need to support old compilers.
- **Toolchain file**: optional `rust-toolchain.toml` pinning stable. Add it when CI is set up.

## Formatting and Lint Gates

- `cargo fmt` must produce no changes. Use default `rustfmt` settings unless there's a concrete reason to override.
- `cargo clippy --all-targets -- -D warnings` must pass. Lints are denied, not warned.
- If a clippy lint is wrong for a specific site, `#[allow(clippy::name)]` with a one-line comment explaining why. Don't blanket-allow at crate root.

## Module Layout

- Prefer `src/foo/mod.rs` over a flat `src/foo.rs` once a module has more than one submodule or more than ~200 lines. Start flat, split when it starts to strain.
- Each module declares its own `Error` type. Cross-module error conversion happens via `From` impls at the boundary, not by leaking one module's error type into another.
- Keep `main.rs` thin — parse args, set up logging, call into `lib.rs` / the top-level orchestration function, translate result into a process exit code.

## Error Handling

### Two-tier pattern

- **Library / parser / writer code**: typed errors via `thiserror`. Callers need to branch on error kind.
- **Application / CLI / orchestration code**: `anyhow::Result<T>` with `.context(...)` for human-readable chains.

### Rules

- **No `unwrap()` or `expect()` in non-test code** except where the invariant is proven by construction (and then write a comment explaining it).
- **No `.unwrap()` in tests either** unless the test will fail meaningfully if it panics. Prefer `?` in test functions that return `Result<(), _>`.
- **`expect()` is for proven-impossible cases**, not "I hope this works." The message should say *why* it can't fail.
- **Always preserve the source when re-wrapping an error**: `.map_err(|e| MyError::Foo(e))` not `.map_err(|_| MyError::Foo)`.
- **Parse errors include location**. Byte offset for binary formats, line/column for text formats. An error without a location is a bug.

### Defining errors

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SsqError {
    #[error("unexpected end of file at byte {offset}")]
    UnexpectedEof { offset: u64 },

    #[error("unknown chunk type {ty} at byte {offset}")]
    UnknownChunk { ty: u16, offset: u64 },

    #[error("I/O error")]
    Io(#[from] std::io::Error),
}
```

### Using errors in the CLI

```rust
use anyhow::{Context, Result};

fn main() -> Result<()> {
    let args = Cli::parse();
    run(args).context("ddr-chart-tools failed")
}

fn run(args: Cli) -> Result<()> {
    let bytes = std::fs::read(&args.input)
        .with_context(|| format!("reading {}", args.input.display()))?;
    let parsed = ssq::parse(&bytes).context("parsing SSQ")?;
    // ...
    Ok(())
}
```

## CLI Design (clap)

- Use the **derive API** (`#[derive(Parser)]`). Builder API only when the derive can't express what's needed.
- Put the `Cli` struct in `src/cli/mod.rs`. Don't spread `#[arg]` attributes across multiple files.
- Use `ArgGroup` and `required_if_eq` for conditional arg validation (e.g. `--chartfile` required when `--input-folder` is absent).
- Validate semantic rules (e.g. "can't set `--to-format DDR_LEGACY`") in a dedicated `validate()` method on `Cli`, not scattered throughout the enum. Return typed errors, map to `clap::Error` at the boundary.
- Help text on every flag. Every flag. The help text is user-facing documentation.
- Long-form flag names use `kebab-case`: `--from-format`, `--input-folder`. Short flags are optional and only for commonly-typed args.

## Ownership, Borrowing, References

- **Take `&str`, not `&String`**. Take `&Path`, not `&PathBuf`.
- **Return owned types by default**, not references. References in return position are for clear lifetime stories (e.g. iterator methods).
- **`impl AsRef<Path>`** for public functions that accept paths — lets callers pass `&str`, `&Path`, `PathBuf`, `&PathBuf` without converting.
- **Avoid `Rc`/`Arc` unless you've measured a cloning cost**. In a CLI tool with no threading, plain ownership and borrowing are almost always enough.
- **No lifetimes in public APIs unless necessary**. If adding `'a` makes the signature harder to read, prefer owning the data.

## Strings

- **`String` for owned, `&str` for borrowed**. `&String` is almost always wrong.
- **No `String + &str` concatenation in hot paths** — use `format!` or `write!` into a buffer.
- **UTF-8 assumptions**: SSC and SM are documented as ASCII/UTF-8. If a file isn't valid UTF-8, return a typed error; don't silently lossy-decode.
- **Paths are not strings**. Use `Path`/`PathBuf` for filesystem paths and convert only at the I/O boundary or for display.

## Binary I/O (SSQ, XWB)

- **Little-endian everywhere** (SSQ spec). Use `i32::from_le_bytes`, `u16::from_le_bytes`, etc. No `ByteOrder::read_*` needed unless you want the trait-based API.
- **Buffered reads**. Wrap `File` in `BufReader`. Don't read one byte at a time from an unbuffered file.
- **Track offsets explicitly**. Errors need byte offsets; if the parser doesn't track where it is, it can't report useful errors.
- **Bounds-check before slicing**. `&bytes[offset..offset + len]` panics if `len` overruns. Return `UnexpectedEof` instead.

## Text I/O (SSC, SM)

- **Stream where possible**, but loading whole files into memory is fine for simfiles (they're small).
- **Tokenize, then parse**. MSD format is `#TAG:VALUE;` — build a tokenizer first, then a higher-level parser that consumes tokens. Don't grep for tags with regex.
- **Preserve unknown tags**? No — log them at `warn` and drop. We're a conversion tool, not a preservation tool.

## Logging

- Use the `log` facade in library code: `log::info!`, `log::warn!`, `log::debug!`, `log::trace!`.
- Set up the subscriber (e.g. `env_logger`) exactly once in `main.rs`.
- **Levels**:
  - `error!`: the run failed.
  - `warn!`: data was dropped, something unexpected happened, but the run continues.
  - `info!`: per-file outcomes in batch mode, summary counts.
  - `debug!`: per-chunk parsing, field values, chosen code paths.
  - `trace!`: byte-level detail, loop iterations.
- **Don't println!** in library code. println is for the CLI's primary output (the result), not diagnostic info.

## Testing

### Structure
- **Unit tests**: `#[cfg(test)] mod tests { ... }` in the same file as the code they test.
- **Integration tests**: `tests/*.rs`, one file per conversion direction.
- **Fixtures**: `tests/fixtures/` for small real-world files. Keep them small (< 100KB each) and license-clean.

### Conventions
- Test names describe the scenario: `fn parse_ssq_with_empty_events_chunk()`, not `fn test1()`.
- Use `assert_eq!` with good messages; or `pretty_assertions::assert_eq!` once fixtures grow.
- **No `unwrap()` in tests that represent real failure modes** — use `?` with `fn foo() -> Result<(), Box<dyn Error>>`.
- **Round-trip tests** are the primary integration test pattern: parse → write → parse → assert equal.
- **Golden-file tests** are acceptable for text formats (SSC) where byte-exact output is meaningful.
- **Don't golden-file binary formats** unless you're willing to regenerate the golden when the writer legitimately changes.

### What to test
- Every public function in a format parser/writer.
- Every business rule from `product.md` has at least one test.
- Error cases (malformed input, unsupported format combos) get tests too — the error type matters.

## Async, Threading, Concurrency

- **Don't.** This is a single-threaded CLI. Batch processing is sequential. If throughput becomes a problem later, add `rayon` for parallel file processing in batch mode — not before.
- **No `tokio`, no async runtime.** Nothing in this tool benefits from async.

## Dependencies

- **Every new dep needs justification** in the design doc of the feature that adds it. "Convenience" isn't enough; what specifically does it give us?
- **Prefer crates with low tree depth** and active maintenance. Check `cargo tree` after adding a dep.
- **Avoid `*`-version deps**. Pin with `"1"`, `"1.2"`, or `"1.2.3"` depending on how much flexibility makes sense.
- **Audit transitive deps for size** if adding a crate balloons `cargo tree` output significantly. We're a CLI, not a framework — we should have a small dep tree.

## Documentation

- **`///` doc comments on every public item** in library modules. Include at least one example where the usage isn't obvious.
- **Module-level `//!` comments** at the top of each `mod.rs` explaining what the module owns and what it doesn't.
- **No out-of-date doc comments**. If the signature changes, the doc comment changes. Agents reading old comments produce wrong code.
- **Link to `docs/ssq_format.md` section numbers** in SSQ parser comments. The spec is the source of truth; the code just implements it.

## Commits and PRs

(Hobby project, no formal review gates — but even solo, some discipline helps.)

- One logical change per commit. Parser + writer + CLI wiring can all be one commit for a new format, but unrelated refactors are separate.
- Commit message first line < 72 chars, imperative mood ("Add SSQ tempo chunk parser", not "Added" or "Adds").
- Run `cargo fmt` and `cargo clippy -- -D warnings` before committing.
- Run `cargo test` before pushing.

## Anti-Patterns

- ❌ `unwrap()` in production code without a comment proving it can't fail.
- ❌ Stringly-typed enums (`format: String` instead of `format: Format`).
- ❌ `HashMap<String, Value>` as a parsed-file representation. Parse into a real struct.
- ❌ Silent failure. If something goes wrong, log it or return an error.
- ❌ `clone()` sprinkled to shut up the borrow checker. Figure out the ownership story.
- ❌ Over-abstracting early. Two formats don't need a `Format` trait; three formats might.
- ❌ Adding `async` to an otherwise-synchronous call chain. Don't.
- ❌ Regenerating golden files without reviewing the diff. Goldens exist to catch regressions; if you regenerate on every failure, they don't.
