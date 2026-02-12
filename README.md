# OpenWah

A Rust desktop app that turns any short sound clip into a playable piano instrument.

## What it does

1. Opens a user-selected audio file (common formats supported via Symphonia).
2. Decodes and trims/pads the clip to about **1 second** to create a base note.
3. Maps that base note across a piano layout (C3–C5), pitch-shifting each key by semitone distance.
4. Lets you play notes by clicking a normal piano-style keyboard layout (black keys over white keys).

## Run

```bash
cargo run
```

In the app:
- Click **Open Sound Clip...** and choose any clip.
- Click keys on the piano.
- Or use keyboard shortcuts near middle C: `A W S E D F T G Y H U J K`.

## Windows support

This project is Windows-compatible and checks cleanly for both common 64-bit Windows Rust targets:

- `x86_64-pc-windows-gnu`
- `x86_64-pc-windows-msvc`

No ALSA/Linux-specific system packages are required on Windows.

## Linux note

On Linux, audio playback via `rodio/cpal` may require ALSA development libraries (`alsa` / `alsa-lib` package family) to be installed.
