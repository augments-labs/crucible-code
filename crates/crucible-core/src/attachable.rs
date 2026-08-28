//! What may be attached to a request, and what says a file is one.
//!
//! A closed table rather than a guess from an extension, and the bytes are read
//! back against it: the cost of being wrong is a request the user paid for and
//! a provider refused. It lives here because two callers ask the same question
//! about the same files — the prompt, where a person names one, and the `read`
//! tool, where a model does — and one of them getting a different answer would
//! mean a file that can be typed and cannot be read, or the reverse.
//!
//! What this does *not* decide is whether the request being built can carry the
//! kind it names. That is the model's half and the provider's, settled per
//! request, and it is not a property of the file.

use crate::Modality;

/// The most raw attachment bytes one request may carry.
///
/// Not a vendor's limit — this one binds first. What a request peaks at is
/// measured rather than derived: `scripts/bench.sh mem` runs a session at this
/// ceiling every time it runs, and reads about three times this figure on top
/// of what the session was already holding. The bytes, their base64 form and
/// the serialized body are alive at once, and the last two each hold the
/// encoding whole.
///
/// The figure this replaced was half as much again, from a reading of the code
/// that counted one of those copies and not the other. At that size the same
/// measurement swung between three times and four from run to run, which is the
/// other half of why this one is lower — a reading that moves by seven
/// megabytes needs somewhere to move to.
///
/// The worst a session is otherwise holding is its record full, at about 14 MB.
/// This is deliberately below what the rest of the 35 MB in
/// `performance-budgets.md` allows. A budget spent to its last megabyte is not
/// a budget.
///
/// A single file larger than this can never be carried whatever else a request
/// holds, which is what lets a caller refuse one before it has read the bytes.
pub const CEILING: usize = 4 * 1024 * 1024;

/// The kind a path's extension names, where it names one this build attaches.
///
/// The name is being asked what somebody meant by it. Whether the bytes agree
/// is [`Kind::confirms`], asked separately and after, because the two failures
/// are different sentences.
#[must_use]
pub fn kind(word: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|kind| kind.names(word))
}

/// One kind of file that may be attached, under the name it goes by.
#[derive(Debug)]
pub struct Kind {
    /// The extension, without its dot, as a prompt would spell it.
    pub extension: &'static str,
    /// What the model would be asked to do with it.
    pub modality: Modality,
    /// What the provider labels the bytes with.
    pub media_type: &'static str,
    /// Whether the bytes are what the extension claims.
    pub confirms: fn(&[u8]) -> bool,
}

impl Kind {
    /// Whether a word in a prompt is a path spelled with this extension.
    ///
    /// Case-insensitive on the extension alone: a camera writes `IMG_0001.JPG`
    /// and a person types what the camera wrote.
    #[must_use]
    pub fn names(&self, word: &str) -> bool {
        word.rsplit_once('.')
            .is_some_and(|(_, tail)| tail.eq_ignore_ascii_case(self.extension))
    }

    /// The kind as it appears mid-sentence, with the article English wants.
    #[must_use]
    pub fn spoken(&self) -> String {
        let article = match self.modality {
            Modality::Image | Modality::Audio => "an",
            Modality::Text | Modality::Pdf | Modality::Video => "a",
        };

        format!("{article} {}", self.modality.as_str())
    }
}

/// Every kind crucible will attach: the picture formats all three vendors
/// document accepting, the document format any of them reads, and the video
/// container Moonshot documents carrying as a base64 data URL.
///
/// A closed list rather than a guess from the extension, because the cost of
/// being wrong is a refused request the user paid for. Anything not here is
/// text, and the `read` tool already opens it.
pub const KINDS: &[Kind] = &[
    Kind {
        extension: "png",
        modality: Modality::Image,
        media_type: "image/png",
        confirms: png,
    },
    Kind {
        extension: "jpg",
        modality: Modality::Image,
        media_type: "image/jpeg",
        confirms: jpeg,
    },
    Kind {
        extension: "jpeg",
        modality: Modality::Image,
        media_type: "image/jpeg",
        confirms: jpeg,
    },
    Kind {
        extension: "gif",
        modality: Modality::Image,
        media_type: "image/gif",
        confirms: gif,
    },
    Kind {
        extension: "webp",
        modality: Modality::Image,
        media_type: "image/webp",
        confirms: webp,
    },
    Kind {
        extension: "pdf",
        modality: Modality::Pdf,
        media_type: "application/pdf",
        confirms: pdf,
    },
    Kind {
        extension: "mp4",
        modality: Modality::Video,
        media_type: "video/mp4",
        confirms: mp4,
    },
];

