//! The command line (#741): one table of the app's flags, the help text built from it, and a pure
//! classification of the argument list that runs before anything with side effects (no window, audio
//! device, plug-in scan, log file or settings write).
//!
//! Not in the table: `--scan-vst3 <file>` and `--scan-clap <file>`, the plug-in scan child's own
//! arguments. `main` answers them first (`transit_ui_studio::plugin_scan::scan_child`) and never
//! classifies them; they are not for people.

use std::path::PathBuf;

/// How a flag takes a value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Value {
    /// A switch.
    None,
    /// The next argument is the value; without one the flag is an error (`missing` is its text).
    Required { placeholder: &'static str, missing: &'static str },
    /// The next argument is the value when it does not start with `--`; otherwise there is none.
    Optional { placeholder: &'static str },
}

/// One flag of the app.
#[derive(Clone, Copy, Debug)]
pub struct Flag {
    pub name: &'static str,
    pub short: Option<&'static str>,
    /// Other spellings that mean the same (shown on the flag's help line).
    pub aliases: &'static [&'static str],
    pub value: Value,
    /// `--name=value` is accepted as well (only `--log` ever was).
    pub equals: bool,
    pub help: &'static str,
}

/// Every flag the app accepts. The help text, the parser and the docs check all read this.
pub const FLAGS: &[Flag] = &[
    Flag { name: "--help", short: Some("-h"), aliases: &[], value: Value::None, equals: false, help: "Print this list and exit." },
    Flag { name: "--version", short: Some("-V"), aliases: &[], value: Value::None, equals: false, help: "Print the version and exit." },
    Flag {
        name: "--report",
        short: None,
        aliases: &[],
        value: Value::Optional { placeholder: "[path]" },
        equals: false,
        help: "Write a diagnostics file (to path, or the current directory) and exit; opens no window or audio stream.",
    },
    Flag { name: "--demo", short: None, aliases: &["--sample"], value: Value::None, equals: false, help: "Start with the demo session." },
    Flag {
        name: "--no-audio",
        short: None,
        aliases: &[],
        value: Value::None,
        equals: false,
        help: "Run with no audio player at all (not with --null-audio).",
    },
    Flag {
        name: "--null-audio",
        short: None,
        aliases: &[],
        value: Value::None,
        equals: false,
        help: "Run on a silent device: no audio hardware is listed or opened (not with --no-audio or --report).",
    },
    Flag {
        name: "--control",
        short: None,
        aliases: &[],
        value: Value::Required { placeholder: "<port>", missing: "--control requires a port from 0 to 65535" },
        equals: false,
        help: "Start the localhost control server on port (0 picks a free port); TRANSIT_CONTROL_PORT does the same.",
    },
    Flag { name: "--verbose", short: None, aliases: &[], value: Value::None, equals: false, help: "Log debug detail for every transit target." },
    Flag {
        name: "--log",
        short: None,
        aliases: &[],
        value: Value::Required { placeholder: "<directives>", missing: "--log requires directives such as debug or warn,transit_mix=trace" },
        equals: true,
        help: "Set the log filter in RUST_LOG syntax, for example warn,transit_mix=trace (also --log=<directives>).",
    },
    Flag { name: "--log-stderr", short: None, aliases: &[], value: Value::None, equals: false, help: "Copy the log to standard error." },
];

/// What the arguments ask for.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Print [`help_text`] and exit 0.
    Help,
    /// Print the version and exit 0.
    Version,
    /// Write the diagnostics file (to the path, if any) and exit.
    Report(Option<PathBuf>),
    /// Print the text (with its hint) and exit non-zero; nothing is started.
    Error(String),
    /// Start the app.
    Run(Launch),
}

/// What a normal launch was asked for.
#[derive(Debug, PartialEq, Eq)]
pub struct Launch {
    pub demo: bool,
    pub no_audio: bool,
    pub null_audio: bool,
    pub control_port: Option<u16>,
    /// The session files to open, in order.
    pub files: Vec<String>,
}

/// The hint line after every usage error.
pub const HELP_HINT: &str = "Run 'transit --help' for the list of options.";

fn find(arg: &str) -> Option<&'static Flag> {
    FLAGS.iter().find(|f| f.name == arg || f.short == Some(arg) || f.aliases.contains(&arg))
}

/// The `--help` text: usage, then one line per user-facing flag.
pub fn help_text() -> String {
    let mut out = String::from("Usage: transit [options] [session.transit ...]\n\nOptions:\n");
    for f in FLAGS {
        let mut left = String::new();
        if let Some(short) = f.short {
            left.push_str(short);
            left.push_str(", ");
        }
        left.push_str(f.name);
        match f.value {
            Value::None => {}
            Value::Required { placeholder, .. } | Value::Optional { placeholder } => {
                left.push(' ');
                left.push_str(placeholder);
            }
        }
        let aliases = if f.aliases.is_empty() { String::new() } else { format!(" (also {})", f.aliases.join(", ")) };
        out.push_str(&format!("  {left:<26}{}{aliases}\n", f.help));
    }
    out.push_str("  --                        Everything after it is a session path, even one that starts with -.\n");
    out.push_str("\nA path that is not an option opens as a session.\n");
    out
}

