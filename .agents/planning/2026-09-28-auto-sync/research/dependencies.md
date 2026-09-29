# Dependencies

Backs D12. Proposed addition to `Cargo.toml`: `rustfft = "6"`.

## `rustfft` 6.4.1

- Pure Rust, MIT/Apache-2.0, ~30M downloads, last release 2025-09
  (<https://crates.io/crates/rustfft>, <https://github.com/ejmahler/RustFFT>).
- Runtime-detected SIMD: AVX on x86_64, NEON on aarch64, scalar fallback.
- New crates it adds to `Cargo.lock` (none present today), from `cargo tree`
  in the prototype:

  ```
  rustfft v6.4.1
  ├── num-complex v0.4.6
  │   └── num-traits v0.2.19
  │       [build-dependencies]
  │       └── autocfg v1.5.1
  ├── num-integer v0.1.47
  ├── num-traits v0.2.19
  ├── primal-check v0.3.4
  ├── strength_reduce v0.2.4
  └── transpose v0.2.3
  ```

  Eight crates, all pure Rust. The only build script is `num-traits`' use of
  `autocfg` (rustc version probing); nothing links C code.
- Clean release build in the prototype; about 2 s of the build time is
  `rustfft` itself.

## Cross-compilation

The `x86_64-pc-windows-gnu` and `x86_64-unknown-linux-musl` targets are not
installed on the development machine, so cross-builds were not run. Risk is
low: no C code or platform-specific APIs; SIMD paths are selected at runtime
behind `cfg(target_arch)`. The plan should include one cross `cargo build`
as a check.

## Not needed

- `realfft` (real-input wrapper, ~2× faster): the envelope costs about 150 ms
  per song single-threaded with complex FFTs, well inside budget. Not worth a
  ninth crate.
- No resampling, windowing, or statistics crates. Hann windows, medians, and
  parabolic interpolation are a few lines each.
