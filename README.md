# FTHRust

FTHRust is a from-scratch Rust rewrite of the Linux side of FTHR Clips, a
local-first instant-replay recorder. It captures the screen into a rolling
buffer, a hotkey saves the last N seconds as a clip, and a built-in library and
editor let the user trim, export and optionally upload clips. No account, no
cloud, no telemetry.

Design: [docs/superpowers/specs/2026-10-22-fthrust-design.md](docs/superpowers/specs/2026-10-22-fthrust-design.md)

Derived from FTHR Clips by FTHR-Community, GPL-3.0-only.
