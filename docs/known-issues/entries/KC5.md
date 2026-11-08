<!-- transit-index
status: Open, needs a tester
follow-up: Tester check on Ubuntu 24.04 with `jackd2` and `pipewire-jack`: JACK connects without `pw-jack` and Audio Devices shows "JACK library: /usr/lib/x86_64-linux-gnu/pipewire-0.3/jack/libjack.so.0". Not testable in WSL (no PipeWire): unloading PipeWire's library after a failed probe while its server is down. If either fails, bind our own function table instead of `jack-sys`'s static.
order: 78
-->

Native JACK library fallback (#230): JACK 2's libjack stays mapped after `dlclose` (measured with libjack-jackd2 1.9.20 in the review), so once it was probed, `jack-sys`'s `dlopen("libjack.so.0")` binds it again and PipeWire's library cannot serve that process; the binding check then refuses with "... connected, but another libjack was already loaded in this process; start transit with pw-jack". That is the expected outcome with `jackd2` installed and the default-first order, so with a PipeWire socket present (and `TRANSIT_JACK_LIBRARY` unset) PipeWire's library is probed before the default and JACK 2's is never loaded. It remains when no PipeWire socket was seen at the first probe.
