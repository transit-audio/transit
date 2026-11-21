<!-- transit-index
status: Open (#709), partly built
follow-up: The converter term of ADR 0015's 2026-11-16 amendment on #709 (the live input aligned from the output converter's position, the probe's click through the output converter, the normalized figure in the record) makes a measurement exact across device rates on paired devices; until it lands, measure at the rate in use.
-->

A hardware insert round trip or External Instrument latency measured with the audio device at one rate is not exact with the device at another rate: since v0.7.3 the rate converters add a delay at a converted rate that depends on their phase and the input history kept at alignment (#724), and the interface's own delay can change with the rate. Since this change every new measurement records the device rate it was taken at; a figure used at another device rate, or saved by v0.7.4 or older (no device rate recorded, so at every rate, equal rates included), is used as measured and says "measure again at this rate" on the insert's card, in the Audio Devices table and on the External Instrument's card. Nothing is adjusted silently. Manual External Instrument latency is never flagged or changed.
