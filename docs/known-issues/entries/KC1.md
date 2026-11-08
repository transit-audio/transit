<!-- transit-index
status: Open, cosmetic
follow-up: Probe once per device rescan instead of once per Auto evaluation, if cheap.
order: 74
-->

Native JACK: with JACK 2's libjack installed and no server running, each probe that finds no library connected (`factory_for`, `jack_system`; since #230 also each probe that falls back to PipeWire's library before one connects) makes JACK 2's libjack print "Cannot connect to server socket" / "jack server is not running" to stderr (libjack's error function is left at its default by design, ADR 0021 decision 7). Once a library connects it is kept and nothing is probed again. Cosmetic.
