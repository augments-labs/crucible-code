//! The download of one artifact of a release the agent catalog reviewed, held
//! to its pin from the first byte.
//!
//! An artifact is fetched from one of three places. The vendor's address is
//! the one the entry gives, and its mirror is a copy of the same bytes the
//! catalog keeps as a release asset of its own repository. Where the entry
//! names an asset built at review in the artifact's place, that asset is
//! fetched instead, from the catalog's own host by its SHA-256, and from
//! nowhere else. The mirror is asked only when the vendor's address failed
//! for a reason other than bytes that are not the pin's: a body of another
//! size or another SHA-256 is the vendor publishing something other than what
//! was reviewed, which another copy does not make right, and a download the
//! user cancelled is not asked again.
//!
//! The client reaches only the places a download may go: the hosts the
//! entry lists for its target, each a name and never an address or a
//! wildcard, and the hosts the catalog is published through, which
//! [`Source::hosts`] names; an asset built at review reaches the catalog's
//! own host and those alone. The first address is held to them as every
//! redirect is: a request to any other place is kept back before it is looked
//! up or dialled, and a redirect is followed by hand, at most four times, only
//! to an address over the scheme the download began with, naming no user
//! information and a host and port that are exactly one of them. The
//! application's hold is asked beside them, as it is for every client that can
//! reach a vendor; a download carries nothing from a session, so the hold
//! keeps back only an origin a warned route names.
//!
//! What arrives is bounded before a byte of it is kept: no more than the
//! pinned size, which is never more than [`RELEASE_BYTES`], within a bound on
//! the whole download and one on a silence in it. It is written into a file
//! its caller lends, hashed as it is, and never held in memory whole; a body
//! that ends short or runs long, or hashes to anything but the pin, is
//! refused whole, and the file is begun again for the next place asked.
//! Progress is told after every piece that arrives. What the receipt records
//! is the address the artifact is pinned at, and every host the download came
//! through, never an address a redirect gave.
//!
//! Every place a download may contact is known before anything is asked
//! ([`reachable`]): an artifact or a mirror pinned on no listed place is
//! refused then, so the question the user answers names them all.

use std::fs;
use std::io::{Seek as _, SeekFrom, Write as _};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crucible_external_harnesses::catalog::{Artifact, numbered};
use crucible_external_harnesses::release::RELEASE_BYTES;
use crucible_external_harnesses::store::Fetched;
use crucible_http::{BodyError, Chunk, Chunks, Hold, Http, HttpError, Origin, Tls};
use crucible_runtime::Cancel;
use sha2::{Digest as _, Sha256};
use tokio::time::timeout;

use crate::catalog_fetch::{self, Source, default_port, place, redirect_to};
use crate::content_use::{Consent, Routes};

/// The longest a release is given to download: the largest is a third of a
/// gigabyte.
const LIFETIME: Duration = Duration::from_mins(45);

/// The longest a download may go without a byte before it is given up.
const STALL: Duration = Duration::from_mins(1);

/// The most redirects followed for one address.
const HOPS: usize = 4;

