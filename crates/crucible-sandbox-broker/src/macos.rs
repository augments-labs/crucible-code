//! Single-threaded setup before the system Seatbelt launcher takes control.

use std::ffi::{OsStr, OsString};
use std::io::{self, Write as _};
use std::os::unix::process::CommandExt as _;
use std::process::{Command, ExitCode};

use rustix::process::{Resource, Rlimit};

use crate::BROKER_ERROR_EXIT;

const SEATBELT: &str = "/usr/bin/sandbox-exec";
const MAX_PROFILE_BYTES: usize = 256 * 1024;
const MAX_DEFINITIONS: usize = 256;

pub(super) fn launch() -> ExitCode {
    match Request::parse(std::env::args_os().skip(2)) {
        Ok(request) => request.exec(),
        Err(problem) => {
            let _ = writeln!(std::io::stderr(), "macOS sandbox launch refused: {problem}");
            ExitCode::from(BROKER_ERROR_EXIT)
        }
    }
}

struct Request {
    cpu_seconds: u64,
    open_files: u64,
    profile: String,
    definitions: Vec<OsString>,
    program: OsString,
    arguments: Vec<OsString>,
}

impl Request {
    fn parse(mut arguments: impl Iterator<Item = OsString>) -> io::Result<Self> {
        named(&mut arguments, "--cpu-seconds")?;
        let cpu_seconds = limit_number(&value(&mut arguments, "CPU limit")?)?;
        named(&mut arguments, "--open-files")?;
        let open_files = limit_number(&value(&mut arguments, "open-file limit")?)?;
        named(&mut arguments, "--profile")?;
        let profile = value(&mut arguments, "Seatbelt profile")?
            .into_string()
            .map_err(|_| invalid("Seatbelt profile is not UTF-8"))?;
        if profile.is_empty() || profile.len() > MAX_PROFILE_BYTES {
            return Err(invalid("Seatbelt profile is empty or oversized"));
        }

        let mut definitions = Vec::new();
        let program = loop {
            let argument = value(&mut arguments, "launcher separator")?;
            if argument == OsStr::new("--") {
                break value(&mut arguments, "sandboxed program")?;
            }
            if argument != OsStr::new("--definition") {
                return Err(invalid("unexpected macOS launcher argument"));
            }
            if definitions.len() >= MAX_DEFINITIONS {
                return Err(invalid("too many Seatbelt definitions"));
            }
            let definition = value(&mut arguments, "Seatbelt definition")?;
            if !has_definition_shape(&definition) {
                return Err(invalid("invalid Seatbelt definition"));
            }
            definitions.push(definition);
        };
        if program.is_empty() {
            return Err(invalid("sandboxed program is empty"));
        }
        let arguments = arguments.collect();
        Ok(Self {
            cpu_seconds,
            open_files,
            profile,
            definitions,
            program,
            arguments,
        })
    }

    fn exec(self) -> ExitCode {
        let descriptors = match inherited_descriptors() {
            Ok(descriptors) => descriptors,
            Err(problem) => return refused("descriptor inventory", &problem),
        };
        if self.cpu_seconds > 0
            && let Err(problem) = hard_limit(Resource::Cpu, self.cpu_seconds)
        {
            return refused("CPU limit", &problem);
        }
        if self.open_files > 0
            && let Err(problem) = hard_limit(Resource::Nofile, self.open_files)
        {
            return refused("open-file limit", &problem);
        }
        for descriptor in descriptors {
            // SAFETY: the single-threaded helper owns no descriptor above
            // standard input/output/error after its directory iterator drops.
            unsafe { rustix::io::close(descriptor) };
        }

        let mut command = Command::new(SEATBELT);
        command.arg("-p").arg(self.profile);
        for definition in self.definitions {
            command.arg("-D").arg(definition);
        }
        let problem = command
            .arg("--")
            .arg(self.program)
            .args(self.arguments)
            .exec();
        refused("Seatbelt exec", &problem)
    }
}

fn named(arguments: &mut impl Iterator<Item = OsString>, expected: &str) -> io::Result<()> {
    let argument = value(arguments, expected)?;
    if argument == OsStr::new(expected) {
        Ok(())
    } else {
        Err(invalid("macOS launcher argument order is invalid"))
    }
}

fn value(
    arguments: &mut impl Iterator<Item = OsString>,
    description: &str,
) -> io::Result<OsString> {
    arguments
        .next()
        .ok_or_else(|| invalid(&format!("missing {description}")))
}

fn limit_number(value: &OsStr) -> io::Result<u64> {
    let parsed = value
        .to_str()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| invalid("resource limit is not an unsigned integer"))?;
    Ok(parsed)
}

fn has_definition_shape(definition: &OsStr) -> bool {
    let bytes = definition.as_encoded_bytes();
    let Some(separator) = bytes.iter().position(|byte| *byte == b'=') else {
        return false;
    };
    separator > 0
        && separator <= 32
        && bytes.get(..separator).is_some_and(|key| {
            key.iter()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_')
        })
        && bytes
            .get(separator.saturating_add(1)..)
            .is_some_and(|value| !value.is_empty())
}

fn inherited_descriptors() -> io::Result<Vec<i32>> {
    let mut descriptors = Vec::new();
    for entry in std::fs::read_dir("/dev/fd")? {
        let entry = entry?;
        let Some(descriptor) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        if descriptor > 2 {
            descriptors.push(descriptor);
        }
    }
    descriptors.sort_unstable();
    descriptors.dedup();
    Ok(descriptors)
}

fn hard_limit(resource: Resource, value: u64) -> io::Result<()> {
    rustix::process::setrlimit(
        resource,
        Rlimit {
            current: Some(value),
            maximum: Some(value),
        },
    )
    .map_err(io::Error::from)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_owned())
}

fn refused(stage: &str, problem: &io::Error) -> ExitCode {
    let _ = writeln!(
        std::io::stderr(),
        "macOS sandbox launch refused during {stage}: {:?}",
        problem.kind()
    );
    ExitCode::from(BROKER_ERROR_EXIT)
}