fn usage_error(text: &str) -> Command {
    Command::Error(format!("transit: {text}\n{HELP_HINT}"))
}

/// Classifies the arguments (without the program name) before anything else starts. `env_control` is
/// `TRANSIT_CONTROL_PORT`; `slash_help` makes exactly `/?` mean help (Windows only; elsewhere an
/// argument starting with `/` is a path); `ignore_psn` skips `-psn_*` arguments (macOS only).
///
/// Help wins over every error. A lone `-` and any other argument starting with `-` that is not in
/// [`FLAGS`] is an error; a path that starts with `-` goes after `--`.
pub fn classify(args: &[String], env_control: Option<&str>, slash_help: bool, ignore_psn: bool) -> Command {
    let mut help = slash_help && args.iter().any(|a| a == "/?");
    let (mut version, mut report) = (false, false);
    let (mut demo, mut no_audio, mut null_audio) = (false, false, false);
    let mut files = Vec::new();
    let mut first_error: Option<Command> = None;
    let mut end = args.len();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        i += 1;
        if arg == "--" {
            files.extend(args[i..].iter().cloned());
            end = i - 1;
            break;
        }
        if slash_help && arg == "/?" {
            continue;
        }
        // macOS only: a Finder launch may pass a process serial number (`-psn_...`); unverified for current versions, kept as a guard.
        if ignore_psn && arg.starts_with("-psn_") {
            continue;
        }
        if !arg.starts_with('-') {
            files.push(arg.to_owned());
            continue;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if arg.starts_with("--") && find(n).is_some_and(|f| f.equals) => (n, Some(v)),
            _ => (arg, None),
        };
        let Some(flag) = find(name) else {
            first_error.get_or_insert_with(|| usage_error(&format!("unknown option '{arg}'")));
            continue;
        };
        match flag.value {
            Value::None => {}
            Value::Required { missing, .. } => {
                if inline.is_none() {
                    match args.get(i) {
                        // `--log` takes directives, never another flag (its rule since #129).
                        Some(v) if flag.name != "--log" || !v.starts_with("--") => i += 1,
                        _ => {
                            first_error.get_or_insert_with(|| usage_error(missing));
                        }
                    }
                }
            }
            Value::Optional { .. } => {
                if args.get(i).is_some_and(|v| !v.starts_with("--")) {
                    i += 1;
                }
            }
        }
        match flag.name {
            "--help" => help = true,
            "--version" => version = true,
            "--report" => report = true,
            "--demo" => demo = true,
            "--no-audio" => no_audio = true,
            "--null-audio" => null_audio = true,
            _ => {}
        }
    }
    if help {
        return Command::Help;
    }
    if let Some(error) = first_error {
        return error;
    }
    let options = &args[..end];
    if report {
        if let Some(refusal) = super::report_refusal(options) {
            return Command::Error(format!("transit --report: {refusal}"));
        }
        return Command::Report(crate::report::parse(options).flatten());
    }
    if version {
        return Command::Version;
    }
    if no_audio && null_audio {
        return Command::Error("--no-audio and --null-audio cannot be combined".into());
    }
    match super::parse_control_port(options, env_control) {
        Ok(control_port) => Command::Run(Launch { demo, no_audio, null_audio, control_port, files }),
        Err(error) => Command::Error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    fn run(values: &[&str]) -> Command {
        classify(&a(values), None, false, false)
    }

    fn launch(values: &[&str]) -> Launch {
        match run(values) {
            Command::Run(l) => l,
            other => panic!("{values:?}: expected Run, got {other:?}"),
        }
    }

    fn error(command: Command) -> String {
        match command {
            Command::Error(text) => text,
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn help_in_every_spelling_returns_before_anything_starts() {
        for args in [&["--help"][..], &["-h"], &["--demo", "-h"], &["--bogus", "--help"], &["song.transit", "--help"]] {
            assert_eq!(run(args), Command::Help, "{args:?}");
        }
        assert_eq!(classify(&a(&["/?"]), None, true, false), Command::Help);
        // Off Windows `/?` is a path; on Windows only the exact argument is help.
        assert_eq!(launch(&["/?"]).files, a(&["/?"]));
        let other = classify(&a(&["/tmp/x.transit"]), None, true, false);
        assert!(matches!(other, Command::Run(l) if l.files == a(&["/tmp/x.transit"])));
    }

    #[test]
    fn a_process_serial_number_argument_is_ignored_only_when_asked() {
        assert_eq!(classify(&a(&["-psn_0_12345"]), None, false, true), run(&[]));
        let with_path = classify(&a(&["-psn_0_12345", "s.transit"]), None, false, true);
        assert!(matches!(with_path, Command::Run(l) if l.files == a(&["s.transit"])));
        assert!(error(run(&["-psn_0_12345"])).contains("unknown option '-psn_0_12345'"));
    }

    #[test]
    fn unknown_options_are_errors_that_name_the_argument() {
        for bad in ["--bogus", "--no-audo", "-x", "-", "--control=5", "-hh", "--Help"] {
            let text = error(run(&["--demo", bad]));
            assert!(text.contains("unknown option") && text.contains(&format!("'{bad}'")) && text.contains("--help"), "{bad}: {text}");
        }
    }

    #[test]
    fn a_session_path_still_opens_and_dashes_end_the_options() {
        assert_eq!(launch(&["song.transit"]).files, a(&["song.transit"]));
        assert_eq!(launch(&["a.transit", "--demo", "/b/c d.transit"]).files, a(&["a.transit", "/b/c d.transit"]));
        let after = launch(&["--demo", "--", "-odd.transit", "--demo"]);
        assert!(after.demo);
        assert_eq!(after.files, a(&["-odd.transit", "--demo"]));
        assert!(!launch(&["--", "--null-audio"]).null_audio, "after -- a flag is a path");
    }

    #[test]
    fn every_flag_in_the_table_parses_with_and_without_its_value() {
        for f in FLAGS {
            for name in std::iter::once(f.name).chain(f.short).chain(f.aliases.iter().copied()) {
                match f.value {
                    Value::None => assert!(!matches!(run(&[name]), Command::Error(_)), "{name}"),
                    Value::Required { .. } => {
                        let value = if f.name == "--control" { "0" } else { "debug" };
                        let with = run(&[name, value, "s.transit"]);
                        assert!(matches!(&with, Command::Run(l) if l.files == a(&["s.transit"])), "{name} {value}: {with:?}");
                        let text = error(run(&[name]));
                        assert!(text.contains(name) && text.contains("requires"), "{name}: {text}");
                    }
                    Value::Optional { .. } => {
                        assert_eq!(run(&[name, "out.txt"]), Command::Report(Some("out.txt".into())));
                        assert_eq!(run(&[name]), Command::Report(None));
                    }
                }
            }
        }
        assert!(!matches!(run(&["--log=warn"]), Command::Error(_)));
        assert_eq!(run(&["--version"]), Command::Version);
        assert_eq!(run(&["-V"]), Command::Version);
        let l = launch(&["--sample", "--no-audio", "--control", "0", "--verbose", "--log", "warn", "--log-stderr"]);
        assert!(l.demo && l.no_audio && !l.null_audio && l.control_port == Some(0) && l.files.is_empty());
        assert!(launch(&["--null-audio"]).null_audio);
    }

    #[test]
    fn the_refused_combinations_keep_their_text() {
        assert!(error(run(&["--report", "--null-audio"])).contains("--report and --null-audio cannot be combined"));
        assert!(error(run(&["--no-audio", "--null-audio"])).contains("--no-audio and --null-audio cannot be combined"));
        assert!(error(run(&["--control"])).contains("--control requires a port from 0 to 65535"));
        assert!(error(run(&["--control", "70000"])).contains("Invalid control port"));
        assert!(error(run(&["--log", "--verbose"])).contains("--log requires directives"));
        let from_env = classify(&a(&[]), Some("77"), false, false);
        assert!(matches!(from_env, Command::Run(l) if l.control_port == Some(77)));
        assert!(error(classify(&a(&[]), Some("bad"), false, false)).contains("Invalid control port"));
        // --version still wins over the run-time combination checks, as before.
        assert_eq!(run(&["--version", "--no-audio", "--null-audio"]), Command::Version);
    }

    #[test]
    fn the_help_text_lists_every_flag_on_its_own_line() {
        let text = help_text();
        for f in FLAGS {
            let line = text.lines().find(|l| l.contains(f.name) && l.contains(f.help)).unwrap_or_else(|| panic!("{} missing from help", f.name));
            for other in f.short.into_iter().chain(f.aliases.iter().copied()) {
                assert!(line.contains(other), "{other} not on the line of {}", f.name);
            }
        }
        assert!(text.lines().any(|l| l.trim_start().starts_with("-- ")), "the terminator is listed");
        assert!(!text.contains("--scan-"), "the scan child's flags are internal");
    }

    /// The docs list the same flags as the table: `docs/building.md` mentions every one, and the
    /// flags `docs/releasing.md` names for the screenshot run are all in the table.
    #[test]
    fn the_docs_agree_with_the_table() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
        let building = std::fs::read_to_string(root.join("building.md")).expect("docs/building.md");
        for f in FLAGS {
            for name in std::iter::once(f.name).chain(f.short).chain(f.aliases.iter().copied()) {
                assert!(building.contains(&format!("`{name}")), "docs/building.md does not mention {name}");
            }
        }
        let releasing = std::fs::read_to_string(root.join("releasing.md")).expect("docs/releasing.md");
        let line = releasing.lines().find(|l| l.contains("--demo")).expect("the screenshot command line");
        let span = line.split('`').nth(1).expect("backticked flags");
        for word in span.split_whitespace().filter(|w| w.starts_with("--")) {
            assert!(find(word).is_some(), "docs/releasing.md names {word}, which is not in the table");
        }
    }
}