/// Why an artifact did not arrive as its pin.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    /// The entry lists a download host that is an address, a wildcard or not
    /// a name, so nothing is fetched under its list.
    #[error("the catalog lists {0:?} as a download host, which is not a host name")]
    Host(Box<str>),
    /// The address or mirror an artifact is pinned at is on no host the
    /// download may reach.
    #[error("{0} is on no host the catalog lists for this download")]
    Unlisted(Box<str>),
    /// The download could not be written where it lands.
    #[error("the download could not be written: {0}")]
    Write(#[source] std::io::Error),
    /// An address is not one a download can be made from.
    #[error("{0} is not an address a download can be made from")]
    Address(Box<str>),
    /// The pin is larger than a release may be.
    #[error("the artifact is pinned at {0} bytes, more than a release may be")]
    TooLarge(u64),
    /// A secure connection could not be set up.
    #[error("a secure connection could not be set up")]
    Tls,
    /// A request failed, or was kept back from a place it may not reach.
    #[error(transparent)]
    Request(#[from] HttpError),
    /// The host answered with something other than the file.
    #[error("the download host answered {0}")]
    Status(u16),
    /// The host redirected somewhere a download does not follow.
    #[error("the download host redirected somewhere crucible does not follow")]
    Redirect,
    /// The host redirected more times than a download follows.
    #[error("the download host redirected more than {HOPS} times")]
    Redirects,
    /// The body could not be read to its end.
    #[error(transparent)]
    Body(#[from] BodyError),
    /// Nothing arrived for longer than a download waits.
    #[error("nothing arrived for {} seconds", STALL.as_secs())]
    Stalled,
    /// The download outlived its bound.
    #[error("the download did not finish within {} minutes", LIFETIME.as_secs() / 60)]
    Late,
    /// The body is not the size the artifact is pinned at.
    #[error("the download is not the {pinned} bytes the artifact is pinned at")]
    Size {
        /// The pinned size.
        pinned: u64,
    },
    /// The body is not the bytes the artifact is pinned as.
    #[error("the download is not the artifact the catalog pinned")]
    Digest,
    /// The download was cancelled.
    #[error("the download was cancelled")]
    Cancelled,
}

impl DownloadError {
    /// Whether the bytes arrived and are not the pin's, which no other copy is
    /// asked to make right.
    const fn mismatched(&self) -> bool {
        matches!(self, Self::Size { .. } | Self::Digest)
    }
}

/// Where a download lands, and who is told how far it has got.
pub struct Landing<'a> {
    /// The file the bytes are written into, from its start, which holds them
    /// whole once the download returns, and only then.
    pub file: &'a mut fs::File,
    /// Told how many of how many bytes have arrived, after every piece.
    pub progress: &'a mut (dyn FnMut(u64, u64) + Send),
}

impl std::fmt::Debug for Landing<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Landing").finish_non_exhaustive()
    }
}

/// `artifact` of a target whose download hosts are `hosts`, fetched under
/// `source` as the module says into `landing`'s file, telling its progress how
/// many of how many bytes have arrived, and given up as soon as `cancel` is
/// requested. What it gives is the artifact as its receipt records it: the
/// address it is pinned at, and every host the download came through.
///
/// # Errors
///
/// [`DownloadError`] naming why the last place asked gave nothing: the vendor's
/// address where the mirror is not asked, and the mirror's where it is.
pub async fn artifact(
    source: &Source,
    hosts: &[Box<str>],
    artifact: &Artifact,
    mut landing: Landing<'_>,
    cancel: &Cancel,
) -> Result<Fetched, DownloadError> {
    let places = reachable(source, hosts, artifact)?;
    let listed: Vec<&str> = places.iter().map(String::as_str).collect();
    if let Some(asset) = &artifact.replaced_by {
        let address = format!("{}/{}", source.base.trim_end_matches('/'), asset.sha256);
        let pin = Pinned {
            address: &address,
            size: asset.size,
            sha256: &asset.sha256,
        };
        return fetch(source, &listed, pin, &mut landing, cancel).await;
    }
    let pin = Pinned {
        address: &artifact.url,
        size: artifact.size,
        sha256: &artifact.sha256,
    };
    match fetch(source, &listed, pin, &mut landing, cancel).await {
        Ok(fetched) => Ok(fetched),
        Err(failed) if failed.mismatched() || matches!(failed, DownloadError::Cancelled) => {
            Err(failed)
        }
        Err(failed) => match &artifact.mirror {
            Some(mirror) => {
                let pin = Pinned {
                    address: mirror,
                    ..pin
                };
                fetch(source, &listed, pin, &mut landing, cancel).await
            }
            None => Err(failed),
        },
    }
}

