<!-- transit-index
status: Open, by design
follow-up: Read the `default` PCM's definition through alsa-lib if testers hit it.
order: 80
-->

ALSA under PipeWire (#230): ALSA's `default` counts as PipeWire's (server range, period labels, the Buffer/period line) only where pipewire-alsa's `99-pipewire-default.conf` is installed in `/etc/alsa/conf.d` or `/usr/share/alsa/alsa.conf.d`; a `default` routed to PipeWire some other way (an `~/.asoundrc`, `pulse` on pipewire-pulse) shows the plain buffer list.
