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
//!
//! A release a pinned Node starts has a package tree beside its artifacts,
//! whose lock pins each tarball by its size and SHA-512 ([`tarballs`]). A
//! tarball is fetched from the registry address the lock gives, which must be
//! on a host the target lists, as [`tree_reachable`] says before anything is
//! asked, and from nowhere else: there is no mirror. It goes through the same
//! hold, redirect rule and bounds as an artifact, held to its pin from the
//! first byte, and is written into a file of its own named by its SHA-512, made
//! new in an owner-only directory, so nothing already there, a link or a pipe
//! among it, is opened. At most [`AT_ONCE`] download together, progress is the
//! sum of what each has received, and the first to fail or a cancel stops them
//! all. A test build alone fetches a registry address from the `registry/`
//! of a catalog published on this machine instead; a shipping build has no
//! such path.
//!
//! A host that answers 404 or 410 is read as gone for good: the text says so,
//! and that the version can likely no longer be installed. Nothing is retried
//! for it.

use std::collections::HashSet;
use std::fs;
use std::future::{Future, poll_fn};
use std::io::{Seek as _, SeekFrom, Write as _};
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::{Duration, Instant};

use crucible_external_harnesses::catalog::{Artifact, numbered};
use crucible_external_harnesses::release::{NpmError, NpmTarball, NpmTree, RELEASE_BYTES};
use crucible_external_harnesses::store::Fetched;
use crucible_http::{BodyError, Chunk, Chunks, Hold, Http, HttpError, Origin, Tls};
use crucible_runtime::Cancel;
use sha2::{Digest as _, Sha256, Sha512};
use tokio::time::timeout;

use crate::catalog_fetch::{self, FetchError, Source, default_port, place, redirect_to};
use crate::content_use::{Consent, Routes};

/// The longest a release is given to download: the largest is a third of a
/// gigabyte.
const LIFETIME: Duration = Duration::from_mins(45);

/// The longest a download may go without a byte before it is given up.
const STALL: Duration = Duration::from_mins(1);

/// The most redirects followed for one address.
const HOPS: usize = 4;

/// The most tarballs of one tree downloaded at once.
pub const AT_ONCE: usize = 8;

/// A download of the downloads [`tarballs`] makes together, as it is polled.
type Downloading<'a> = Pin<Box<dyn Future<Output = Result<(), DownloadError>> + Send + 'a>>;

/// Who is told how far a tree's download has got, and the sum so far.
type Told<'a> = Mutex<(&'a mut (dyn FnMut(u64, u64) + Send), u64)>;

/// Why an artifact, a package lock or a tarball did not arrive as its pin.
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
    /// The host answered with something other than the file; a 404 or a 410
    /// reads as likely permanent.
    #[error("{}", answered(*.0))]
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
    /// The package lock of a release a pinned Node starts did not arrive from
    /// the catalog's host as the catalog lists it.
    #[error("the release's package lock could not be fetched from the catalog: {0}")]
    Lock(#[source] FetchError),
    /// The package lock is not one this crucible reads, or a tarball it pins
    /// could not be unpacked as it says.
    #[error(transparent)]
    Tree(#[from] NpmError),
}

/// What a host that answered `status` said, read as likely permanent for a
/// 404 or a 410, which no later try is likely to change.
fn answered(status: u16) -> String {
    match status {
        404 | 410 => format!(
            "the download host answered {status}, which is likely permanent: the file is gone, so \
             this version can likely no longer be installed"
        ),
        other => format!("the download host answered {other}"),
    }
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
    // The address it is pinned at, not one a redirect gave, which may carry
    // a signature that lapses.
    let receipted = |pin: Pinned<'_>, hosts| Fetched {
        url: pin.address.into(),
        hosts,
        size: pin.size,
        sha256: pin.digest.hex().into(),
    };
    if let Some(asset) = &artifact.replaced_by {
        let address = format!("{}/{}", source.base.trim_end_matches('/'), asset.sha256);
        let pin = Pinned {
            address: &address,
            size: asset.size,
            digest: Digest::Sha256(&asset.sha256),
        };
        let hosts = fetch(source, &listed, pin, &mut landing, cancel).await?;
        return Ok(receipted(pin, hosts));
    }
    let pin = Pinned {
        address: &artifact.url,
        size: artifact.size,
        digest: Digest::Sha256(&artifact.sha256),
    };
    match fetch(source, &listed, pin, &mut landing, cancel).await {
        Ok(hosts) => Ok(receipted(pin, hosts)),
        Err(failed) if failed.mismatched() || matches!(failed, DownloadError::Cancelled) => {
            Err(failed)
        }
        Err(failed) => match &artifact.mirror {
            Some(mirror) => {
                let pin = Pinned {
                    address: mirror,
                    ..pin
                };
                let hosts = fetch(source, &listed, pin, &mut landing, cancel).await?;
                Ok(receipted(pin, hosts))
            }
            None => Err(failed),
        },
    }
}

