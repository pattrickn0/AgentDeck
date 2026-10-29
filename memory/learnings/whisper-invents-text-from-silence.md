---
title: Whisper fed silence invents a short phrase instead of returning nothing
summary: A dead or wrong microphone made dictation type "e aí"; transcribe_pcm16k now refuses recordings that peak below -40 dBFS.
touches: [crates/core/src/whisper.rs, crates/app/src/audio.rs]
---
Whisper does not report silence reliably: given near-silent audio it often outputs a short filler phrase in the
configured language ("e aí", "Thank you.", "Obrigado."), and `[BLANK_AUDIO]` is not guaranteed. On openSUSE the
default input was a USB adapter with nothing plugged in (peak 33 of 32768, about -76 dBFS), and every dictation typed
"e aí" into the terminal.

**Why:** found on the first Linux run (2026-10-29). cpal captured correctly; the input itself was silent.
**How to apply:** `whisper::is_silence` (peak below `SILENCE_PEAK`, 0.01) stops the transcription with a message
pointing at the system's input device. Do not lower the threshold to rescue quiet audio: a wrong device is the usual
cause, and a made-up sentence typed into an agent's prompt is worse than an error. To check a machine's input without
the app: `pw-record --rate 16000 --channels 1 --format s16 /tmp/m.wav` while speaking, then look at the peak.
