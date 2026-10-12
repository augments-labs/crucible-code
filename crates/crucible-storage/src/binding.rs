//! What a session records about an installed agent that ran it, as records
//! naming no agent.
//!
//! A session run by an installed agent rather than by a provider's model
//! holds three facts a store keeps and a resume reads: the route it was
//! opened on, the agent's own name for the session, and a turn that ended
//! with its effects not known. Each is a plain bounded value here, so a store
//! keeps it without linking whatever ran the agent, and nothing here decides
//! what a resume does with it.
//!
//! A [`RouteBinding`] holds the agent's driver, its configured instance, the
//! profile directory as configured, a digest of the program it resolved to,
//! the version it reported and the generation of the run. It never holds an
//! environment, a command line or a credential: what a resume compares is
//! which route, and none of those says which. Two bindings are the
//! [same route](RouteBinding::same_route) when the first four agree; a version
//! or a generation that moved is the same agent run again.

use std::fmt;
use std::path::Path;

use sha2::{Digest as _, Sha256};

/// The most bytes a driver, instance or version keeps.
pub const BINDING_NAME_BYTES: usize = 256;

/// The most bytes a profile directory, as configured, keeps.
pub const BINDING_PROFILE_BYTES: usize = 4096;

/// The most bytes an agent's name for its session keeps.
pub const EXTERNAL_REFERENCE_BYTES: usize = 256;

/// The most bytes of what an uncertain turn reported under way are kept.
pub const UNCERTAIN_BYTES: usize = 64 * 1024;

/// Why a binding could not be made from what it was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BindingError {
    /// A field that names something was empty.
    #[error("a session binding's {0} is empty")]
    Empty(&'static str),
    /// A field ran past its ceiling, so keeping a prefix would name something
    /// else.
    #[error("a session binding's {0} is longer than it may be")]
    TooLong(&'static str),
    /// A field held a control character, which a list or a message naming it
    /// would act on rather than show.
    #[error("a session binding's {0} holds a control character")]
    Control(&'static str),
}

/// Text kept whole up to a ceiling, and saying whether anything was left out.
#[derive(Clone, PartialEq, Eq)]
pub struct Clipped {
    /// Whole characters, at most the ceiling's bytes.
    text: Box<str>,
    /// Whether anything was left out.
    incomplete: bool,
}

impl Clipped {
    /// `text`, cut to at most `ceiling` bytes on a character boundary.
    #[must_use]
    pub fn new(text: &str, ceiling: usize) -> Self {
        let end = text.floor_char_boundary(ceiling);
        Self {
            text: text.get(..end).unwrap_or_default().into(),
            incomplete: end < text.len(),
        }
    }

    /// What was kept.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether anything was left out.
    #[must_use]
    pub const fn incomplete(&self) -> bool {
        self.incomplete
    }

    /// This, marked as missing something left out before it was kept.
    #[must_use]
    const fn missing(mut self, earlier: bool) -> Self {
        self.incomplete |= earlier;
        self
    }
}

impl fmt::Debug for Clipped {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Clipped")
            .field("text", &self.text)
            .field("incomplete", &self.incomplete)
            .finish()
    }
}

/// `text` as a field named `field`, refused rather than cut.
fn exact(field: &'static str, text: &str, ceiling: usize) -> Result<Box<str>, BindingError> {
    if text.is_empty() {
        Err(BindingError::Empty(field))
    } else if text.len() > ceiling {
        Err(BindingError::TooLong(field))
    } else if text.chars().any(char::is_control) {
        Err(BindingError::Control(field))
    } else {
        Ok(text.into())
    }
}

/// The route a session was opened on: which installed agent ran it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteBinding {
    /// The driver, as the agent registry names it.
    driver: Box<str>,
    /// The configured instance.
    instance: Box<str>,
    /// The profile directory, as configured.
    profile: Option<Box<str>>,
    /// A digest of where its program resolved to.
    executable: Option<[u8; 32]>,
    /// The version the program reported.
    version: Option<Clipped>,
    /// The generation of the run that opened it.
    generation: u64,
}

