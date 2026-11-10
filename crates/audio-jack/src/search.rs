//! Where PipeWire's libjack is on this machine (#430), found the way `pw-jack` finds it, with no
//! distribution's path written into transit.
//!
//! `pw-jack` is a shell script that puts PipeWire's JACK directory (PipeWire's build option
//! `libjack-path`, by default `<libdir>/pipewire-0.3/jack`) in front of `LD_LIBRARY_PATH`. So the
//! search reads two things:
//!
//! 1. **`pw-jack` itself**, the first one on `PATH`: every absolute path in it (a distribution may
//!    write it with the loader's `$LIB` token, expanded here with the library directories the
//!    loader uses) that holds a `libjack.so.0`.
//! 2. **The loader's own search directories**, each joined with `pipewire-0.3/jack/libjack.so.0`:
//!    `LD_LIBRARY_PATH`, the directories of the shared objects already mapped in this process
//!    (`/proc/self/maps`: where this machine's libc lives is its `<libdir>`), the directories of
//!    `/etc/ld.so.conf` and its includes, and the loader's built-in `/lib64`, `/usr/lib64`, `/lib`,
//!    `/usr/lib`.
//!
//! Only libraries that exist are returned, each once (by its canonical path), in that order. Safe,
//! platform-neutral code over a [`MachineView`], so the tests run anywhere with a scripted machine.

/// PipeWire's JACK directory under a library directory (PipeWire's default `libjack-path`).
pub const PIPEWIRE_JACK_SUBDIR: &str = "pipewire-0.3/jack";
/// The libjack file name in a JACK directory.
pub const LIBJACK_FILE: &str = "libjack.so.0";
/// The loader's built-in directories (glibc's system search path on 64- and 32-bit layouts).
pub const SYSTEM_DIRS: [&str; 4] = ["/lib64", "/usr/lib64", "/lib", "/usr/lib"];
/// How deep `include` lines of `ld.so.conf` are followed (a cycle stops here).
const MAX_INCLUDE_DEPTH: usize = 8;

/// What the search reads of the machine.
pub trait MachineView {
    /// An environment variable.
    fn var(&self, name: &str) -> Option<String>;
    /// A text file's contents (`None` when it cannot be read).
    fn read(&self, path: &str) -> Option<String>;
    /// The names of the entries of a directory (empty when it cannot be listed).
    fn list(&self, dir: &str) -> Vec<String>;
    /// The canonical path of a file that exists, `None` otherwise.
    fn canonical(&self, path: &str) -> Option<String>;
}

/// The real machine, through `std`.
pub struct RealMachine;

impl MachineView for RealMachine {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn list(&self, dir: &str) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        entries.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect()
    }

    fn canonical(&self, path: &str) -> Option<String> {
        let path = std::fs::canonicalize(path).ok()?;
        path.is_file().then(|| path.to_string_lossy().into_owned())
    }
}

/// Appends `item` unless it is already in `list`.
fn push_new(list: &mut Vec<String>, item: &str) {
    if !item.is_empty() && !list.iter().any(|l| l == item) {
        list.push(item.to_owned());
    }
}

/// The directory part of an absolute path (`/usr/lib/libc.so.6` -> `/usr/lib`).
fn parent(path: &str) -> Option<&str> {
    let (dir, _) = path.rsplit_once('/')?;
    Some(if dir.is_empty() { "/" } else { dir })
}

/// The directories of the shared objects in a `/proc/self/maps` text, in order of appearance: the
/// path column of every mapping whose file name contains `.so`.
pub fn mapped_dirs(maps: &str) -> Vec<String> {
    let mut dirs = Vec::new();
    for line in maps.lines() {
        // address perms offset dev inode path (the path may contain spaces: everything after the
        // fifth column).
        let mut rest = line;
        for _ in 0..5 {
            rest = rest.trim_start();
            rest = rest.find(char::is_whitespace).map_or("", |end| &rest[end..]);
        }
        let path = rest.trim();
        let file = path.rsplit('/').next().unwrap_or_default();
        if path.starts_with('/')
            && file.contains(".so")
            && let Some(dir) = parent(path)
        {
            push_new(&mut dirs, dir);
        }
    }
    dirs
}

