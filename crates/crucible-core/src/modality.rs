//! What a model will accept, and what a provider can spell.
//!
//! Five words, and the set is closed: they are exactly the input vocabulary of
//! the model database this project generates its capability table from, read
//! across every model in it. A sixth word appearing there is a change to what
//! crucible can be asked about, so it belongs in a diff somebody reads rather
//! than in a string that silently fails to match — which is why this is an enum
//! and why nothing here ends a `match` with a wildcard.
//!
//! `Text` is a variant although every model has it. Leaving it out would make
//! this a different set from the one it is generated against, and the generator
//! would need private knowledge of which word to throw away.

use std::str::FromStr;

/// One kind of content a model reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Modality {
    /// Prose, code, and everything else that is already characters.
    Text,
    /// A picture, whatever it is a picture of.
    Image,
    /// A PDF, which vendors carry as a document rather than as its pages.
    Pdf,
    /// Moving pictures, carried whole.
    Video,
    /// Sound.
    Audio,
}

impl Modality {
    /// Every modality, in the order they are declared.
    pub const EVERY: [Self; 5] = [Self::Text, Self::Image, Self::Pdf, Self::Video, Self::Audio];

    /// The word the database uses, which is the word crucible uses.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Pdf => "pdf",
            Self::Video => "video",
            Self::Audio => "audio",
        }
    }

    /// Where this modality sits in a set.
    const fn bit(self) -> u8 {
        match self {
            Self::Text => 0b0_0001,
            Self::Image => 0b0_0010,
            Self::Pdf => 0b0_0100,
            Self::Video => 0b0_1000,
            Self::Audio => 0b1_0000,
        }
    }
}

/// A word where a modality was expected.
///
/// Names what was read and then the whole vocabulary, because the reader is a
/// generator walking a database file: the useful thing to know is not that one
/// word failed but that the set it was checked against has these five members.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no modality called {named}; crucible knows {}", Modality::EVERY.map(Modality::as_str).join(", "))]
pub struct ModalityError {
    /// What was read.
    pub named: Box<str>,
}

impl FromStr for Modality {
    type Err = ModalityError;

    /// Exact, where [`Effort`](crate::Effort) is forgiving. Nobody types this:
    /// it is read out of a generated database, so a word in another case is
    /// that vocabulary having moved, and the useful answer to a vocabulary that
    /// moved is a failed build rather than a quiet match.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::EVERY
            .into_iter()
            .find(|one| one.as_str() == text)
            .ok_or_else(|| ModalityError { named: text.into() })
    }
}

/// A set of modalities, small enough to copy and to build in a `const`.
///
/// There are five possible members and this is read where a request is being
/// shaped, so it is a handful of bits rather than a `Vec` somebody allocates
/// per turn.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Modalities(u8);

impl Modalities {
    /// A set with nothing in it — the seed every declaration is built from.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// The same set with one more member. Consuming, so a declaration reads as
    /// one expression and can be a `const`.
    #[must_use]
    pub const fn insert(self, one: Modality) -> Self {
        Self(self.0 | one.bit())
    }

    /// Whether this set has that member.
    #[must_use]
    pub const fn contains(self, one: Modality) -> bool {
        self.0 & one.bit() != 0
    }

    /// What both sets have — the whole of what may be attached, where one side
    /// is what the model accepts and the other is what the provider can spell.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Whether nothing is in it.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The members, in the order the variants are declared, so a sentence
    /// listing them reads the same however the set was built.
    pub fn iter(self) -> impl Iterator<Item = Modality> {
        Modality::EVERY
            .into_iter()
            .filter(move |one| self.contains(*one))
    }
}

/// Written as its members rather than as its bits, because the number means
/// nothing to whoever is reading a failed assertion.
impl std::fmt::Debug for Modalities {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Modalities(")?;
        for (nth, one) in self.iter().enumerate() {
            if nth > 0 {
                out.write_str(", ")?;
            }
            out.write_str(one.as_str())?;
        }
        out.write_str(")")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_modality_is_read_back_from_the_word_it_is_written_as() {
        for one in Modality::EVERY {
            assert_eq!(one.as_str().parse(), Ok(one));
        }
    }

    #[test]
    fn a_word_the_database_does_not_use_is_refused_with_the_five_that_are() {
        // The reader is a generator walking a database file, so the sentence
        // has to say what the vocabulary is rather than only that this missed.
        let refused = "picture".parse::<Modality>().expect_err("not a modality");

        assert_eq!(
            refused.to_string(),
            "no modality called picture; crucible knows text, image, pdf, video, audio"
        );
    }

    #[test]
    fn a_word_in_another_case_is_refused_rather_than_matched() {
        // Nothing types these. A capital is the database's vocabulary having
        // moved, and that is a build to fix rather than a case to tolerate.
        assert!("Image".parse::<Modality>().is_err());
    }

    #[test]
    fn a_set_holds_what_was_put_in_it_and_nothing_else() {
        let takes = Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image);

        assert!(takes.contains(Modality::Text));
        assert!(takes.contains(Modality::Image));
        assert!(!takes.contains(Modality::Pdf));
        assert!(!takes.is_empty());
    }

    #[test]
    fn two_sets_with_nothing_in_common_intersect_to_nothing() {
        let model = Modalities::empty().insert(Modality::Video);
        let provider = Modalities::empty().insert(Modality::Pdf);

        assert!(model.intersection(provider).is_empty());
    }

    #[test]
    fn an_intersection_keeps_only_what_both_sides_have() {
        // The whole of the capability answer: what the model accepts times
        // what the provider can spell, and neither half alone.
        let model = Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Pdf);
        let provider = Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Video);

        let both = model.intersection(provider);

        assert_eq!(
            both.iter().collect::<Vec<_>>(),
            vec![Modality::Text, Modality::Image]
        );
    }

    #[test]
    fn a_set_reads_back_in_variant_order_however_it_was_built() {
        let backwards = Modalities::empty()
            .insert(Modality::Audio)
            .insert(Modality::Image);

        assert_eq!(
            backwards.iter().collect::<Vec<_>>(),
            vec![Modality::Image, Modality::Audio]
        );
    }

    #[test]
    fn a_set_shows_its_members_rather_than_its_bits() {
        let takes = Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Pdf);

        assert_eq!(format!("{takes:?}"), "Modalities(text, pdf)");
    }
}