/// Every place a download of `artifact` may contact, each a host, or a host
/// and port, as it is listed: the hosts its target lists and those the catalog
/// is published through, or for an asset built at review the catalog's own
/// host and those; refused where a listed host is not a name, or the address
/// or mirror the artifact is pinned at is on none of them. Asked before
/// anything is downloaded, so what a question names is what the hold lets
/// through.
///
/// # Errors
///
/// [`DownloadError::Host`] for a listed host that is not a name,
/// [`DownloadError::Address`] for an address that cannot be read and
/// [`DownloadError::Unlisted`] for one on no listed place.
pub fn reachable(
    source: &Source,
    hosts: &[Box<str>],
    artifact: &Artifact,
) -> Result<Vec<String>, DownloadError> {
    let mut places: Vec<String> = Vec::new();
    if artifact.replaced_by.is_some() {
        let origin = Origin::of(&source.base)
            .ok_or_else(|| DownloadError::Address(source.base.as_str().into()))?;
        places.push(format!("{}:{}", origin.host(), origin.port()));
    } else {
        named(hosts)?;
        places.extend(hosts.iter().map(ToString::to_string));
    }
    for host in source.hosts {
        if !places.iter().any(|place| place == host) {
            places.push((*host).to_owned());
        }
    }
    if artifact.replaced_by.is_none() {
        for address in std::iter::once(&artifact.url).chain(artifact.mirror.as_ref()) {
            let origin =
                Origin::of(address).ok_or_else(|| DownloadError::Address(address.clone()))?;
            let default = default_port(origin.scheme());
            let listed = places
                .iter()
                .filter_map(|entry| place(entry, default))
                .any(|(host, port)| host == origin.host() && port == origin.port());
            if !listed {
                return Err(DownloadError::Unlisted(address.clone()));
            }
        }
    }
    Ok(places)
}

/// Whether every host of `hosts`, a target's download hosts as the catalog
/// lists them, is a name a download may reach: never an address, in any
/// spelling, and never a wildcard.
///
/// # Errors
///
/// [`DownloadError::Host`] naming the first that is not.
pub fn named(hosts: &[Box<str>]) -> Result<(), DownloadError> {
    match hosts
        .iter()
        .find(|host| numbered(host) || place(host, default_port("https")).is_none())
    {
        Some(host) => Err(DownloadError::Host(host.clone())),
        None => Ok(()),
    }
}

/// Where a download is asked from, and the size and SHA-256 it is held to.
#[derive(Debug, Clone, Copy)]
struct Pinned<'a> {
    address: &'a str,
    size: u64,
    sha256: &'a str,
}

/// The body at `address`, from the places `listed` names alone, held to `pin`.
async fn fetch(
    source: &Source,
    listed: &[&str],
    pin: Pinned<'_>,
    landing: &mut Landing<'_>,
    cancel: &Cancel,
) -> Result<Fetched, DownloadError> {
    if pin.size > RELEASE_BYTES {
        return Err(DownloadError::TooLarge(pin.size));
    }
    let origin =
        Origin::of(pin.address).ok_or_else(|| DownloadError::Address(pin.address.into()))?;
    let client = client(source, &origin, listed)?;
    let fetching = async {
        timeout(LIFETIME, followed(&client, &origin, listed, pin, landing))
            .await
            .map_err(|_| DownloadError::Late)?
    };
    cancel
        .race(fetching)
        .await
        .ok_or(DownloadError::Cancelled)?
}

/// The client a download from the places `listed` names is made with, over
/// `source`'s proxy settings, beginning at `origin`.
fn client(source: &Source, origin: &Origin, listed: &[&str]) -> Result<Http, DownloadError> {
    let tls = Tls::new().map_err(|_| DownloadError::Tls)?;
    let plain = catalog_fetch::lookups();
    let default = default_port(origin.scheme());
    let places = Places {
        places: listed
            .iter()
            .filter_map(|entry| place(entry, default))
            .collect(),
        consent: Consent::new(Routes::production()),
    };
    Ok(
        Http::new(&tls, plain.clone().into(), plain, source.proxy.clone())
            .holding(Arc::new(places)),
    )
}

