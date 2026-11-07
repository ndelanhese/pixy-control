# Changelog

## Unreleased

- Isolate physical-camera processing from the AVFoundation callback using a bounded latest-frame mailbox and preserve delivery QoS; three repeated 4K/strong-blur/hidden-window cadence tests passed.

- Use bounded native live-call framing and blur with asynchronous person segmentation when the local window is inactive. Preserve full-resolution photos and recordings.
- Establish native video priority before transport setup, preserve capture age, and bound the downstream queue.
- Reduce active-video metadata reconnections while keeping idle wake detection fast.
- Keep background hardware refresh to a fresh privacy-mode read instead of repeatedly querying static USB capabilities.
- Write optional diagnostic reports through a bounded nonblocking worker.
- Add a real-pixel virtual-camera cadence diagnostic that detects repeated images.
- Document measured delivery cadence and call-app coverage limits, including unmeasured native Teams fluency.

- Keep virtual output, local blur and automatic privacy active after closing the window; reopen or quit from the macOS menu bar.
- Add an event-driven USB launcher and per-user login installation, with removal script.
- Skip hidden-window layout and texture work.

- Initial public source for the native Rust PIXY controller.
- Local camera controls, framing, presets, photos, and segmented hardware video recording.
- Local person-segmented background blur.
- Persistent virtual output with independent conversion/publication and bounded buffering.
- Idle privacy with previous-mode restoration and a separate manual privacy lock.
- Reduced background rendering, immutable pixel reuse, and no-consumer output work.

Hardware and call-app compatibility are still being validated. See the README for current limits.
