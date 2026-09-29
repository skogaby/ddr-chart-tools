# Progress: onset-envelope

## Checklist

- [ ] Dep + tests (red)
- [ ] Envelope (green)
- [ ] tech.md
- [ ] Validate
- [ ] Commit

## Log
- [x] all

1. Red: compile errors for missing `Envelope`/`envelope`.
2. Green: 5 tests pass (debug-build runtime 1.3 s).
3. Clippy: `neg_cmp_op_on_partial_ord` fixed with explicit `is_finite`. Unused-in-lib `onset` module carries a temporary `#[allow(dead_code)]`, removed by the estimator task.
4. `tech.md` row added. fmt, clippy and test clean (415 passed).

## Deviations

- A temporary `#[allow(dead_code)]` on `mod onset` keeps this commit lint-clean before its consumer exists.

Status: Complete 96209ae
