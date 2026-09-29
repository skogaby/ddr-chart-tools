# Plan: auto-sync-legacy

Status: Approved 2026-09-28. This rests on the upstream approvals and the
maintainer's standing approval. Auto mode.

## Tests

**CLI:** `auto_sync_accepts_legacy_input`, replacing
`auto_sync_rejects_legacy_for_now`.

**`tests/ddr_legacy_to_ddr.rs`:**

- `auto_sync_converges_on_wavm_audio`
- `auto_sync_is_linear_in_audio_shift`
- `report_mode_writes_the_same_chart`
- `passthrough_audio_is_untouched_while_the_chart_moves`
- `undecodable_passthrough_audio_skips_auto_sync`

**`tests/ddr_legacy_to_sm5.rs`:**

- `auto_sync_converges_on_wavm_audio`
- `report_mode_writes_the_same_chart`
- `bias_composes_with_auto_sync`

## Red

All the legacy integration tests fail today: the CLI rejects `--auto-sync`
with `DDR_LEGACY` (exit 2).
