# Local camera effects

## Controls

Open **Efeitos** in Pixy Control.

- **Desfocar fundo** sets person-aware background blur.
- **Iluminação de estúdio** brightens the person and dims the background.
- **Fundo virtual** selects Original, Grafite, Azul or Verde. A solid background replaces blur while selected.
- **Luz de borda da tela** creates a static, click-through white border on the main screen while capture is active. It turns off with capture and on exit.
- **Reações por gestos** enables local hand-pose recognition.
- **Enviar reação** triggers an animation manually. The macOS **Pixy** status menu offers the same eight choices with the main window hidden.

New effects default to disabled when loading older settings. Camera-control gestures implemented in PIXY firmware have their own controls.

| Gesture | Reaction |
| --- | --- |
| Two hands forming a heart | Hearts |
| One thumb up | Thumbs-up |
| One thumb down | Thumbs-down |
| One victory sign | Balloons |
| Two victory signs | Confetti |
| Two thumbs up | Fireworks |
| Two thumbs down | Rain |
| Two rock signs | Lasers |

Keep the hands visible and away from the face. Hold the pose for roughly half a second and release before repeating. A three-second cooldown limits accidental repeat reactions. Low-confidence or late observations are discarded.

## Processing

The Rust app copies at most five reduced BGRA inputs per second to a separate Apple Vision worker. The longest dimension is 480 pixels. There is one pending inference input and one result slot. Core Video owns each copied allocation until its pixel buffer is released. Inference has Utility priority and uses Vision's lower-resource background preference. Frame delivery runs independently. [Vision background preference](https://developer.apple.com/documentation/vision/vnrequest/preferbackgroundprocessing)

The local animations are 2D compositions with a 2.6-second duration. The app applies them after segmentation, preserving the person-mask input. Preview, photos, recordings and virtual output use the same animation state.

The asynchronous person mask already used for blur also supplies the solid backgrounds and studio lighting. Upright live calls retain the bounded 1080p rendering path with the window hidden. Metal and Core Image perform the Lanczos crop in a reusable surface. Accelerate handles the fallback when Metal is unavailable. Apple Silicon vector instructions compose lighting and solid backgrounds with integer-exact parity to the scalar implementation. Full-resolution preview and recording retain the existing Core Image path. Missing or expired masks use a full-background fallback.

The screen light uses four static, thin native NSPanels to form a 36-point white border. Each backing surface covers only its edge of the screen. It accepts no mouse input and creates no frame-processing worker.

The tested PIXY advertises native Apple reactions through 1080p. Its 1440p and 4K formats report unsupported. On supported formats, native gestures enabled in the macOS Video menu take precedence over local detection. The macOS menu controls those native effects separately.

## Automated verification

The standard suite covers sustained-pose gating, late observations, all animation renderers at landscape and portrait sizes, legacy settings, masked lighting/color composition and capture shutdown during a permission request.

A separate ignored test exercises Apple Vision with external photo fixtures. It covers all eight reactions, including two-hand compositions, and negative fist, pointing and single-rock poses. The test reduces fixtures to the same maximum 480-pixel input used during calls.

Set **PIXY_GESTURE_FIXTURES** to a directory containing the following files and run the ignored test.

    PIXY_GESTURE_FIXTURES=/absolute/fixture-directory cargo test vision_classifies_real_hand_photos -- --ignored --nocapture

Download thumb_up.jpg, victory.jpg, fist.jpg and pointing_up.jpg from Google's MediaPipe assets. The test rotates the thumbs-up fixture for thumbs-down and duplicates single hands for the paired variants. It also uses Hand_heart.JPG by Dylan.cronk and a crop from the HaGRID gesture chart. Save the Wikimedia file as heart.jpg and the HaGRID chart as hagrid.png. Fixtures stay outside this repository. [MediaPipe assets](https://storage.googleapis.com/mediapipe-assets/thumb_up.jpg) · [Hand heart photograph and license](https://commons.wikimedia.org/wiki/File:Hand_heart.JPG) · [HaGRID chart](https://raw.githubusercontent.com/hukenovs/hagrid/master/images/gestures.png)

