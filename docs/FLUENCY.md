# Virtual output verification

The physical-camera callback now retains the incoming sample and returns promptly. A separate worker processes at most one in-flight sample and one replaceable pending sample. It keeps the original AVFoundation delivery queue's User Interactive QoS. This is normal macOS scheduling, not a real-time thread policy. Capture timestamps, bounded output queues, immutable published pixels, and privacy-generation checks remain in place.

## Repeated hardware measurements

Test conditions: physical 3840×2160 at 30 fps, background blur 0.65, digital crop enabled, app window hidden and not visible. A separate native AVFoundation consumer reads the virtual camera. CRC-verified camera-frame markers distinguish different source images from repeated transport samples. Each measurement lasts 60 seconds after warmup.

Fixed acceptance limits: at least 28.5 distinct images/s, p95 distinct-image gap no greater than 50 ms, maximum gap no greater than 100 ms.

| Delivery-priority worker | Distinct fps | p95 gap | Maximum gap | Result |
| --- | ---: | ---: | ---: | --- |
| Run 1 | 29.23 | 36.41 ms | 69.56 ms | Pass |
| Run 2 | 29.92 | 33.52 ms | 68.32 ms | Pass |
| Run 3 | 29.90 | 34.13 ms | 67.05 ms | Pass |

The earlier worker used a lower scheduling class: two repetitions passed, but a third had a 183.45 ms gap and failed. Reducing physical capture to 2K also failed. Those failures were not discarded or counted as successful verification of the final worker.

In two 55-second samples of the final worker, Pixy plus its reaped metadata helpers used about 0.77–0.78 CPU core, with peak process physical footprints of 305–307 MiB. A separate 55-second sample during a private Google Meet call used about 0.74 core and peaked at 354 MiB. These figures exclude the EMEET extension service, browser/call app, and diagnostic consumer. They are observations on one Apple Silicon Mac, not a memory-leak test or a universal resource guarantee.

## Call-app coverage

A private Google Meet call received the processed video with blur while the Pixy window remained hidden. Turning the Meet camera off released the virtual consumer and entered firmware Privacy; turning it on restored Tracking and processed video. Leaving the call was also checked. The microphone was muted before joining; no other participant was invited and no recording was started.

The browser automation interface did not expose fine-grained presented-frame counters, so the numeric cadence measurements above belong to the separate native consumer, not to Meet's renderer or a remote participant. Native Microsoft Teams could not be operated through this environment's available UI tools, and Teams web required an authenticated session. **Native Teams fluency and remote-call quality have not been independently measured.**

The already installed EMEET extension remains the transport. Pixy does not modify or redistribute it. Its source-client counter still requires bounded fresh metadata queries because the extension does not notify changes to that custom property.
