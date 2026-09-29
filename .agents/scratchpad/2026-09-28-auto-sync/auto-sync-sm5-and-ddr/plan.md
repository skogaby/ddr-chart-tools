# Plan: auto-sync-sm5-and-ddr

Status: Approved 2026-09-28. This rests on the upstream approvals and the
maintainer's standing approval. Auto mode.

## Unit tests

**`cli`:**

- `auto_sync_bare_flag_applies_with_default_cap`
- `auto_sync_report_mode`
- `auto_sync_max_ms_sets_cap`
- `auto_sync_max_requires_auto_sync`
- `auto_sync_max_out_of_range` (0, 201)
- `auto_sync_rejects_legacy_for_now`
- `auto_sync_absent_by_default`

**`job::sync_offset`, pure `decide`:**

- `apply_moves_and_logs_tail`
- `negative_delta_says_earlier`
- `unchanged_logs_info`
- `report_mode_applies_nothing`
- `report_mode_refusal_is_refused`
- `each_refusal_has_actionable_message_and_key`
- `unmeasured_refusal_omits_measured_keys`

**`sync`:** `refusal_keys_are_stable`.

## Integration tests

Helper: synthetic 150 BPM song over two charts, about 11 s of OGG, with the
audio shifted by `X` ms.

**`tests/sm5_to_ddr.rs`:**

1. `auto_sync_converges`
   - Audio +20 ms.
   - `SM5→DDR --auto-sync`, then that output `DDR→SM5 --auto-sync report`,
     reports |δ| ≤ 1.
2. `auto_sync_is_linear_in_audio_shift`
   - Report-mode `correction_ms` for X = 0 and X = 25 differ by 25 ± 1.
3. `report_mode_writes_the_same_chart`
   - The SSQ is byte-identical to a run without the flag.
4. `bias_composes_with_auto_sync`
   - Every anchor is +10 compared with auto-sync alone.
5. `refusal_keeps_sync_and_succeeds`
   - A chart with only 10 notes: exit 0, a warn line with
     `reason=too_few_events`, and the SSQ identical to a run without the
     flag.

**`tests/ddr_to_sm5.rs`:**

1. `auto_sync_converges`
   - DDR input made by the tool (SM5→DDR, no auto-sync) from audio +20.
   - `DDR→SM5 --auto-sync`, then `SM5→DDR --auto-sync report` on the
     output, reports |δ| ≤ 1.
2. `report_mode_writes_the_same_chart`
   - The SSC is byte-identical.
3. `bias_composes_with_auto_sync`
   - `#OFFSET` is exactly 0.010 s lower.