## Live verification

Testing took place on 2026-11-07 with a physical PIXY at 3840×2160/30 fps, local blur 0.65, zoom 1.3 and a hidden main window.

A private Google Meet call received the processed 1280×720 video. Local animated reactions were visible over the blurred picture. The native Teams device preview also displayed the processed picture with blur and the Pixy main window hidden. These checks cover the call's local preview and the native virtual source. Teams renderer cadence and remote receive quality require separate measurement.

A separate native consumer measures distinct processed images using CRC source markers. Each cadence trial lasts 60 seconds after warmup. Fixed acceptance limits are at least 28.5 distinct images/s, p95 gap at most 50 ms and maximum gap at most 100 ms.

Earlier CPU-rendered trials passed intermittently. Fully hidden-window trials also recorded peaks of 200.83 ms, 247.98 ms, 262.61 ms and 369.75 ms. The traced producer showed missed frame deadlines before publication. Moving the crop to Metal and vectorizing lighting reduced the CPU work per frame.

### Exposure comparison on the same installed build

The PIXY's UVC camera-terminal descriptor omits the frame-rate-priority control. The tested exposure modes were automatic and manual. The first four trials changed the exposure mode on the same running app. The shutter value read back as 300 units of 100 µs, equivalent to 30 ms. Blur and local reactions stayed active, with 14–15 animation triggers per resource sample.

| Exposure | Distinct fps | p95 gap ms | Maximum gap ms | Result |
| --- | --- | --- | --- | --- |
| Automatic | 27.37 | 66.68 | 100.05 | Fail |
| Fixed 30 ms | 29.97 | 33.76 | 66.74 | Pass |
| Automatic repeat | 19.88 | 67.09 | 169.41 | Fail |
| Fixed 30 ms repeat | 29.72 | 33.90 | 85.14 | Pass |
| Fixed 30 ms with studio and edge lighting | 29.73 | 36.30 | 133.59 | Fail |
| Fixed 30 ms with studio and edge lighting after privacy resume | 29.95 | 36.23 | 66.68 | Pass |

The two fixed-exposure trials used 0.66 of one CPU core including reaped metadata helpers. Peak physical footprint was 385–386 MiB, with peak RSS of 187–203 MiB. These measurements cover the Pixy Control process. The extension, browser, Teams and diagnostic consumer run separately.

Use **Efeitos → Exposição automática** and the exposure control to choose the camera's exposure. In this installation the physical PIXY was configured with fixed 30 ms exposure after testing.

### Privacy and final configuration

Turning the camera off in Meet reduced the virtual consumer count to zero, stopped capture, entered firmware Privacy and turned off the edge light. Turning it on restored the previous camera mode, the fixed 30 ms exposure and the live effects. The green solid background was then observed in the private Meet call with the Pixy window hidden.

The installed app was restarted in normal background mode after the call. Test markers and automatic reaction cycling were removed. The saved framing and blur intensity were restored, gesture recognition was enabled, and optional lighting and solid backgrounds were reset to their defaults. The physical PIXY retained manual exposure at 30 ms.

The tested executable SHA-256 was **b25dd27fa808cbb7082f64279994354e0a3916bd1ea61fd0dab89d7714ff286f**. This build passed 173 standard tests and two explicitly run hardware-framework tests for Metal cropping and Vision photo classification.

## Diagnostic playback

For repeatable reaction-load testing, start the app with explicit diagnostic arguments.

    open "Pixy Control.app" --args --background --sharing-report /tmp/pixy-state.json --trace-camera-frames --exercise-reactions

**--exercise-reactions** sends one of the eight effects every four seconds while valid capture is active. The JSON report includes the trigger count and screen-light visibility. Restart normally to end the exercise and remove frame markers.

A rebuilt ad-hoc-signed executable can cause macOS to request camera permission again. The privacy wake sequence now waits for a pending authorization request instead of abandoning that request at its ordinary eight-second USB timeout.
