# Contributing

Open an issue describing the observed behavior, macOS version, architecture, app version/commit, requested capture format, blur state, and whether the physical or virtual camera was selected. Remove private paths, serials, and camera content first.

Keep changes scoped. Run `cargo fmt --check`, `cargo test --locked`, and `cargo clippy --all-targets --locked -- -D warnings` on macOS. Hardware tests are opt-in; label synthetic, native-consumer, and real call-app evidence separately. Report new-frame cadence and stalls rather than transport FPS alone.

Preserve bounded queues, privacy generation fences, manual privacy ownership, and unknown-usage handling. Do not add cloud uploads, accounts, or telemetry. Never run factory reset or overwrite an existing startup position as a routine test. Preserve the user's camera and capture preferences.

Do not commit vendor binaries, reverse-engineering dumps, camera frames, captures, device serials, credentials, or local diagnostic reports. Contributions are accepted under the project's MIT License.
