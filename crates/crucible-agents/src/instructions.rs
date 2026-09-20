//! The sentence an agent is asked under, and the difference between none and
//! empty.

/// The stable operator instructions a definition stands under, if any.
///
/// Nothing said and an empty thing said are two different requests: one carries
/// no system field at all, the other carries a field holding nothing. Only the
/// first is what "nobody wrote one" means, and a vendor reads the two
/// differently. The rule lives on this type rather than in prose beside a
/// field, because a rule stated in prose holds wherever somebody remembered it.
///
/// Model, effort, tool, permission and workspace facts are typed context
/// sections assembled per pass. None of them belongs here: these are the bytes
/// that stay the same while those change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Instructions(Option<Box<str>>);

impl Instructions {
    /// Nobody wrote any.
    #[must_use]
    pub const fn none() -> Self {
        Self(None)
    }

    /// What somebody wrote, reading nothing written as nothing said.
    #[must_use]
    pub fn said(text: &str) -> Self {
        Self((!text.is_empty()).then(|| text.into()))
    }

    /// The sentence, or `None` where nobody wrote one.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::Instructions;

    #[test]
    fn nothing_said_is_not_the_empty_string() {
        assert_eq!(Instructions::said("").text(), None);
        assert_eq!(Instructions::none().text(), None);
    }

    #[test]
    fn what_was_said_is_what_comes_back() {
        assert_eq!(
            Instructions::said("mind the workspace").text(),
            Some("mind the workspace")
        );
    }
}
