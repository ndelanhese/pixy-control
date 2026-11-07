# Pixy Control

A local macOS camera controller written in Rust for the **EMEET PIXY**. Configure the camera, frame your video, blur your background, and send the processed image to other apps through an already installed EMEET virtual camera extension.

No account, cloud processing, network service, or telemetry. Person segmentation runs on the Mac with Apple Vision; full-resolution effects use Core Image. This is an independent project, not affiliated with or endorsed by EMEET.

**Early development:** tested with one PIXY on Apple Silicon. Hardware compatibility and call-app behavior need validation on additional machines. This repository contains original application source; it does not include EMEET installers, binaries, SDKs, firmware, or its camera extension.

[Guia em português](docs/USO.pt-BR.md) · [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md)

## Features

| Area | Controls |
| --- | --- |
| Camera | Manual, firmware tracking, privacy, pan/tilt, eight presets, startup position |
| Image | Focus, exposure, white balance, brightness, contrast, saturation, sharpness, anti-flicker |
| Framing | Digital zoom, crop, rotation, mirror, aspect ratio, four-corner perspective |
| Effects | Adjustable local background blur with person segmentation |
| Capture | Live preview, PNG photos, hardware-encoded H.264 video, optional microphone, recording segments |
| Sharing | Processed output through the installed EMEET virtual camera, persistent pause/resume session |
| Privacy | Idle detection, return to previous Manual/Tracking mode, manual privacy lock |

Availability depends on the firmware and video mode. Firmware tracking and gestures run locally on the camera. Beauty effects, cloud features, streaming platforms, and accounts from the vendor app are excluded.

## Requirements

- Apple Silicon Mac; macOS 14 or later APIs. Current runtime testing was on macOS 27.
- EMEET PIXY USB device `328f:00c0`. Other EMEET models are not selected automatically.
- Rust 1.97.0 (pinned in `rust-toolchain.toml`), Cargo, and Xcode Command Line Tools.
- FFmpeg on `PATH` or at `/opt/homebrew/bin/ffmpeg` for recording. Preview and controls do not require FFmpeg.
- **For virtual output only:** the EMEET STUDIO camera extension must already be installed and approved through the vendor's normal macOS installation flow. Pixy Control does not install or distribute it.

Close EMEET Studio while using Pixy Control; the camera's HID interface can be exclusive.

## Build and run

```sh
git clone https://github.com/ndelanhese/pixy-control.git
cd pixy-control
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
./build-app.sh "$PWD/dist/Pixy Control.app"
open "dist/Pixy Control.app"
```

Build the `.app` and launch it normally so macOS can show camera and microphone permission requests. The bundle is signed ad hoc for local use, not Developer ID signed or notarized. No prebuilt release is promised by this repository.

## Teams, Meet, and other apps

1. Leave Pixy Control running and enable **V-Cam → Ativar câmera virtual**.
2. In the call app, select **EMEET STUDIO Virtual Camera**.
3. Configure framing and **Efeitos → Desfocar fundo** in Pixy Control.

Selecting the physical **EMEET PIXY** bypasses the software crop and blur. Hardware controls still go directly to the camera.

The installed extension accepts **1920×1080 at 30 fps**. Selecting 4K in Pixy Control sets its physical capture format, not the format requested by every other application. Call apps may scale or compress the video independently. A 30 fps transport does not by itself prove 30 different camera images. Consumer negotiation, camera exposure, and system load can affect delivered motion.

```mermaid
flowchart LR
  Camera[PIXY USB] --> Capture[AVFoundation]
  Capture --> Process[Local blur and framing]
  Process --> Preview[Preview / photo / recording]
  Process --> Output[Bounded conversion and publication]
  Output --> Extension[Installed EMEET extension]
  Extension --> Apps[Call apps]
```

### Automatic privacy

Enable **V-Cam → Privacidade automática**. A visible foreground preview and recording count as use. After confirmed idle, the app enters firmware Privacy and remembers Manual/Tracking. A later consumer wakes the camera and restores that mode. Brief client renegotiations have a hold period before USB capture is released.

Manual privacy remains locked until you explicitly choose Manual or Tracking. Unknown usage is never treated as confirmed idle. Automatic privacy requires Pixy Control to remain running; this is not an OS-wide guarantee after quitting or crashing. Closing the main window keeps the service running. Use **Pixy** in the macOS menu bar to reopen the window or explicitly quit.

### Background and USB startup

The red close button hides the window while virtual output, local blur, recording and automatic privacy continue. The menu bar **Pixy → Abrir Pixy Control** restores it; **Encerrar Pixy Control** stops the service.

To install the app in `~/Applications` and enable automatic startup:

```sh
zsh install-background.sh "$PWD/dist/Pixy Control.app"
```