/// Every place a download of `tree`'s tarballs may contact, each a host, or a
/// host and port, as it is listed: the hosts its target lists, `hosts`, and
/// those the catalog is published through; refused where a listed host is not
/// a name, or a tarball's address is on none of them. Asked before anything is
/// downloaded, so what a question names is what the hold lets through.
///
/// # Errors
///
/// [`DownloadError::Host`] for a listed host that is not a name,
/// [`DownloadError::Address`] for an address that cannot be read and
/// [`DownloadError::Unlisted`] for one on no listed place.
pub fn tree_reachable(
    source: &Source,
    hosts: &[Box<str>],
    tree: &NpmTree,
) -> Result<Vec<String>, DownloadError> {
    named(hosts)?;
    let mut places: Vec<String> = hosts.iter().map(ToString::to_string).collect();
    for host in source.hosts {
        if !places.iter().any(|place| place == host) {
            places.push((*host).to_owned());
        }
    }
    for tarball in tree
        .packages()
        .iter()
        .filter_map(|package| package.tarball.as_ref())
    {
        listed_on(&places, &tarball.url)?;
    }
    Ok(places)
}

/// A package tree to download, and the hosts its target lists.
#[derive(Debug, Clone, Copy)]
pub struct Tree<'a> {
    /// The hosts the target lists.
    pub hosts: &'a [Box<str>],
    /// The tree, as its lock gives it.
    pub tree: &'a NpmTree,
}

/// What each of the downloads [`tarballs`] makes shares.
struct Shared<'a, 'b> {
    source: &'a Source,
    listed: &'a [&'a str],
    into: &'a Path,
    told: &'a Told<'b>,
    total: u64,
    cancel: &'a Cancel,
}

/// Every tarball `tree` pins, from the registry address its lock gives on the
/// places [`tree_reachable`] names alone, each held to its size and SHA-512
/// and written into a file of its own under `into`, a directory made new and
/// owner-only for them, named by that SHA-512; at most [`AT_ONCE`] at a time,
/// telling `progress` how many of how many bytes of them all have arrived,
/// and given up, every one, at the first that fails or as soon as `cancel` is
/// requested. A tarball two packages share is downloaded once.
///
/// # Errors
///
/// [`DownloadError`] naming why the first that failed did, or
/// [`DownloadError::Write`] where `into`, or a file under it, could not be
/// made new.
pub async fn tarballs(
    source: &Source,
    Tree { hosts, tree }: Tree<'_>,
    into: &Path,
    progress: &mut (dyn FnMut(u64, u64) + Send),
    cancel: &Cancel,
) -> Result<(), DownloadError> {
    #[cfg_attr(not(feature = "proof"), expect(unused_mut))]
    let mut places = tree_reachable(source, hosts, tree)?;
    // Only a test build fetches the registry's copy from a catalog on this
    // machine, which is then a place it reaches; see `fixture`.
    #[cfg(feature = "proof")]
    if crate::proof::looped(&source.base)
        && let Some(origin) = Origin::of(&source.base)
    {
        places.push(format!("{}:{}", origin.host(), origin.port()));
    }
    let listed: Vec<&str> = places.iter().map(String::as_str).collect();
    fs::create_dir(into).map_err(DownloadError::Write)?;
    crucible_privacy::directory(into)
        .map_err(|error| DownloadError::Write(std::io::Error::other(error)))?;
    let mut seen: HashSet<&str> = HashSet::new();
    let pinned: Vec<&NpmTarball> = tree
        .packages()
        .iter()
        .filter_map(|package| package.tarball.as_ref())
        .filter(|tarball| seen.insert(&tarball.sha512))
        .collect();
    // Only what progress is told of: the lock was read within the most a
    // tree may be, and each body is held to its own pin.
    let total = pinned
        .iter()
        .fold(0_u64, |total, tarball| total.saturating_add(tarball.size));
    let told: Told<'_> = Mutex::new((progress, 0));
    let shared = Shared {
        source,
        listed: &listed,
        into,
        told: &told,
        total,
        cancel,
    };
    let mut waiting = pinned.into_iter();
    let mut running: Vec<Downloading<'_>> = Vec::with_capacity(AT_ONCE);
    let all = poll_fn(|context| {
        loop {
            while running.len() < AT_ONCE {
                let Some(tarball) = waiting.next() else {
                    break;
                };
                running.push(Box::pin(one(&shared, tarball)));
            }
            if running.is_empty() {
                return Poll::Ready(Ok(()));
            }
            let before = running.len();
            let mut failed = None;
            running.retain_mut(|download| match download.as_mut().poll(context) {
                Poll::Ready(Ok(())) => false,
                Poll::Ready(Err(error)) => {
                    failed.get_or_insert(error);
                    false
                }
                Poll::Pending => true,
            });
            if let Some(error) = failed {
                return Poll::Ready(Err(error));
            }
            if running.len() == before {
                return Poll::Pending;
            }
        }
    });
    cancel
        .race(timeout(LIFETIME, all))
        .await
        .ok_or(DownloadError::Cancelled)?
        .map_err(|_| DownloadError::Late)?
}