/// The directories an `ld.so.conf` names, following its `include` lines (absolute globs whose last
/// part may hold one `*`, as the distributions write `include /etc/ld.so.conf.d/*.conf`), in order.
/// A file that includes one of the files including it is not read again (the cycle ends there).
pub fn ld_so_conf_dirs(machine: &dyn MachineView, path: &str) -> Vec<String> {
    let mut dirs = Vec::new();
    collect_conf(machine, path, &mut Vec::new(), &mut dirs);
    dirs
}

fn collect_conf(machine: &dyn MachineView, path: &str, chain: &mut Vec<String>, dirs: &mut Vec<String>) {
    if chain.len() > MAX_INCLUDE_DEPTH || chain.iter().any(|open| open == path) {
        return;
    }
    let Some(text) = machine.read(path) else { return };
    chain.push(path.to_owned());
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if let Some(pattern) = line.strip_prefix("include").filter(|rest| rest.starts_with(char::is_whitespace)) {
            for pattern in pattern.split_whitespace() {
                let pattern = if pattern.starts_with('/') { pattern.to_owned() } else { format!("{}/{pattern}", parent(path).unwrap_or("/")) };
                for file in expand_glob(machine, &pattern) {
                    collect_conf(machine, &file, chain, dirs);
                }
            }
        } else if line.starts_with('/') {
            // A directory, optionally followed by `=type` (old hwcap syntax) or more on the line.
            for dir in line.split([' ', '\t', ',', ':']).filter(|d| d.starts_with('/')) {
                push_new(dirs, dir.split('=').next().unwrap_or_default().trim_end_matches('/'));
            }
        }
    }
    chain.pop();
}

/// The files a pattern names: itself without a `*`, else the entries of its directory matching its
/// last part with one `*`, sorted (as glob(3) returns them).
fn expand_glob(machine: &dyn MachineView, pattern: &str) -> Vec<String> {
    let Some((dir, name)) = pattern.rsplit_once('/') else { return Vec::new() };
    let Some((before, after)) = name.split_once('*') else { return vec![pattern.to_owned()] };
    let mut files: Vec<String> = machine
        .list(dir)
        .into_iter()
        .filter(|entry| entry.len() >= before.len() + after.len() && entry.starts_with(before) && entry.ends_with(after))
        .map(|entry| format!("{dir}/{entry}"))
        .collect();
    files.sort();
    files
}

/// The loader's search directories, in order: `LD_LIBRARY_PATH`, the directories of the objects
/// mapped in this process, `/etc/ld.so.conf` and its includes, then [`SYSTEM_DIRS`].
pub fn loader_dirs(machine: &dyn MachineView) -> Vec<String> {
    let mut dirs = Vec::new();
    for dir in machine.var("LD_LIBRARY_PATH").unwrap_or_default().split([':', ';']) {
        if dir.starts_with('/') {
            push_new(&mut dirs, dir.trim_end_matches('/'));
        }
    }
    for dir in mapped_dirs(&machine.read("/proc/self/maps").unwrap_or_default()) {
        push_new(&mut dirs, &dir);
    }
    for dir in ld_so_conf_dirs(machine, "/etc/ld.so.conf") {
        push_new(&mut dirs, &dir);
    }
    for dir in SYSTEM_DIRS {
        push_new(&mut dirs, dir);
    }
    dirs
}

/// The values the loader's `$LIB` token can take here: each search directory under `/usr` or `/`
/// as a relative path (`/usr/lib/x86_64-linux-gnu` -> `lib/x86_64-linux-gnu`).
fn lib_tokens(dirs: &[String]) -> Vec<String> {
    let mut tokens = Vec::new();
    for dir in dirs {
        let relative = dir.strip_prefix("/usr/").or_else(|| dir.strip_prefix('/')).unwrap_or_default();
        if relative.starts_with("lib") {
            push_new(&mut tokens, relative);
        }
    }
    tokens
}

