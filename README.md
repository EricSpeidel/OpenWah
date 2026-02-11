# OpenWah

A Rust desktop app that turns any short sound clip into a playable piano-like instrument.

## What it does

1. Opens a user-selected audio file (common formats supported through Symphonia codec features).
2. Decodes and trims/pads the clip to about **1 second** to create a base note.
3. Maps that base note to a two-octave+ piano keyboard (C3–C5), pitch-shifting per key so you can "play piano" with your own sample.

## Run

```bash
cargo run
```

Use **Open Sound Clip...** to load audio, then click the on-screen keys (or use A/W/S/E... keyboard mapping around middle C).