/// One of the tarballs [`tarballs`] downloads, into a file made new for it
/// under `into`, adding what arrives to the sum `told` keeps of `total`.
async fn one(
    &Shared {
        source,
        listed,
        into,
        told,
        total,
        cancel,
    }: &Shared<'_, '_>,
    tarball: &NpmTarball,
) -> Result<(), DownloadError> {
    // Made new, so nothing already at the name, a link or a pipe among it,
    // is followed or opened.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(into.join(&*tarball.sha512))
        .map_err(DownloadError::Write)?;
    let mut last: u64 = 0;
    let mut tell = |done: u64, _: u64| {
        if let Ok(mut told) = told.lock() {
            let (progress, sum) = &mut *told;
            *sum = sum.saturating_add(done.saturating_sub(last)).min(total);
            last = done;
            progress(*sum, total);
        }
    };
    let fixture = fixture(source, &tarball.url);
    let pin = Pinned {
        address: fixture.as_deref().unwrap_or(&tarball.url),
        size: tarball.size,
        digest: Digest::Sha512(&tarball.sha512),
    };
    let landing = &mut Landing {
        file: &mut file,
        progress: &mut tell,
    };
    fetch(source, listed, pin, landing, cancel).await.map(drop)
}

/// Where a test build fetches the registry's `url` from: the `registry/` of
/// a catalog published on this machine, as [`crate::proof::looped`] reads
/// its address, so the headless suite installs a tree with no network. A
/// shipping build has no such path, as it has none for the catalog itself.
#[cfg(feature = "proof")]
fn fixture(source: &Source, url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://registry.npmjs.org/")?;
    crate::proof::looped(&source.base)
        .then(|| format!("{}/registry/{rest}", source.base.trim_end_matches('/')))
}

/// A shipping build fetches every tarball from the address its lock gives.
#[cfg(not(feature = "proof"))]
const fn fixture(_: &Source, _: &str) -> Option<String> {
    None
}

/// Refused unless `address` is on one of `places`, matched by host and port.
fn listed_on(places: &[String], address: &str) -> Result<(), DownloadError> {
    let origin = Origin::of(address).ok_or_else(|| DownloadError::Address(address.into()))?;
    let default = default_port(origin.scheme());
    let listed = places
        .iter()
        .filter_map(|entry| place(entry, default))
        .any(|(host, port)| host == origin.host() && port == origin.port());
    if listed {
        Ok(())
    } else {
        Err(DownloadError::Unlisted(address.into()))
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
            listed_on(&places, address)?;
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

/// Where a download is asked from, and the size and digest it is held to.
#[derive(Debug, Clone, Copy)]
struct Pinned<'a> {
    address: &'a str,
    size: u64,
    digest: Digest<'a>,
}

/// The digest a download is pinned by, in lowercase hexadecimal.
#[derive(Debug, Clone, Copy)]
enum Digest<'a> {
    /// An artifact's, or an asset's.
    Sha256(&'a str),
    /// A tarball's, as its lock gives it.
    Sha512(&'a str),
}

impl<'a> Digest<'a> {
    const fn hex(self) -> &'a str {
        match self {
            Self::Sha256(hex) | Self::Sha512(hex) => hex,
        }
    }
}

/// A digest being computed over a body as it arrives.
enum Hashing {
    Sha256(Sha256),
    Sha512(Sha512),
}

impl Hashing {
    fn new(digest: Digest<'_>) -> Self {
        match digest {
            Digest::Sha256(_) => Self::Sha256(<Sha256 as sha2::Digest>::new()),
            Digest::Sha512(_) => Self::Sha512(<Sha512 as sha2::Digest>::new()),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Sha256(digest) => sha2::Digest::update(digest, bytes),
            Self::Sha512(digest) => sha2::Digest::update(digest, bytes),
        }
    }

    fn finish(self) -> String {
        match self {
            Self::Sha256(digest) => hex(&digest.finalize()),
            Self::Sha512(digest) => hex(&digest.finalize()),
        }
    }
}

/// The body at `address`, from the places `listed` names alone, held to `pin`;
/// gives every host the download came through.
async fn fetch(
    source: &Source,
    listed: &[&str],
    pin: Pinned<'_>,
    landing: &mut Landing<'_>,
    cancel: &Cancel,
) -> Result<Vec<Box<str>>, DownloadError> {
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
/// `listed` names, held to `pin`; gives every host it came through.
async fn followed(
    client: &Http,
    origin: &Origin,
    listed: &[&str],
    pin: Pinned<'_>,
    landing: &mut Landing<'_>,
) -> Result<Vec<Box<str>>, DownloadError> {
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
                return Ok(through);
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
    let mut digest = Hashing::new(pin.digest);
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
    if digest.finish() != pin.digest.hex() {
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
