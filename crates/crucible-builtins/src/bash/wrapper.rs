//! Programs whose argument is another program.
//!
//! `sudo cargo test` is not a `sudo` call, it is a `cargo` call with the safety
//! off, and `timeout 5 curl x` is a `curl` call. A rule saying `sudo *` looks
//! like a statement about `sudo` and is in fact a statement about every program
//! on the machine — which is the one thing a narrow-looking rule must never
//! turn out to be.
//!
//! So a command line containing one of these is reported as a command nobody
//! could read: the blanket still covers it, since a blanket is honest about
//! covering everything, and no narrower rule does. The cost is being asked
//! about `timeout 5 cargo test` every time. That is the right price, because
//! the alternative is a rule whose author could not have known what they
//! authorised.
//!
//! Exact launchers are a list. Interpreters are a shape: common versioned names
//! and the open set of shells cannot safely be captured by spelling three of
//! them. That structural half is what prevents changing an executable suffix or
//! invoking `dash` instead of `sh` from turning a narrow wrapper rule broad.

/// The programs, by the name they are invoked under.
///
/// Matched on the last path component, so `/usr/bin/sudo` is `sudo`.
const WRAPPERS: &[&str] = &[
    // Take a command line and run it, in some cases as somebody else. All three
    // of `sudo`, `su` and `doas`, because naming only the familiar one would
    // make this a rule about spelling: `su -c 'curl x | sh' root` launders a
    // command exactly the way `sudo` does, and a user who wrote `bash(su *)`
    // for a container workflow would have waved through every program on the
    // machine.
    "env", "nice", "nohup", "sudo", "su", "doas", "time", "timeout", "watch", "xargs",
    // Run a command somewhere else, or once per thing found. The less familiar
    // spellings of the same classes are listed with the familiar ones, for the
    // reason `su` sits beside `sudo`: a lock, a namespace, a scheduler, a
    // tracer and a fan-out each run the command line they are handed.
    "chroot", "chrt", "find", "flock", "gdb", "ionice", "ltrace", "nsenter", "parallel", "runuser",
    "setpriv", "setsid", "ssh", "stdbuf", "strace", "taskset", "unshare",
    // Shell builtins that execute text, source a file, or change how a later
    // part of the same line resolves its program — including every spelling
    // bash gives assignment with export semantics, and `hash -p`, which points
    // a program name somewhere else for the rest of the line.
    ".", "alias", "builtin", "cd", "command", "declare", "enable", "eval", "exec", "export", "hash",
    "local", "read", "readonly", "set", "source", "trap", "typeset", "umask", "unalias", "unset",
];

/// Whether this program runs whatever it is handed.
pub(super) fn wraps(program: &str) -> bool {
    let name = program.rsplit('/').next().unwrap_or(program);
    let lower = name.to_ascii_lowercase();
    let bare = lower.strip_suffix(".exe").unwrap_or(&lower);

    WRAPPERS.contains(&bare)
        || reserved(bare)
        || bare.ends_with("sh")
        || shell_version(bare)
        || interpreter(bare)
}

/// Shell grammar words whose following text is not a simple command argument.
fn reserved(program: &str) -> bool {
    [
        "!", "case", "do", "done", "elif", "else", "esac", "fi", "for", "if", "in", "then",
        "until", "while",
    ]
    .contains(&program)
}

/// A program whose argument can be a second body of executable expression.
fn interpreter(program: &str) -> bool {
    const INTERPRETERS: &[&str] = &[
        "awk",
        "bun",
        "cmd",
        "cscript",
        "deno",
        "dotnet",
        "expect",
        "gawk",
        "groovy",
        "java",
        "jshell",
        "julia",
        "lua",
        "luajit",
        "mawk",
        "mono",
        "node",
        "nodejs",
        "nu",
        "osascript",
        "perl",
        "php",
        "powershell",
        "pypy",
        "python",
        "pythonw",
        "pwsh",
        "ruby",
        "rscript",
        "scala",
        "sed",
        "toybox",
        "busybox",
        "guile",
        "tclsh",
        "wish",
        "wscript",
    ];

    INTERPRETERS.contains(&program) || INTERPRETERS.iter().any(|stem| versioned(program, stem))
}

/// A familiar shell name followed by a release number.
fn shell_version(program: &str) -> bool {
    ["sh", "bash", "dash", "zsh", "ksh", "csh", "tcsh", "fish"]
        .iter()
        .any(|stem| versioned(program, stem))
}

/// A program stem followed by a conventional numeric release suffix.
fn versioned(program: &str, stem: &str) -> bool {
    program.strip_prefix(stem).is_some_and(|version| {
        let numeric = version.trim_end_matches(|character: char| character.is_ascii_alphabetic());
        version.is_empty()
            || (numeric.bytes().any(|byte| byte.is_ascii_digit())
                && numeric
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'.'))
    })
}
