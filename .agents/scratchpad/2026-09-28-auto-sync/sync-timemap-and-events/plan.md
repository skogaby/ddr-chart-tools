# Plan: sync-timemap-and-events

Status: Approved 2026-09-28. This rests on the upstream approvals (the
task was generated from the approved plan and design) and on the
maintainer's standing approval. Auto mode.

## Test scenarios

### `timemap.rs`

1. **`pairs_single_bpm_interpolates`**
   - `[(0,0),(4096,2000)]` @1000: beat 2.0 → 1.0 s; beat 0 → 0; beat 4
     → 2.0.
2. **`pairs_stop_note_on_stop_takes_stop_start`**
   - `[(0,0),(8192,4000),(8192,4500),(12288,6500)]`: beat 8 → 4.0; beat 9
     → 5.0 (4.5 + 0.5).
3. **`pairs_extrapolate_before_and_after`**
   - Same pairs as scenario 2: beat −1 → −0.5; beat 13 → 7.0 (last-segment
     slope 0.5 s/beat); last segment non-stop.
4. **`pairs_extrapolate_past_trailing_stop`**
   - `[(0,0),(4096,2000),(4096,2500)]`: beat 5 → 3.0 (slope from the last
     nonzero-span segment, applied from the last anchor).
5. **`pairs_tps_scales_seconds`**
   - `[(0,0),(4096,300)]` @150: beat 4 → 2.0.
6. **`pairs_reject_malformed`**
   - Single pair; TPS 0; decreasing ticks; all pairs at one tick → `None`.
7. **`song_extrapolates_with_final_bpm`**
   - Song 160 BPM @0, 120 BPM @16: beat 40 → 16·0.375 + 24·0.5 = 18.0 s,
     not 15.0.
8. **`song_matches_synthesized_pairs`**
   - 160@0, 120@16, a 0.5 s stop @24, offset 0.02 s. Pairs from
     `synthesize_tempo_entries_until(song, Some(beat 44))`. At every
     integer beat 0..=44, `|from_song − from_tempo_pairs| ≤ 1 ms`.
9. **`song_first_bpm_applies_from_beat_zero`**
   - The only segment is at beat 2 @120: beat 0 → offset; beat 2 → +1.0 s.
10. **`song_rejects_no_segments_and_nonpositive_bpm`**

### `events.rs`

11. **`weights_count_charts_not_notes`**
    - Chart A: tap @4, hold @4 (other panel), tap @6. Chart B: tap @4,
      mine @5. Result: [(4, w2), (6, w1)], sorted, with times from the map.
12. **`mines_only_chart_contributes_nothing`**
13. **`shocks_count`**

All fail to compile before the implementation exists.

## Implementation

- `TimeMap { anchors: Vec<(f64, f64)> }`.
- `seconds_at` lookup:
  - `k = partition_point(beat < b)`;
  - `k < n && anchors[k].beat == b` → `anchors[k].seconds`;
  - `0 < k < n` → interpolate between `k−1` and `k` (their beats differ,
    since `anchors[k−1].beat < b < anchors[k].beat`);
  - `k == 0` → extrapolate from `anchors[0]` with the first nonzero-span
    slope;
  - `k == n` → from `anchors[n−1]` with the last nonzero-span slope.
- **Slopes** are precomputed at construction as `first_slope` and
  `last_slope`, so a map always has one; the constructors return `None`
  otherwise.
- **`chart_events`:**
  - per chart, collect a `BTreeSet<Beat>` of counted notes;
  - merge into a `BTreeMap<Beat, u32>`;
  - map each entry to `SyncEvent`.
