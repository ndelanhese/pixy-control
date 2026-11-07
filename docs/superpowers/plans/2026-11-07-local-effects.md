# Local camera effects implementation plan

Use the executing-plans workflow within this session.

## Goal

Add local gesture reactions, solid virtual backgrounds, studio lighting and screen edge lighting to the existing Rust camera pipeline. Keep capture, processing and inference bounded while the window is hidden.

## Architecture

Vision hand detection runs at most five times per second on a separate lower-priority worker with one queued 480px input. Core Video owns inference input allocations until released. A sustained-pose gate emits one reaction until release, with a three-second cooldown. The existing person mask supplies blur, background color and lighting. Animated overlays are composed after segmentation and before each existing output.

## Constraints

- Reuse the existing AVFoundation, Vision, vImage and Core Image integration.
- Keep original settings and presets readable with effects disabled by default.
- Preserve physical 4K capture and existing privacy behavior.
- Keep photos, recordings, preview and virtual output consistent.
- Measure distinct camera images through CRC source markers.

## Tasks

- [x] Exercise sustained-pose and stale-observation gate tests before implementation.
- [x] Implement eight reactions and a bounded hand-pose worker in src/reactions.rs.
- [x] Verify Vision classification with real hand photo fixtures and negative poses.
- [x] Add persistent effects to src/transform.rs and controls in src/ui_sharing.rs.
- [x] Reuse segmentation in src/live_blur.rs and src/background_blur.rs.
- [x] Add manual reaction buttons to the app and macOS status menu.
- [x] Verify color/mask composition, legacy settings, portrait overlays and privacy reset.
- [x] Build, sign and install the release.
- [x] Exercise Meet call and native Teams preview with strong blur and hidden window.
- [x] Run repeated 60-second cadence trials and sample resources during active reactions.
- [x] Document measured coverage and remaining limitations in docs/LOCAL-EFFECTS.md.

## Review focus

Check ownership of retained native pixel buffers, disabled-menu validation, stale gesture/mask state across privacy, matching full-resolution/live lighting, and performance when animations overlap strong blur.

The acceptance measurements and remaining all-effects outlier are recorded in [Local camera effects](../../LOCAL-EFFECTS.md).
