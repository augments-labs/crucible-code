//! The releases of Claude Code this build was reviewed with.
//!
//! The agent is run only as a release someone read, named by a range of
//! versions: its stream protocol, its permission requests and the figures its
//! `result` frame carries were reviewed at the range's lower bound and at the
//! release current when it was written. The version is read from what the
//! program prints when asked, once per connect and never per turn, and a
//! version outside the range, or one printed otherwise, is refused.

use crucible_execution::{Failure, Text};

/// A version as three numbers.
type Version = (u32, u32, u32);

/// The reviewed releases of the agent.
#[derive(Debug, Clone, Copy)]
pub struct Manifest {
    /// The name the program prints after its version.
    pub name: &'static str,
    /// The first version reviewed.
    pub lowest: Version,
    /// The first version past the reviewed ones.
    pub below: Version,
}

impl Manifest {
    /// The releases this build was reviewed with.
    pub const REVIEWED: Self = Self {
        name: "Claude Code",
        lowest: (2, 1, 277),
        below: (2, 2, 0),
    };

    /// The version `said`, the first line the program printed when asked,
    /// where it is one this manifest lists.
    ///
    /// # Errors
    ///
    /// A [`Failure`] naming the range, where the line is not a version and
    /// this program's name, or the version is outside the range.
    pub fn admits(&self, said: &str) -> Result<Text, Failure> {
        let (lowest, below) = (written(self.lowest), written(self.below));
        let printed = said
            .trim()
            .strip_suffix(&format!("({})", self.name))
            .map(str::trim_end)
            .ok_or_else(|| {
                Failure::new(&format!("The program did not say it is {}.", self.name))
            })?;
        match version(printed) {
            Some(version) if (self.lowest..self.below).contains(&version) => Ok(Text::new(printed)),
            Some(_) => Err(Failure::new(&format!(
                "Version {printed} of the agent was not reviewed for this release of Crucible, \
                 which runs {lowest} and later before {below}."
            ))),
            None => Err(Failure::new(&format!(
                "The agent printed a version that is not three numbers; this release of \
                 Crucible runs {lowest} and later before {below}."
            ))),
        }
    }
}

/// `printed` as three numbers, where it is three numbers and nothing else.
fn version(printed: &str) -> Option<Version> {
    let mut numbers = printed.split('.').map(|number| {
        number
            .bytes()
            .all(|digit| digit.is_ascii_digit())
            .then(|| number.parse::<u32>().ok())
            .flatten()
    });
    let version = (numbers.next()??, numbers.next()??, numbers.next()??);
    numbers.next().is_none().then_some(version)
}

/// `version` as it is written.
fn written((major, minor, patch): Version) -> String {
    format!("{major}.{minor}.{patch}")
}
