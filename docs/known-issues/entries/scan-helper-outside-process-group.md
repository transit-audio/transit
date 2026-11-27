<!-- transit-index
status: Open, platform
follow-up: None needed for transit itself. Stop a helper that outlived a scan by hand, and keep plug-ins from starting helpers in a process group or session of their own.
-->

On Linux and macOS, a plug-in scan child and everything it starts are ended with the scan, and when transit itself ends, a crash included. One exception: a helper process that a plug-in starts in a new process group or session of its own (for example with `setsid`) is outside that reach and can keep running after the probe, or after transit has quit. Such a process no longer holds anything inside transit: the scan stops reading its output and closes the pipe when the probe ends. Until then, output it inherited from the probe child can still be read as that probe's answer. A scan child notices that transit has gone on a periodic check, not instantly. Windows is not affected: a scan child and every process it starts belong to a job that ends with transit.
