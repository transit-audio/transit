//! Which libjack the backend uses (#230). The loader's default `libjack.so.0` is JACK 2's when the
//! `jackd2` package is installed, and JACK 2's library finds no server under PipeWire: PipeWire's
//! replacement library lives off the loader path on Debian and Ubuntu (`pw-jack` puts it on
//! `LD_LIBRARY_PATH`). So the backend tries, in order, the library `TRANSIT_JACK_LIBRARY` names,
//! the loader's default, then PipeWire's library at the distributions' paths, and uses the first one
//! whose probe client connects. Safe code over a [`Loader`]: `crate::ffi` implements it over
//! `dlopen`, the tests over a script, so the order is tested without a server.

/// The environment variable that names a libjack to try first (a path, or a soname the loader
/// resolves). PipeWire documents no such variable (`pw-jack` sets `LD_LIBRARY_PATH`), so this one is
/// transit's own.
pub const LIBRARY_ENV: &str = "TRANSIT_JACK_LIBRARY";
/// The loader's default: JACK 2's library, or PipeWire's where `pw-jack` or an `ld.so.conf.d` entry
/// puts it first.
pub const DEFAULT_LIBRARY: &str = "libjack.so.0";
/// PipeWire's libjack (the `pipewire-jack` package) where the distributions install it: Debian and
/// Ubuntu's multiarch directories, then Fedora's `lib64`, then Arch's `lib`.
pub const PIPEWIRE_LIBRARIES: [&str; 4] = [
    "/usr/lib/x86_64-linux-gnu/pipewire-0.3/jack/libjack.so.0",
    "/usr/lib/aarch64-linux-gnu/pipewire-0.3/jack/libjack.so.0",
    "/usr/lib64/pipewire-0.3/jack/libjack.so.0",
    "/usr/lib/pipewire-0.3/jack/libjack.so.0",
];

/// What one library's probe found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A probe client opened (and was closed): this library talks to a running server.
    Connected,
    /// The library did not load, or lacks `jack_client_open`; the loader's text.
    LoadFailed(String),
    /// It loaded, and no server answered it (`JackServerFailed`); none was started.
    NotRunning,
    /// The server refused the probe client for another reason.
    Failed(String),
}

/// One library tried, and what its probe found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub library: String,
    pub outcome: Outcome,
}

/// Loads a library and opens (then closes) a probe client through it, never starting a server. A
/// library that connected stays loaded; one that did not is unloaded.
pub(crate) trait Loader {
    fn probe(&mut self, library: &str) -> Outcome;
}

/// The libraries to try, in order: `env` (the [`LIBRARY_ENV`] value) when set, the loader's
/// default, then each of [`PIPEWIRE_LIBRARIES`] that `exists`. No library is listed twice.
pub fn candidates(env: Option<&str>, exists: impl Fn(&str) -> bool) -> Vec<String> {
    let mut list: Vec<String> = Vec::new();
    let named = env.map(str::trim).filter(|v| !v.is_empty());
    for library in named.into_iter().chain([DEFAULT_LIBRARY]).chain(PIPEWIRE_LIBRARIES.into_iter().filter(|p| exists(p))) {
        if !list.iter().any(|l| l == library) {
            list.push(library.to_owned());
        }
    }
    list
}

/// What the selection found: every library tried, in order, and the one that connected.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Selection {
    pub attempts: Vec<Attempt>,
    /// Whether PipeWire's library was on the list at all (installed at one of its paths, or named).
    pub pipewire_found: bool,
}

impl Selection {
    /// The library that connected (the last attempt, when it did).
    pub fn connected(&self) -> Option<&str> {
        self.attempts.last().filter(|a| a.outcome == Outcome::Connected).map(|a| a.library.as_str())
    }

    /// Why no library connected, as the backend's error: the library is missing when none loaded,
    /// no server when one loaded and none answered, else the first refusal. `None` when one
    /// connected.
    pub(crate) fn error(&self) -> Option<crate::api::ClientError> {
        use crate::api::ClientError;
        if self.connected().is_some() {
            return None;
        }
        if self.attempts.iter().any(|a| a.outcome == Outcome::NotRunning) {
            return Some(ClientError::ServerNotRunning);
        }
        if let Some(text) = self.attempts.iter().find_map(|a| if let Outcome::Failed(t) = &a.outcome { Some(t.clone()) } else { None }) {
            return Some(ClientError::Failed(text));
        }
        let text = self.attempts.iter().find_map(|a| if let Outcome::LoadFailed(t) = &a.outcome { Some(t.clone()) } else { None });
        Some(ClientError::LibraryMissing(text.unwrap_or_default()))
    }

    /// The Audio Devices line: "JACK library: <path>" when one connected, else every library tried
    /// with its result, and a hint when PipeWire's library is not installed.
    pub fn summary(&self) -> String {
        if let Some(library) = self.connected() {
            return format!("JACK library: {library}");
        }
        let tried: Vec<String> = self
            .attempts
            .iter()
            .map(|a| {
                let why = match &a.outcome {
                    Outcome::Connected => "connected".to_owned(),
                    Outcome::LoadFailed(_) => "not loaded".to_owned(),
                    Outcome::NotRunning => "no server answered".to_owned(),
                    Outcome::Failed(text) => text.clone(),
                };
                format!("{} ({why})", a.library)
            })
            .collect();
        let mut line = format!("JACK libraries tried: {}", tried.join(", "));
        if !self.pipewire_found {
            line.push_str("; PipeWire's JACK library is not installed (pipewire-jack)");
        }
        line
    }
}

