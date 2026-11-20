use super::*;

impl Player {
    /// The links other programs made to the open device's ports (the native JACK backend; `None`
    /// elsewhere), read for one reopen (ADR 0015, amendment #431, rule 6). Call it on the control
    /// thread that replaces this Player, before anything of it closes: the reopen holds the
    /// snapshot while this Player drops and hands it to its replacement's
    /// [`Self::restore_links`].
    pub fn snapshot_links(&self) -> Option<ab::LinkSnapshot> {
        self.stream.as_ref().and_then(|stream| stream.snapshot_links())
    }

    /// Restores, once, a snapshot the Player this one replaced took ([`Self::snapshot_links`]),
    /// on this Player's started device. Returns the diagnostic for the user when some links were
    /// not restored. A Player of another audio system, or with no device stream, discards the
    /// snapshot.
    pub fn restore_links(&self, snapshot: ab::LinkSnapshot) -> Option<String> {
        self.stream.as_ref().and_then(|stream| stream.restore_links(snapshot))
    }

    /// The open device's display-only lines for Audio Devices (native JACK: the graph, the latency
    /// reported at startup, PipeWire's sentence, links not observed at the startup check; ADR 0021,
    /// amendment for #760, rules 3 and 6). Empty elsewhere and for a silent runtime. They feed
    /// nothing: Path latency and compensation keep `output_latency`.
    pub fn device_lines(&self) -> Vec<String> {
        if self.silent {
            return Vec::new();
        }
        self.stream.as_ref().map(|stream| stream.device_lines()).unwrap_or_default()
    }
}
