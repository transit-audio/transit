<!-- transit-index
status: Open, unverified
follow-up: Tester check, step 6 of the Linux acceptance list in the JACK slice 2 claim note.
order: 76
-->

Native JACK: PipeWire's libjack is not verified to call the buffer-size callback at activation (JACK 2 does). If it does not, a quantum change between open and start leaves the achieved buffer stale with no reset.