A per-user LaunchAgent starts a small IOKit observer at login. It launches Pixy Control hidden when USB device `328f:00c0` appears, including if already connected at login. It sleeps awaiting USB events; it does not poll or capture video. Quitting remains effective until the next connection/login; it does not repeatedly relaunch the main app. Login Items must remain allowed by macOS.

Disable automatic startup with `zsh install-background.sh --uninstall`. This removes only this project's LaunchAgent and leaves settings and the application intact. The observer runs only in the logged-in user's session.

### Performance

The current capture-worker change moves physical-camera image processing out of the AVFoundation delivery callback. It keeps one retained in-flight sample and one replaceable pending sample, with receipt timestamps preserved and explicit buffer release at shutdown. Automated tests cover a blocked processor and bounded frame ownership. **Hardware validation of this change is pending camera consent for the rebuilt local app; it is not yet a verified fix for call-app stutters.** Rebuilding an ad hoc signed app can require fresh macOS camera permission.

**Known limitation:** intermittent virtual-output stalls remain on the test machine with blur and a hidden window. Recent repeated 60-second measurements did not consistently meet the project's 28.5 distinct fps / 100 ms worst-gap target. These optimizations reduce processing cost; they do not establish parity with the preview or a verified remote Teams/Meet call. The installed vendor extension does not notify changes to its source-client counter, so automatic privacy currently still needs bounded fresh metadata queries.

Capture callbacks and the CMIO publisher establish their A/V priority before opening their native queues, including background starts. A latency-critical activity is scoped to camera capture and ends with it. Conversion runs separately from the fixed 30 Hz output clock. A bounded two-frame reserve absorbs conversion jitter. Pixel buffers and the video format description are reused; repeated/neutral frames reuse immutable pixels. The metadata worker runs at Utility priority; its short-lived CMIO reader uses UserInitiated priority. Refreshes are shared and less frequent during active video, avoiding repeated CMIO client connections. Idle polling stays fast; automatic privacy still requires confirmed idle time, so detection and the privacy grace can take several seconds after a call ends. Without consumers, conversion and publication stop; after five confirmed idle seconds, the producer transport is suspended and resumes when usage returns. Connected clients keep the same active session through capture pauses. Background/minimized UI work is reduced. Periodic background hardware refresh reads the privacy mode only; full UVC ranges, firmware, audio and motor snapshots resume when the controller is active. Explicit commands still receive full readback.

For an upright rectangular view with the window hidden or unfocused and no recording active, live framing samples the configured zoom/pan directly from the capture buffer into a bounded 1080p canvas. Person segmentation runs on a separate worker, with a single latest input and expiring masks. Late/missing masks temporarily blur the full image; they never block frame delivery or expose unprocessed input. Native CPU filtering avoids synchronously waiting for GPU/segmentation work on each delivered frame. Photos and recordings still require a full-resolution frame. Rotation, vertical flip and perspective correction retain their original processing path. These paths can have different resource costs. Hardware encoding requires VideoToolbox; recording reports an error instead of silently falling back to a slower software encoder.

## Data and privacy

Settings and preset thumbnails are stored under `~/Library/Application Support/Pixy Control`. Captures default to `~/Movies/Pixy Control`. The app has no cloud upload or remote telemetry code; your chosen call app controls any transmission of its camera input.

Do not attach camera photos, recordings, serial numbers, personal paths, or unredacted diagnostic dumps to public issues.

## Development and validation

```sh
cargo fmt --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
```

Unit tests exercise protocol parsing, transforms, local composition, privacy ownership, bounded queues, output timing, and cancellation. They do not prove real camera behavior or successful Teams/Meet calls. CI does not open a physical camera, move motors, or reset firmware.

Explicit hardware/virtual diagnostics are documented in the [Portuguese guide](docs/USO.pt-BR.md). Run synthetic virtual tests only without external clients: their test patterns appear on the virtual output. Hardware diagnostics can change camera controls; read their scope before running. Factory reset requires a separate confirmation in the application.

## License

Original project code is licensed under the [MIT License](LICENSE). Dependencies retain their own licenses. EMEET product names and trademarks belong to their owners; the vendor extension is a separate prerequisite and is not covered by this project's license.

### Measuring delivered video

For a local diagnostic consumer, run the bundled executable with
`--measure-virtual /absolute/path/report.json`. The report measures 60 seconds
of received images after warm-up, including repeated frames and long gaps;
it does not save camera images. Native transport sample rate alone is not a
motion-quality measurement.

A producer started with `--trace-camera-frames` adds a small, CRC-checked frame
identifier to its virtual output for end-to-end diagnostics. This is visible to
all virtual-camera clients while that diagnostic instance runs. It is disabled
in normal launches and does not alter photos or recordings. Optional
`--sharing-report /absolute/path/report.json` writes bounded processing and
publication samples asynchronously. Keep diagnostic reports private: they can
include local settings and paths. Diagnostic consumers do not verify a remote
Teams/Meet participant's network or received call quality.
