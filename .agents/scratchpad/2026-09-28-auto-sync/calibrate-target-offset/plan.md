# Plan: calibrate-target-offset

Status: Approved 2026-09-28. This rests on the upstream approvals and the
maintainer's standing approval. Auto mode.

## Test

`calibrate_target_offset_against_stock_catalogue` (ignored):

1. Skip if either env var is unset.
2. Measure the corpus.
3. Print the report.
4. Assert:
   - `n ≥ 500`;
   - `|T − TARGET| ≤ 0.1`;
   - within 1 ms ≥ 90%;
   - within 2 ms ≥ 96%;
   - acceptance ≥ 93%.

Red/green: first run against −2.10. If the measured T differs by more than
0.1, update the constant and re-run.

## Parity gate

Within 1 ms ≥ 92% on this subset, compared with the prototype's 94.0%.
