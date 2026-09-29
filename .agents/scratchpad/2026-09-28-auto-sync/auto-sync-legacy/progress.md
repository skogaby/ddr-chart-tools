# Progress: auto-sync-legacy

## Checklist

- [ ] Tests (red)
- [ ] Wiring (green)
- [ ] README
- [ ] Validate
- [ ] Commit

## Log
- [x] all

1. **Red:** all 8 legacy integration tests failed with the CLI's
   "does not support DDR_LEGACY yet" error, and the CLI acceptance test
   failed.
2. **Green.** Wired `legacy_to_ddr`, which decodes for analysis and reuses
   that decode for re-encoding, with a warn-and-skip fallback. Wired
   `legacy_to_sm5`, and removed the rejection. All 8 now pass.
   - The test-only WAVM encoder round-trips through `src/wavm`:
     convergence and linearity pass on WAVM audio.
3. **README:** the legacy limitation note is replaced by the passthrough
   behavior.
4. **Gates:** fmt and clippy clean; `cargo test` passes (443 unit and 16
   integration tests) in about 30 s.

Status: Complete c8ba8d5
