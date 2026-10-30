# Escape records

An escaped defect becomes a gate class, not a one-off fix: each record names the defect, why no gate caught it, and the gate that now does (see [the escape record format](../contributing/evidence/escape-record.md)).

- [E45: retrospective MIDI time overflow (#45)](E45-retrospective-time-overflow.md)
- [E45b: MIDI position format compatibility (#45)](E45b-midi-position-compatibility.md)
- [E46: the Windows reopen-state test passed without running (#36)](E46-reopen-state-test-never-ran.md)
- [E47: header state jobs raced the rendered document (#73)](E47-header-state-jobs-raced-the-rendered-document.md)
- [E48: late plug-in instances were discarded (#82)](E48-late-plug-in-instances-were-discarded.md)
- [E49: the echo latency test asserted a 10 ms wall-clock bound (#87)](E49-echo-latency-test-wall-clock-bound.md)
- [E50: the live-input ring kept a per-session backlog in front of the reader (#88)](E50-input-ring-backlog-shifted-the-live-input.md)
- [E51: test temp dirs collided between concurrent tests (#89)](E51-test-temp-dirs-collided-in-one-process.md)
- [E52: the Measure Cancel test assumed the MIDI output thread had kept up with the pump](E52-measure-cancel-test-assumed-the-output-threads-progress.md)
- [E53: Controller-only MIDI recordings were discarded](E53-controller-only-recordings-were-discarded.md)
- [E54: The Measure alignment guard was validated only on a lockstep fake](E54-measure-guard-validated-only-on-a-lockstep-fake.md)
- [E55: A MIDI feedback loop ran until the app was closed](E55-midi-feedback-loop-ran-until-the-app-was-closed.md)
- [E56: A lost graphics device froze the app in a plug-in's unload code](E56-a-lost-graphics-device-froze-the-app-in-a-plug-in-unload.md)
- [E57: an autosave folder's lock outlived its session in a spawned child](E57-an-autosave-lock-outlived-its-session-in-a-spawned-child.md)
- [E58: an armed learn skipped the first message of a port it opened](E58-an-armed-learn-skipped-the-first-message-of-a-port-it-opened.md)
- [E59: Active Sensing was stamped when the worker looked, not when it arrived](E59-active-sensing-was-stamped-when-the-worker-looked-not-when-it-arrived.md)
- [E60: a recovered-autosave test left the save order to the clock](E60-a-recovered-autosave-test-left-the-save-order-to-the-clock.md)
- [E61: a starved gate run shared the host with cold builds](E61-a-starved-gate-run-shared-the-host-with-cold-builds.md)
- [E62: an autosave test polled the clock for a background write (#189)](E62-an-autosave-test-polled-the-clock-for-a-background-write.md)
- [E63: the Speech model played out of tune away from 48 kHz (#195)](E63-the-speech-model-played-out-of-tune-away-from-48-khz.md)