/// The eight bytes a PNG starts with, of which the last four catch a file a
/// transfer has rewritten the line endings of.
fn png(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
}

/// Every JPEG starts with a start-of-image marker and the next marker's
/// introducer. What follows differs by encoder, so three bytes is the whole of
/// what is common to all of them.
fn jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xff, 0xd8, 0xff])
}

/// The two GIF versions, both still written by something.
fn gif(bytes: &[u8]) -> bool {
    bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")
}

/// A WebP is a RIFF container, and the four bytes saying which kind sit after
/// the length rather than beside the tag.
fn webp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(&b"WEBP"[..])
}

/// The header a PDF opens with, version and all.
fn pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

/// An MP4-family file whose first box declares a bounded `ftyp` payload.
///
/// A four-byte major brand and minor version are mandatory; compatible brands
/// are extensible but must each be complete. An `ftyp` box consuming the whole
/// file is not a usable video because it leaves no room for media boxes.
fn mp4(bytes: &[u8]) -> bool {
    if bytes.get(4..8) != Some(&b"ftyp"[..]) {
        return false;
    }

    let Some(size) = bytes
        .get(..4)
        .and_then(|size| <[u8; 4]>::try_from(size).ok())
        .map(u32::from_be_bytes)
    else {
        return false;
    };
    let (header, size) = match size {
        0 => return false,
        1 => {
            let Some(size) = bytes
                .get(8..16)
                .and_then(|size| <[u8; 8]>::try_from(size).ok())
                .map(u64::from_be_bytes)
                .and_then(|size| usize::try_from(size).ok())
            else {
                return false;
            };
            (16, size)
        }
        size => (8, size as usize),
    };

    size >= header + 8 && size <= bytes.len() && (size - header - 8).is_multiple_of(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal whole `ftyp` box with one compatible brand.
    fn video(major: [u8; 4], compatible: [u8; 4]) -> Vec<u8> {
        let mut bytes = Vec::from(&20_u32.to_be_bytes()[..]);
        bytes.extend_from_slice(b"ftyp");
        bytes.extend_from_slice(&major);
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(&compatible);
        bytes
    }

    /// Rewrites the ordinary box size in a fixture known to contain it.
    fn sized(bytes: &mut [u8], size: u32) {
        bytes
            .get_mut(..4)
            .expect("the fixture starts with a four-byte size")
            .copy_from_slice(&size.to_be_bytes());
    }

    #[test]
    fn mp4_names_are_case_insensitive_and_name_video() {
        let kind = kind("clips/demo.MP4").expect("mp4 is attachable");

        assert_eq!(kind.modality, Modality::Video);
        assert_eq!(kind.media_type, "video/mp4");
        assert!((kind.confirms)(&video(*b"isom", *b"mp42")));
    }

    #[test]
    fn moonshots_documented_quicktime_branded_mp4_is_accepted() {
        assert!(mp4(&video(*b"qt  ", *b"qt  ")));
    }

    #[test]
    fn mp4_brands_are_extensible() {
        assert!(mp4(&video(*b"vend", *b"more")));
    }

    #[test]
    fn an_mp4_name_does_not_make_unrelated_bytes_a_video() {
        assert!(!mp4(b"this is not an ISO base media file"));
    }

    #[test]
    fn a_truncated_or_malformed_ftyp_box_is_refused() {
        let mut truncated = video(*b"isom", *b"mp42");
        truncated.pop();
        assert!(!mp4(&truncated));

        let mut partial_brand = video(*b"isom", *b"mp42");
        sized(&mut partial_brand, 19);
        assert!(!mp4(&partial_brand));

        let mut too_small = video(*b"isom", *b"mp42");
        sized(&mut too_small, 12);
        assert!(!mp4(&too_small));

        let mut unbounded = video(*b"isom", *b"mp42");
        sized(&mut unbounded, 0);
        assert!(!mp4(&unbounded));
    }
}