impl RouteBinding {
    /// Instance `instance` of `driver`, with `profile` as configured.
    ///
    /// # Errors
    ///
    /// [`BindingError`] where a name is empty, holds a control character or
    /// is longer than [`BINDING_NAME_BYTES`], or the profile is the same past
    /// [`BINDING_PROFILE_BYTES`]: each is compared on resume, and a prefix of
    /// one would compare equal to a route it is not.
    pub fn new(driver: &str, instance: &str, profile: Option<&str>) -> Result<Self, BindingError> {
        Ok(Self {
            driver: exact("driver", driver, BINDING_NAME_BYTES)?,
            instance: exact("instance", instance, BINDING_NAME_BYTES)?,
            profile: profile
                .map(|profile| exact("profile", profile, BINDING_PROFILE_BYTES))
                .transpose()?,
            executable: None,
            version: None,
            generation: 0,
        })
    }

    /// This binding, with its program resolved to `program`.
    ///
    /// Kept as a digest of the path, so the record says whether the program
    /// resolves where it did without saying where that is.
    #[must_use]
    pub fn resolved(self, program: &Path) -> Self {
        self.digested(Sha256::digest(program.as_os_str().as_encoded_bytes()).into())
    }

    /// This binding, with its program resolved to where `executable` is the
    /// digest of, as a store read it back.
    #[must_use]
    pub const fn digested(mut self, executable: [u8; 32]) -> Self {
        self.executable = Some(executable);
        self
    }

    /// This binding, with the program reporting `version`, cut to
    /// [`BINDING_NAME_BYTES`] and marked incomplete where it was cut or
    /// `incomplete` says it already was.
    #[must_use]
    pub fn reporting(mut self, version: &str, incomplete: bool) -> Self {
        self.version = Some(Clipped::new(version, BINDING_NAME_BYTES).missing(incomplete));
        self
    }

    /// This binding, opened by the run of generation `generation`.
    #[must_use]
    pub const fn generation_of(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }

    /// The driver, as the agent registry names it.
    #[must_use]
    pub fn driver(&self) -> &str {
        &self.driver
    }

    /// The configured instance.
    #[must_use]
    pub fn instance(&self) -> &str {
        &self.instance
    }

    /// The profile directory, as configured.
    #[must_use]
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    /// A digest of where its program resolved to.
    #[must_use]
    pub const fn executable(&self) -> Option<&[u8; 32]> {
        self.executable.as_ref()
    }

    /// The version the program reported.
    #[must_use]
    pub const fn version(&self) -> Option<&Clipped> {
        self.version.as_ref()
    }

    /// The generation of the run that opened it.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether `other` is this route: the same driver, instance, profile and
    /// program. The version and the generation are left out, because an
    /// agent updated or started again is the same agent.
    #[must_use]
    pub fn same_route(&self, other: &Self) -> bool {
        self.driver == other.driver
            && self.instance == other.instance
            && self.profile == other.profile
            && self.executable == other.executable
    }
}

/// An installed agent's own name for the session it is running, under the
/// instance that named it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalRef {
    /// The configured instance.
    instance: Box<str>,
    /// What the agent calls the session.
    reference: Box<str>,
}

impl ExternalRef {
    /// `reference`, as instance `instance` named its session.
    ///
    /// # Errors
    ///
    /// [`BindingError`] where either is empty or holds a control character,
    /// or the instance is longer than
    /// [`BINDING_NAME_BYTES`] or the reference than
    /// [`EXTERNAL_REFERENCE_BYTES`]: a prefix of a session's name is the name
    /// of no session, or of another.
    pub fn new(instance: &str, reference: &str) -> Result<Self, BindingError> {
        Ok(Self {
            instance: exact("instance", instance, BINDING_NAME_BYTES)?,
            reference: exact("reference", reference, EXTERNAL_REFERENCE_BYTES)?,
        })
    }

    /// The configured instance.
    #[must_use]
    pub fn instance(&self) -> &str {
        &self.instance
    }

