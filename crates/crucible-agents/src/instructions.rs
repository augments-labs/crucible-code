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
///
/// How many of its last bytes somebody appended to crucible's own is kept
/// beside the text, so a reading of the request can tell the two apart. It is
/// held to the text it ends: a count longer than the text would say more was
/// appended than was sent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Instructions {
    text: Option<Box<str>>,
    appended: usize,
}

impl Instructions {
    /// Nobody wrote any.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            text: None,
            appended: 0,
        }
    }

    /// What somebody wrote, reading nothing written as nothing said.
    #[must_use]
    pub fn said(text: &str) -> Self {
        Self {
            text: (!text.is_empty()).then(|| text.into()),
            appended: 0,
        }
    }

    /// The same, its last `bytes` appended by the user or the checkout.
    #[must_use]
    pub fn ending_with(self, bytes: usize) -> Self {
        let appended = self.text.as_deref().map_or(0, |text| bytes.min(text.len()));
        Self { appended, ..self }
    }

    /// The sentence, or `None` where nobody wrote one.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// How many of the last bytes of [`Self::text`] were appended to
    /// crucible's own.
    #[must_use]
    pub const fn appended(&self) -> usize {
        self.appended
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

    #[test]
    fn what_was_appended_is_counted_and_held_to_the_text_it_ends() {
        let said = Instructions::said("crucible's own\nproject rules");

        assert_eq!(said.clone().appended(), 0);
        assert_eq!(said.clone().ending_with(13).appended(), 13);
        assert_eq!(said.ending_with(999).appended(), 28);
        assert_eq!(Instructions::said("").ending_with(5).appended(), 0);
    }
}
