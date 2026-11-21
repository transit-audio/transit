<!-- transit-index
status: Open (#725), partly fixed in v0.7.4
follow-up: Record with a count-in needs ADR 0052's 2026-11-16 amendment revisited first (running the converter on moves the first click by the reader's lead); Locate needs a decision on what a live input sounding across a locate does. Run the audio device at the session rate to avoid both.
-->

At a converted device rate (the device runs at a rate other than the session's), starting Play or Record from a stop cut audio that was already sounding (#725). Since v0.7.4 Play and Record without a count-in keep the converter running through the start (confirmed on hardware with a 44.1 kHz interface and a 48 kHz session). Still open: Record with a count-in restarts the converter when the count-in begins, so audio sounding at that moment gets a splice (about 17 to 19 session frames, 0.35 to 0.4 ms, skipped, and the filter starts from silence), and Locate while playing restarts it too. A device at the session rate is unaffected.