    /// What the agent calls the session.
    #[must_use]
    pub fn reference(&self) -> &str {
        &self.reference
    }
}

/// A turn that ended without the agent saying whether what it set out to do
/// was done.
#[derive(Clone, PartialEq, Eq)]
pub struct Uncertain {
    /// What it reported under way when it stopped.
    under_way: Clipped,
}

impl Uncertain {
    /// A turn that stopped with `under_way` reported, cut to
    /// [`UNCERTAIN_BYTES`] and marked incomplete where it was cut or
    /// `incomplete` says it already was.
    #[must_use]
    pub fn new(under_way: &str, incomplete: bool) -> Self {
        Self {
            under_way: Clipped::new(under_way, UNCERTAIN_BYTES).missing(incomplete),
        }
    }

    /// What it reported under way when it stopped.
    #[must_use]
    pub const fn under_way(&self) -> &Clipped {
        &self.under_way
    }
}

/// Written by hand, because what an agent says it was doing can quote what it
/// read, and a `{:?}` reaches further than the log this is kept in.
impl fmt::Debug for Uncertain {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Uncertain")
            .field(
                "under_way",
                &format_args!("[{} bytes]", self.under_way.text.len()),
            )
            .field("incomplete", &self.under_way.incomplete)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_past_its_ceiling_is_refused_rather_than_cut() {
        let long = "a".repeat(BINDING_NAME_BYTES + 1);
        assert_eq!(
            RouteBinding::new("claude", &long, None),
            Err(BindingError::TooLong("instance"))
        );
        assert_eq!(
            RouteBinding::new("", "work", None),
            Err(BindingError::Empty("driver"))
        );
        let reference = "r".repeat(EXTERNAL_REFERENCE_BYTES + 1);
        assert_eq!(
            ExternalRef::new("work", &reference),
            Err(BindingError::TooLong("reference"))
        );
        assert_eq!(
            RouteBinding::new("claude", "wo\u{1b}[2Jrk", None),
            Err(BindingError::Control("instance"))
        );
        assert!(RouteBinding::new("claude", &"a".repeat(BINDING_NAME_BYTES), None).is_ok());
    }

    #[test]
    fn what_was_under_way_is_cut_on_a_character_and_says_so() {
        let said = format!("{}é", "a".repeat(UNCERTAIN_BYTES - 1));
        let uncertain = Uncertain::new(&said, false);
        assert_eq!(uncertain.under_way().as_str().len(), UNCERTAIN_BYTES - 1);
        assert!(uncertain.under_way().incomplete());
        assert!(Uncertain::new("editing", true).under_way().incomplete());
        assert!(!Uncertain::new("editing", false).under_way().incomplete());
        assert!(!format!("{uncertain:?}").contains("aaa"));
    }

    #[test]
    fn a_program_is_kept_as_where_it_resolved_and_not_the_path() {
        let here = RouteBinding::new("claude", "work", None)
            .unwrap()
            .resolved(Path::new("/usr/bin/claude"));
        let there = RouteBinding::new("claude", "work", None)
            .unwrap()
            .resolved(Path::new("/opt/claude"));
        assert!(!here.same_route(&there));
        assert!(!format!("{here:?}").contains("/usr/bin"));
    }

    #[test]
    fn a_route_is_its_driver_instance_profile_and_program() {
        let route = RouteBinding::new("claude", "work", Some("/p"))
            .unwrap()
            .digested([1; 32]);
        let again = route.clone().reporting("2.0", false).generation_of(7);
        assert!(route.same_route(&again));

        for other in [
            RouteBinding::new("antigravity", "work", Some("/p"))
                .unwrap()
                .digested([1; 32]),
            RouteBinding::new("claude", "home", Some("/p"))
                .unwrap()
                .digested([1; 32]),
            RouteBinding::new("claude", "work", None)
                .unwrap()
                .digested([1; 32]),
            RouteBinding::new("claude", "work", Some("/p"))
                .unwrap()
                .digested([2; 32]),
        ] {
            assert!(!route.same_route(&other), "{other:?}");
        }
    }
}
