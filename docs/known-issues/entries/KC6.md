<!-- transit-index
status: Open, decision pending
follow-up: Make the chosen size the period on ALSA (request 4x from cpal, or periods in the cpal fork), with the latency compensation and calibration keys that read the device buffer; then default the request to the server's quantum.
order: 79
-->

ALSA under PipeWire (#230): the buffer control sets the ALSA buffer, and cpal asks for 4 periods (the pinned fork's `set_period_size_near(v / 4)`), so PipeWire runs transit's node at about a quarter of the chosen size (2048 gave 512; 128 asked for 32 and the plug-in raised it to its 64 minimum). The whole buffer stays queued, so the latency is about the buffer plus the quantum. Audio Devices shows buffer, period and the server's quantum ("about" the period when none is forced: the graph's quantum is not read), labels each size with its period, and offers only buffers whose periods the server allows. Open with it: the issue's acceptance item 3, the default request following the server's quantum.