/// The JACK directories a `pw-jack` script names: every absolute path in it (split at quotes,
/// `=`, `:`, `;` and white space), with `${LIB}` or `$LIB` expanded to each of `lib_tokens`, that
/// holds a `libjack.so.0`. Paths with any other `$` are skipped (they are the user's variables), and
/// so is a plain loader directory among `dirs`: its `libjack.so.0` is the system's (jackd2's), not
/// PipeWire's.
pub fn pw_jack_dirs(machine: &dyn MachineView, script: &str, dirs: &[String]) -> Vec<String> {
    let tokens = lib_tokens(dirs);
    let mut found = Vec::new();
    for word in script.split(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '=' | ':' | ';')) {
        let word = word.trim_end_matches('/');
        if !word.starts_with('/') {
            continue;
        }
        let expanded: Vec<String> = if word.contains("${LIB}") || word.contains("$LIB") {
            tokens.iter().map(|lib| word.replace("${LIB}", lib).replace("$LIB", lib)).collect()
        } else {
            vec![word.to_owned()]
        };
        for dir in expanded.into_iter().filter(|d| !d.contains('$') && !dirs.contains(d)) {
            if machine.canonical(&format!("{dir}/{LIBJACK_FILE}")).is_some() {
                push_new(&mut found, &dir);
            }
        }
    }
    found
}

/// The text of the first `pw-jack` on `PATH`.
fn pw_jack_script(machine: &dyn MachineView) -> Option<String> {
    let path = machine.var("PATH")?;
    path.split(':').filter(|d| d.starts_with('/')).find_map(|dir| machine.read(&format!("{}/pw-jack", dir.trim_end_matches('/'))))
}