/// Probes `candidates` in order through `loader` and stops at the first that connects.
pub(crate) fn select(candidates: &[String], loader: &mut dyn Loader) -> Selection {
    let pipewire_found = candidates.iter().any(|c| c != DEFAULT_LIBRARY && c.contains("pipewire"));
    let mut selection = Selection { attempts: Vec::new(), pipewire_found };
    for library in candidates {
        let outcome = loader.probe(library);
        let connected = outcome == Outcome::Connected;
        selection.attempts.push(Attempt { library: library.clone(), outcome });
        if connected {
            break;
        }
    }
    selection
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ClientError;

    /// Answers each library from a script and records the order it was asked in.
    struct Script {
        answers: Vec<(&'static str, Outcome)>,
        asked: Vec<String>,
    }

    impl Loader for Script {
        fn probe(&mut self, library: &str) -> Outcome {
            self.asked.push(library.to_owned());
            self.answers.iter().find(|(l, _)| *l == library).map_or(Outcome::LoadFailed("no such file".into()), |(_, o)| o.clone())
        }
    }

    const UBUNTU: &str = PIPEWIRE_LIBRARIES[0];

    #[test]
    fn the_order_is_the_named_library_the_default_then_pipewires_installed_paths() {
        let installed = |p: &str| p == UBUNTU || p == PIPEWIRE_LIBRARIES[3];
        assert_eq!(candidates(None, installed), [DEFAULT_LIBRARY, UBUNTU, PIPEWIRE_LIBRARIES[3]]);
        assert_eq!(candidates(Some("/opt/jack/libjack.so.0"), installed), ["/opt/jack/libjack.so.0", DEFAULT_LIBRARY, UBUNTU, PIPEWIRE_LIBRARIES[3]]);
        assert_eq!(candidates(Some("  "), |_| false), [DEFAULT_LIBRARY], "an empty variable names nothing");
        assert_eq!(candidates(Some(UBUNTU), installed), [UBUNTU, DEFAULT_LIBRARY, PIPEWIRE_LIBRARIES[3]], "no library twice");
    }

    #[test]
    fn jack2s_library_without_jackd_falls_back_to_pipewires() {
        // The tester's machine (#230): jackd2's libjack is the default and finds no jackd;
        // PipeWire answers through its own library.
        let mut loader = Script { answers: vec![(DEFAULT_LIBRARY, Outcome::NotRunning), (UBUNTU, Outcome::Connected)], asked: Vec::new() };
        let list = candidates(None, |p| p == UBUNTU);
        let selection = select(&list, &mut loader);
        assert_eq!(loader.asked, [DEFAULT_LIBRARY, UBUNTU]);
        assert_eq!(selection.connected(), Some(UBUNTU));
        assert_eq!(selection.error(), None);
        assert_eq!(selection.summary(), format!("JACK library: {UBUNTU}"));
    }

    #[test]
    fn the_first_library_that_connects_wins_and_the_rest_are_not_loaded() {
        let mut loader = Script { answers: vec![(DEFAULT_LIBRARY, Outcome::Connected), (UBUNTU, Outcome::Connected)], asked: Vec::new() };
        let selection = select(&candidates(None, |_| true), &mut loader);
        assert_eq!(loader.asked, [DEFAULT_LIBRARY], "JACK 2 with jackd running, or pw-jack: the default serves");
        assert_eq!(selection.connected(), Some(DEFAULT_LIBRARY));
    }

    #[test]
    fn with_none_connected_the_error_and_the_line_say_what_each_library_did() {
        let mut loader = Script { answers: vec![(DEFAULT_LIBRARY, Outcome::NotRunning), (UBUNTU, Outcome::NotRunning)], asked: Vec::new() };
        let selection = select(&candidates(None, |p| p == UBUNTU), &mut loader);
        assert_eq!(selection.error(), Some(ClientError::ServerNotRunning));
        assert_eq!(selection.summary(), format!("JACK libraries tried: libjack.so.0 (no server answered), {UBUNTU} (no server answered)"));

        let mut nothing = Script { answers: Vec::new(), asked: Vec::new() };
        let selection = select(&candidates(None, |_| false), &mut nothing);
        assert_eq!(selection.error(), Some(ClientError::LibraryMissing("no such file".into())));
        assert_eq!(selection.summary(), "JACK libraries tried: libjack.so.0 (not loaded); PipeWire's JACK library is not installed (pipewire-jack)");

        let mut refused = Script { answers: vec![(DEFAULT_LIBRARY, Outcome::Failed("status 0x21".into()))], asked: Vec::new() };
        let selection = select(&candidates(None, |_| false), &mut refused);
        assert_eq!(selection.error(), Some(ClientError::Failed("status 0x21".into())));
    }
}
