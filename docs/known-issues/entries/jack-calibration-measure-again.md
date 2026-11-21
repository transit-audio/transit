<!-- transit-index
status: Open, cosmetic (ADR 0021, amendment for #760)
follow-up: The note gets an action when a loopback Measure exists on the JACK system (K70, K72). Until then nothing is needed: the record is kept and not applied.
-->

Native JACK: a stored loopback calibration that transit holds back (saved without the buffer size the server ran, or measured through other ports of the input) shows a note ending in "measure again", but no build of transit can measure a loopback calibration on the JACK system: the app's Measure is native ASIO only and `audio-bench` measures WASAPI and ASIO loopbacks. Such a record can exist only in a hand-edited settings file, or as a record from another system whose device name equals a JACK port group's. Nothing wrong is applied: the record is kept and not used.