/// PipeWire's libjack wherever this machine has it: the directories `pw-jack` names first, then
/// [`PIPEWIRE_JACK_SUBDIR`] under each of the loader's directories. Each library that exists, once
/// (two spellings of one file, such as `/lib/...` and `/usr/lib/...` on a merged-`/usr` system,
/// count once: the first spelling is kept).
pub fn pipewire_libraries(machine: &dyn MachineView) -> Vec<String> {
    let dirs = loader_dirs(machine);
    let mut jack_dirs = pw_jack_script(machine).map(|script| pw_jack_dirs(machine, &script, &dirs)).unwrap_or_default();
    for dir in &dirs {
        let joined = if dir == "/" { format!("/{PIPEWIRE_JACK_SUBDIR}") } else { format!("{dir}/{PIPEWIRE_JACK_SUBDIR}") };
        push_new(&mut jack_dirs, &joined);
    }
    let mut libraries = Vec::new();
    let mut seen = Vec::new();
    for dir in jack_dirs {
        let library = format!("{dir}/{LIBJACK_FILE}");
        let Some(canonical) = machine.canonical(&library) else { continue };
        if !seen.contains(&canonical) {
            seen.push(canonical);
            libraries.push(library);
        }
    }
    libraries
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A scripted machine: variables, text files, directory listings and the files that exist
    /// (each with its canonical path).
    #[derive(Default)]
    struct Machine {
        vars: HashMap<&'static str, &'static str>,
        files: HashMap<&'static str, &'static str>,
        dirs: HashMap<&'static str, Vec<&'static str>>,
        libraries: HashMap<&'static str, &'static str>,
    }

    impl MachineView for Machine {
        fn var(&self, name: &str) -> Option<String> {
            self.vars.get(name).map(|v| (*v).to_owned())
        }
        fn read(&self, path: &str) -> Option<String> {
            self.files.get(path).map(|v| (*v).to_owned())
        }
        fn list(&self, dir: &str) -> Vec<String> {
            self.dirs.get(dir).map(|d| d.iter().map(|e| (*e).to_owned()).collect()).unwrap_or_default()
        }
        fn canonical(&self, path: &str) -> Option<String> {
            self.libraries.get(path).map(|c| (*c).to_owned())
        }
    }

    const MAPS: &str = "\
55d0c8a00000-55d0c8a01000 r--p 00000000 08:01 131 /opt/transit/bin/transit
7f1c2a000000-7f1c2a028000 r--p 00000000 08:01 262 /usr/lib/x86_64-linux-gnu/libc.so.6
7f1c2a200000-7f1c2a201000 rw-p 00000000 00:00 0
7f1c2a300000-7f1c2a320000 r-xp 00000000 08:01 263 /usr/lib/x86_64-linux-gnu/libm.so.6
7ffd1c000000-7ffd1c021000 rw-p 00000000 00:00 0 [stack]
7f1c2a400000-7f1c2a420000 r-xp 00000000 08:01 264 /usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2
";

    #[test]
    fn the_mapped_objects_give_the_loaders_library_directory() {
        assert_eq!(mapped_dirs(MAPS), ["/usr/lib/x86_64-linux-gnu"], "the binary, anonymous maps and [stack] are not libraries");
        assert_eq!(mapped_dirs("7f00-7f01 r--p 0 08:01 9 /usr/lib64/libc.so.6\n"), ["/usr/lib64"]);
        assert_eq!(mapped_dirs("7f00-7f01 r--p 0 08:01 9 /opt/my libs/libx.so (deleted)\n"), ["/opt/my libs"], "a path with spaces");
    }

    #[test]
    fn ld_so_conf_includes_are_followed_in_glob_order() {
        let machine = Machine {
            files: HashMap::from([
                ("/etc/ld.so.conf", "include /etc/ld.so.conf.d/*.conf\n/opt/extra/lib # local\n"),
                (
                    "/etc/ld.so.conf.d/x86_64-linux-gnu.conf",
                    "# Multiarch support\n/usr/local/lib/x86_64-linux-gnu\n/lib/x86_64-linux-gnu\n/usr/lib/x86_64-linux-gnu\n",
                ),
                ("/etc/ld.so.conf.d/libc.conf", "/usr/local/lib\n"),
                ("/etc/ld.so.conf.d/loop.conf", "include /etc/ld.so.conf\n"),
            ]),
            dirs: HashMap::from([("/etc/ld.so.conf.d", vec!["x86_64-linux-gnu.conf", "libc.conf", "README", "loop.conf"])]),
            ..Machine::default()
        };
        assert_eq!(
            ld_so_conf_dirs(&machine, "/etc/ld.so.conf"),
            ["/usr/local/lib", "/usr/local/lib/x86_64-linux-gnu", "/lib/x86_64-linux-gnu", "/usr/lib/x86_64-linux-gnu", "/opt/extra/lib"],
            "sorted includes first, a cycle stops, comments and non-.conf entries skipped"
        );
    }

    #[test]
    fn the_loader_dirs_are_ld_library_path_then_mapped_then_ld_so_conf_then_the_system() {
        let machine = Machine {
            vars: HashMap::from([("LD_LIBRARY_PATH", "/home/u/lib::relative:/usr/lib/x86_64-linux-gnu/")]),
            files: HashMap::from([("/proc/self/maps", MAPS), ("/etc/ld.so.conf", "/opt/extra/lib\n")]),
            ..Machine::default()
        };
        assert_eq!(loader_dirs(&machine), ["/home/u/lib", "/usr/lib/x86_64-linux-gnu", "/opt/extra/lib", "/lib64", "/usr/lib64", "/lib", "/usr/lib"]);
    }

    /// Debian and Ubuntu's `pw-jack` writes the loader's `$LIB` token; upstream's a literal path.
    const PW_JACK_LIB_TOKEN: &str = "#!/bin/sh\n# PipeWire JACK wrapper\nexport LD_LIBRARY_PATH='/usr/${LIB}/pipewire-0.3/jack'\"${LD_LIBRARY_PATH+\":$LD_LIBRARY_PATH\"}\"\nexec \"$@\"\n";
    const PW_JACK_LITERAL: &str = "#!/bin/sh\nPW_JACK_LIBDIR=\"/opt/pw/lib/pipewire-jack\"\nexport LD_LIBRARY_PATH=\"$PW_JACK_LIBDIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}\"\nexec \"$@\"\n";
    const UBUNTU_PW: &str = "/usr/lib/x86_64-linux-gnu/pipewire-0.3/jack/libjack.so.0";
    const UBUNTU_PW_FILE: &str = "/usr/lib/x86_64-linux-gnu/pipewire-0.3/jack/libjack.so.0.3.1600";

    #[test]
    fn pw_jacks_directory_is_read_from_the_script_with_lib_expanded() {
        let machine = Machine {
            libraries: HashMap::from([
                (UBUNTU_PW, UBUNTU_PW_FILE),
                ("/opt/pw/lib/pipewire-jack/libjack.so.0", "/opt/pw/lib/pipewire-jack/libjack.so.0"),
            ]),
            ..Machine::default()
        };
        let dirs = ["/usr/lib/x86_64-linux-gnu".to_owned(), "/lib64".to_owned(), "/usr/lib".to_owned()];
        assert_eq!(pw_jack_dirs(&machine, PW_JACK_LIB_TOKEN, &dirs), ["/usr/lib/x86_64-linux-gnu/pipewire-0.3/jack"]);
        assert_eq!(
            pw_jack_dirs(&machine, PW_JACK_LITERAL, &dirs),
            ["/opt/pw/lib/pipewire-jack"],
            "a custom libjack-path, and /bin/sh holds no libjack"
        );
        let jackd2 = Machine {
            libraries: HashMap::from([("/usr/lib/x86_64-linux-gnu/libjack.so.0", "/usr/lib/x86_64-linux-gnu/libjack.so.0.1.0")]),
            ..Machine::default()
        };
        let script = "#!/bin/sh
LIBDIR=/usr/lib/x86_64-linux-gnu
";
        assert!(pw_jack_dirs(&jackd2, script, &dirs).is_empty(), "a plain library directory holds jackd2's libjack, not PipeWire's");
    }

    /// The four machines of #430, as the candidate list sees them (`crate::library::candidates`
    /// orders these with the soname).
    #[test]
    fn pipewires_library_is_found_without_a_distribution_path_in_transit() {
        let maps_only = |libraries: HashMap<&'static str, &'static str>, pw_jack: bool| Machine {
            vars: HashMap::from([("PATH", "/usr/local/bin:/usr/bin")]),
            files: if pw_jack {
                HashMap::from([("/proc/self/maps", MAPS), ("/usr/bin/pw-jack", PW_JACK_LIB_TOKEN)])
            } else {
                HashMap::from([("/proc/self/maps", MAPS)])
            },
            libraries,
            ..Machine::default()
        };
        // Ubuntu with pipewire-jack (with or without jackd2 beside it): found through pw-jack, and
        // again under the mapped libc's directory, which is the same file: listed once.
        assert_eq!(pipewire_libraries(&maps_only(HashMap::from([(UBUNTU_PW, UBUNTU_PW_FILE)]), true)), [UBUNTU_PW]);
        // pipewire-jack's library without its pw-jack script: the loader's directories find it.
        assert_eq!(pipewire_libraries(&maps_only(HashMap::from([(UBUNTU_PW, UBUNTU_PW_FILE)]), false)), [UBUNTU_PW]);
        // jackd2 only, or neither: no PipeWire library.
        assert!(pipewire_libraries(&maps_only(HashMap::new(), true)).is_empty());
        // Fedora's layout (lib64), no pw-jack on PATH, merged /usr: one file under two spellings.
        let fedora = Machine {
            files: HashMap::from([("/proc/self/maps", "7f00-7f01 r--p 0 08:01 9 /usr/lib64/libc.so.6\n")]),
            libraries: HashMap::from([
                ("/usr/lib64/pipewire-0.3/jack/libjack.so.0", "/usr/lib64/pipewire-0.3/jack/libjack.so.0.3.1600"),
                ("/lib64/pipewire-0.3/jack/libjack.so.0", "/usr/lib64/pipewire-0.3/jack/libjack.so.0.3.1600"),
            ]),
            ..Machine::default()
        };
        assert_eq!(pipewire_libraries(&fedora), ["/usr/lib64/pipewire-0.3/jack/libjack.so.0"]);
    }
}
