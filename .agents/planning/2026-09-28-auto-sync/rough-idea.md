# Rough Idea: Auto-sync charts to audio during conversion

Captured 2026-09-28 from the maintainer.

Add an optional flag that auto-syncs the output chart to its audio when
converting from any format to modern DDR format.

Today the tool relies solely on the sync cues in the input — `#OFFSET` from a
StepMania simfile, or the tempo chunk of a legacy SSQ. Some subset of songs
(especially from legacy DDR titles) are inherently off-sync. The current
workaround is:

1. Convert the song to StepMania format.
2. Open it in ArrowVortex and nudge the offset until the chart and audio line up.
3. Export a new StepMania chart.
4. Convert the re-synced chart to DDR format.

The goal is to skip that middle-man process and have the tool intelligently
sync the audio and chart itself, so DDR charts come out perfectly synced
without manual fine-tuning.

Initial idea for the mechanism: do waveform analysis against the output
steps, then align the chart's sync to the nearest beat in the waveform to the
beginning of the chart timeline. The expected error is not more than about
150–200 ms in either direction.

Constraints stated:

- Opt-in flag, not default behavior.