/// The places a download may reach, and the application's hold beside them.
#[derive(Debug)]
struct Places {
    places: Vec<(String, u16)>,
    consent: Consent,
}

impl Hold for Places {
    fn held(&self, origin: &Origin) -> Option<Box<str>> {
        if !self
            .places
            .iter()
            .any(|(host, port)| host == origin.host() && *port == origin.port())
        {
            return Some("a place the catalog does not list for this download".into());
        }
        self.consent.held(origin)
    }
}

/// The body at `address`, after at most [`HOPS`] redirects to the places
/// `listed` names, held to `pin`.
async fn followed(
    client: &Http,
    origin: &Origin,
    listed: &[&str],
    pin: Pinned<'_>,
    landing: &mut Landing<'_>,
) -> Result<Fetched, DownloadError> {
    // What an earlier place left is not this place's.
    landing.file.set_len(0).map_err(DownloadError::Write)?;
    landing
        .file
        .seek(SeekFrom::Start(0))
        .map_err(DownloadError::Write)?;
    let mut address = pin.address.to_owned();
    let mut through: Vec<Box<str>> = Vec::new();
    for _ in 0..=HOPS {
        if let Some(at) = Origin::of(&address)
            && !through.iter().any(|host| **host == *at.host())
        {
            through.push(at.host().into());
        }
        let response = client.get(&address).await?;
        match response.status().as_u16() {
            200 => {
                body(Chunks::new(response.into_body()), pin, landing).await?;
                // The address it is pinned at, not one a redirect gave, which
                // may carry a signature that lapses.
                return Ok(Fetched {
                    url: pin.address.into(),
                    hosts: through,
                    size: pin.size,
                    sha256: pin.sha256.into(),
                });
            }
            301 | 302 | 303 | 307 | 308 => {
                address = response
                    .headers()
                    .get("location")
                    .and_then(|location| location.to_str().ok())
                    .and_then(|location| redirect_to(location, origin.scheme(), listed))
                    .ok_or(DownloadError::Redirect)?;
            }
            other => return Err(DownloadError::Status(other)),
        }
    }
    Err(DownloadError::Redirects)
}

/// What `chunks` hold, written into `landing`'s file as it arrives and
/// hashed as it is, refused where that is not exactly `pin`'s bytes.
async fn body(
    mut chunks: Chunks,
    pin: Pinned<'_>,
    landing: &mut Landing<'_>,
) -> Result<(), DownloadError> {
    let mismatched = DownloadError::Size { pinned: pin.size };
    let mut held: u64 = 0;
    let mut digest = Sha256::new();
    let mut heard = Instant::now();
    while let Some(next) = chunks.next().await {
        match next? {
            Chunk::Data(bytes) => {
                let total = u64::try_from(bytes.len())
                    .ok()
                    .and_then(|arrived| held.checked_add(arrived))
                    .ok_or(DownloadError::Size { pinned: pin.size })?;
                if total > pin.size {
                    return Err(mismatched);
                }
                digest.update(&bytes);
                landing
                    .file
                    .write_all(&bytes)
                    .map_err(DownloadError::Write)?;
                held = total;
                heard = Instant::now();
                (landing.progress)(total, pin.size);
            }
            Chunk::Quiet if heard.elapsed() >= STALL => return Err(DownloadError::Stalled),
            Chunk::Quiet => {}
        }
    }
    if held != pin.size {
        return Err(mismatched);
    }
    if hex(&digest.finalize()) != pin.sha256 {
        return Err(DownloadError::Digest);
    }
    landing.file.flush().map_err(DownloadError::Write)
}

/// `bytes` as lower case hexadecimal.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

#[cfg(test)]
mod tests;
