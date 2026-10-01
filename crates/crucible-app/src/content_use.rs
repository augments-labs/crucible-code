//! The routes whose vendor says it uses what is sent to it, and the yes that
//! lets a request leave for one.
//!
//! **A route** is what one answer is about: a `/login` row, spelled by its list
//! and the name its credential is stored under (`subscription:openai`,
//! `key:moonshot@kimi.ai`); a model that is itself warned, spelled
//! `model:<provider>/<model>`; or a Kimi open platform address, spelled by its
//! host alone (`api.moonshot.ai`), so one address is one route under whichever
//! provider's `baseUrl` holds it. A spelling is written into the user's
//! configuration file, so it never changes once shipped.
//!
//! **A warned route** is one whose vendor's own terms say what is sent there
//! may be used to train or improve its models ([`WARNED`]). Each carries the
//! vendor's words in English, saying no more than its page, the way to opt
//! out where the page gives one, and the page and the day it was read. A
//! route whose vendor does not say gets no warning and is never asked about.
//!
//! **The hold.** Every HTTP client built outside tests that can reach a vendor
//! is handed a [`Consent`], asked before each request leaves; the release
//! check's own, which reaches GitHub alone, is the one that is not. A request to an origin of a warned route
//! with no yes waits, whichever way the route was reached: `/login`, `/model`,
//! a key from the environment, configuration, `--model` or a resumed session.
//! What the front ends ask first is a courtesy on top of that; this is what
//! nothing walks around.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

use crucible_auth::{Dropped, LettingGo};
use crucible_config::Settings;
use crucible_http::{Hold, Origin};

use crate::providers::{List, Row, Rows};

/// What a vendor says about one route, and where it says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Warning {
    /// The vendor's words in English, the condition first where there is one
    /// and the way to opt out last where the page gives one.
    pub sentence: &'static str,
    /// What decides whether it holds, where crucible cannot see it: the plan
    /// or the quota. The sentence opens with it.
    pub condition: Option<&'static str>,
    /// The page, as the panel's quiet row names it.
    pub source: &'static str,
    /// Where the page is.
    pub link: &'static str,
    /// The day it was read, or the day of the archived copy it rests on.
    pub read: &'static str,
    /// A `/login` row's few words for it, in the vendor's own verb.
    pub caution: &'static str,
}

impl Warning {
    /// The quiet row under the sentence: the page and the day.
    #[must_use]
    pub fn cited(&self) -> String {
        format!("{}, {}", self.source, self.read)
    }
}

/// One warned route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Warned {
    /// The route's spelling, as the yes is written.
    pub route: &'static str,
    /// What the panel is titled.
    pub shown: &'static str,
    /// What its vendor says.
    pub warning: Warning,
    /// Every origin a request of this route is sent to: its model and web tool
    /// address, and for a sign-in the host that issues and renews its tokens.
    pub origins: &'static [&'static str],
}

const OPENAI_PLANS: Warning = Warning {
    sentence: "On Free, Plus and Pro, OpenAI may use what you send to train its models. To stop \
               it, turn off Improve the model for everyone under Settings, Data controls in \
               ChatGPT, or choose Do not train on my content in its Privacy Portal; that covers \
               only what you send afterwards.",
    condition: Some("On Free, Plus and Pro"),
    source: "OpenAI Help Center, archived copy",
    link: "https://help.openai.com/en/articles/5722486-how-your-data-is-used-to-improve-model-performance",
    read: "28 Sep 2026",
    caution: "may train on what is sent",
};

const GEMINI_UNPAID: Warning = Warning {
    sentence: "On unpaid quota, Google uses what you send and what it answers to improve its \
               products and machine learning technologies. In the EEA, Switzerland and the UK \
               the paid terms apply instead.",
    condition: Some("On unpaid quota"),
    source: "Gemini API terms",
    link: "https://ai.google.dev/gemini-api/terms",
    read: "30 Sep 2026",
    caution: "uses what is sent",
};

const KIMI_AI: Warning = Warning {
    sentence: "Kimi may use what you send to train its models. To stop it, contact Kimi as its \
               terms say; that covers only what you send afterwards.",
    condition: None,
    source: "kimi.ai terms of service",
    link: "https://www.kimi.ai/user/agreement/modelUse?version=v2",
    read: "30 Sep 2026",
    caution: "may train on what is sent",
};

const KIMI_COM: Warning = Warning {
    sentence: "Kimi may use what you send, and what it answers, to improve its models. To keep \
               it out of training, contact Kimi as its terms say.",
    condition: None,
    source: "kimi.com user agreement",
    link: "https://www.kimi.com/user/agreement/modelUse?version=v2",
    read: "30 Sep 2026",
    caution: "may use what is sent",
};

const PLATFORM_AI: Warning = Warning {
    sentence: "Moonshot may use what you send to develop and improve its services, and to train \
               its models, unless you agree otherwise with it in writing.",
    condition: None,
    source: "Kimi open platform terms",
    link: "https://platform.kimi.ai/docs/agreement/modeluse",
    read: "30 Sep 2026",
    caution: "may train on what is sent",
};

const PLATFORM_CN: Warning = Warning {
    sentence: "Moonshot may use what you send, and what it answers, to improve its models.",
    condition: None,
    source: "Kimi open platform terms, platform.kimi.com",
    link: "https://platform.kimi.com/docs/agreement/modeluse",
    read: "30 Sep 2026",
    caution: "may use what is sent",
};

/// Every warned route this build has.
pub const WARNED: [Warned; 8] = [
    Warned {
        route: "subscription:openai",
        shown: "OpenAI",
        warning: OPENAI_PLANS,
        origins: &["https://chatgpt.com", "https://auth.openai.com"],
    },
    Warned {
        route: "subscription:moonshot@kimi.ai",
        shown: "Kimi Code · kimi.ai",
        warning: KIMI_AI,
        origins: &["https://api.kimi.ai", "https://auth.kimi.ai"],
    },
    Warned {
        route: "subscription:moonshot",
        shown: "Kimi Code · kimi.com",
        warning: KIMI_COM,
        origins: &["https://api.kimi.com", "https://auth.kimi.com"],
    },
    Warned {
        route: "key:google",
        shown: "Google",
        warning: GEMINI_UNPAID,
        origins: &["https://generativelanguage.googleapis.com"],
    },
    Warned {
        route: "key:moonshot@kimi.ai",
        shown: "MoonshotAI · kimi.ai",
        warning: KIMI_AI,
        origins: &["https://api.kimi.ai"],
    },
    Warned {
        route: "key:moonshot",
        shown: "MoonshotAI · kimi.com",
        warning: KIMI_COM,
        origins: &["https://api.kimi.com"],
    },
    Warned {
        route: "api.moonshot.ai",
        shown: "Kimi open platform · api.moonshot.ai",
        warning: PLATFORM_AI,
        origins: &["https://api.moonshot.ai"],
    },
    Warned {
        route: "api.moonshot.cn",
        shown: "Kimi open platform · api.moonshot.cn",
        warning: PLATFORM_CN,
        origins: &["https://api.moonshot.cn"],
    },
];

/// One address crucible documents, and the route a `baseUrl` at it answers
/// for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recognised {
    /// The address, as the docs write it.
    pub address: &'static str,
    /// The route it answers for: a key row's, or a Kimi open platform
    /// address's own.
    pub route: &'static str,
}

/// Every address a `baseUrl` is recognised at.
///
/// The address of built-in rows answers for the key row there, since only a
/// key is ever sent to a `baseUrl`; so the `OpenAI` sign-in's address, where
/// no key row stands, is not among them. Each Kimi open platform address is a
/// route of its own.
pub const RECOGNISED: [Recognised; 7] = [
    Recognised {
        address: "https://api.anthropic.com/v1",
        route: "key:anthropic",
    },
    Recognised {
        address: "https://generativelanguage.googleapis.com/v1beta",
        route: "key:google",
    },
    Recognised {
        address: "https://api.kimi.ai/coding/v1",
        route: "key:moonshot@kimi.ai",
    },
    Recognised {
        address: "https://api.kimi.com/coding/v1",
        route: "key:moonshot",
    },
    Recognised {
        address: "https://api.openai.com/v1",
        route: "key:openai",
    },
    Recognised {
        address: "https://api.moonshot.ai/v1",
        route: "api.moonshot.ai",
    },
    Recognised {
        address: "https://api.moonshot.cn/v1",
        route: "api.moonshot.cn",
    },
];

/// The route a row's credential is sent on.
#[must_use]
pub fn row_route(row: &Row) -> String {
    let list = match row.list {
        List::Subscription => "subscription",
        List::Key => "key",
    };
    format!("{list}:{}", row.stored)
}

/// The route of one model, where the model itself is warned.
#[must_use]
pub fn model_route(provider: &str, model: &str) -> String {
    format!("model:{provider}/{model}")
}

/// The route a configured `baseUrl` answers for, where it is an address
/// crucible documents, and `None` for every other.
///
/// Recognised when its scheme is the documented address's, its host is that
/// address's without regard to case, to a trailing dot or to the default port
/// written out, and its path, under any of six readings, is that address's
/// path or lies under it by whole segments. The readings are three orders,
/// each with adjacent slashes kept or merged: `.` and `..` resolved before
/// escapes are decoded; after, with a decoded `/` kept inside its segment;
/// and after, with it taken as a separator. Any one of them being documented
/// is enough for the route to be asked about. A reading is only ever added,
/// never taken away, so a spelling one of them recognises stays recognised.
/// A path a server reads some other way, such as with `\` as a separator or
/// with `;` parameters dropped, is not one of these.
#[must_use]
pub fn recognised(base_url: &str) -> Option<&'static str> {
    let origin = Origin::of(base_url)?;
    let read = readings(base_url);
    RECOGNISED
        .iter()
        .find(|one| {
            let documented = readings(one.address);
            Origin::of(one.address).is_some_and(|at| at == origin)
                && read.iter().any(|path| {
                    documented
                        .first()
                        .is_some_and(|documented| path.starts_with(documented))
                })
        })
        .map(|one| one.route)
}

/// The ways a server can read `url`'s path, each as its named segments.
///
/// With empty segments kept while `..` is resolved (`/v1//../x` is `/v1/x`),
/// and with adjacent slashes merged first (`/x//../v1` is `/v1`); and for
/// each, three orders: resolved as written then decoded, each segment decoded
/// whole then resolved, and decoded with a decoded `/` a separator then
/// resolved. Empty segments are left out once resolved, so a trailing slash
/// says nothing.
fn readings(url: &str) -> Vec<Vec<String>> {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = after_scheme
        .find('/')
        .and_then(|at| after_scheme.get(at..))
        .unwrap_or_default();
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let written: Vec<String> = path.split('/').map(str::to_owned).collect();
    let whole: Vec<String> = written.iter().map(|segment| decoded(segment)).collect();
    let split: Vec<String> = whole
        .iter()
        .flat_map(|segment| segment.split('/').map(str::to_owned).collect::<Vec<_>>())
        .collect();
    let named = |segments: Vec<String>| -> Vec<String> {
        segments
            .into_iter()
            .filter(|segment| !segment.is_empty())
            .collect()
    };
    let mut readings = Vec::with_capacity(6);
    for merged in [false, true] {
        let kept = |segments: &[String]| -> Vec<String> {
            segments
                .iter()
                .filter(|segment| !merged || !segment.is_empty())
                .cloned()
                .collect()
        };
        readings.push(named(
            resolved(kept(&written).into_iter())
                .iter()
                .map(|segment| decoded(segment))
                .collect(),
        ));
        readings.push(named(resolved(kept(&whole).into_iter())));
        readings.push(named(resolved(kept(&split).into_iter())));
    }
    readings
}

/// `segments` with `.` and `..` resolved as RFC 3986 resolves them: a `.`
/// names nothing, and a `..` takes out the segment before it, empty or not.
fn resolved(segments: impl Iterator<Item = String>) -> Vec<String> {
    let mut named: Vec<String> = Vec::new();
    for segment in segments {
        match segment.as_str() {
            "." => {}
            ".." => {
                named.pop();
            }
            _ => named.push(segment),
        }
    }
    named
}

/// `segment` with each `%XX` of two hex digits read as the byte it names.
fn decoded(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        let hex = bytes
            .get(at + 1..at + 3)
            .filter(|pair| pair.iter().all(u8::is_ascii_hexdigit))
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        if let (b'%', Some(value)) = (byte, hex) {
            out.push(value);
            at += 3;
        } else {
            out.push(byte);
            at += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The warned routes a build has, and the origins each is sent to.
#[derive(Debug, Clone)]
pub struct Routes {
    warned: Arc<[Warned]>,
}

impl Routes {
    /// The routes this build ships.
    #[must_use]
    pub fn production() -> Self {
        Self::new(WARNED.to_vec())
    }

    /// Any routes, for a caller that stands its own origins in.
    #[must_use]
    pub fn new(warned: Vec<Warned>) -> Self {
        Self {
            warned: warned.into(),
        }
    }

    /// The warned route spelled `route`, and `None` for a route whose vendor
    /// does not say.
    #[must_use]
    pub fn warned(&self, route: &str) -> Option<&Warned> {
        self.warned.iter().find(|warned| warned.route == route)
    }

    /// Every warned route.
    #[must_use]
    pub fn all(&self) -> &[Warned] {
        &self.warned
    }
}

/// What one provider's requests are sent on at the moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Serving {
    /// The route, where it has a spelling: a row's, or the one a recognised
    /// `baseUrl` answers for. `None` for an address crucible does not know.
    pub route: Option<String>,
    /// The origin its requests go to, where it is not the route's own: a
    /// configured `baseUrl`'s.
    pub at: Option<Origin>,
}

/// What a provider is served on, read again as things are when it is asked:
/// the answer for a provider whose credential was just taken out, which may
/// still be served by another credential, or by nothing.
pub type Resolver = Box<dyn Fn(&str) -> Reading + Send + Sync>;

/// What a [`Resolver`] read of a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    /// What it is served on now, or `None` for nothing.
    Served(Option<Serving>),
    /// What it is served on could not be read, the store being unreadable:
    /// it stays stale, what it was last served on still claims its origin,
    /// and the next question reads it again.
    Unread,
}

/// The [`Resolver`] a consent was handed, which has nothing to show.
struct Resolving(Resolver);

impl std::fmt::Debug for Resolving {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Resolving")
    }
}

/// The yes given and recorded for each route, and what each provider is
/// served on, asked before every request leaves.
///
/// Cheap to clone; clones share one state, so the client a turn sends through
/// and the front end that records a yes see the same answer.
#[derive(Debug, Clone)]
pub struct Consent {
    routes: Routes,
    state: Arc<RwLock<State>>,
    /// The user's own configuration file, where a yes is written down.
    file: Arc<std::sync::OnceLock<PathBuf>>,
    /// Reads again what a provider is served on once that has gone stale.
    resolving: Arc<std::sync::OnceLock<Resolving>>,
}

#[derive(Debug, Default)]
struct State {
    /// Routes the user's own file says yes to.
    recorded: BTreeSet<String>,
    /// Whether the file's yes has been read this run.
    read: bool,
    /// Routes a `/login` choice said yes to, not written down until the
    /// credential is stored.
    given: BTreeSet<String>,
    /// What each provider's requests go on, as last resolved.
    serving: BTreeMap<String, Serving>,
    /// Providers whose credential was taken out since they were resolved,
    /// read again at the next send or choice, whichever provider it is for.
    stale: BTreeSet<String>,
}

impl Consent {
    /// Consent over `routes`, with no yes yet.
    #[must_use]
    pub fn new(routes: Routes) -> Self {
        Self {
            routes,
            state: Arc::default(),
            file: Arc::default(),
            resolving: Arc::default(),
        }
    }

    /// Reads again, through `resolver`, what a provider is served on once
    /// its credential was taken out. The first resolver given is the one
    /// kept; with none, such a provider keeps what it was last served on
    /// until it is set up again.
    pub fn resolves(&self, resolver: Resolver) {
        let _ = self.resolving.set(Resolving(resolver));
    }

    /// Writes each yes into `file`, the user's own configuration file. The
    /// first file given is the one kept.
    pub fn keeps_in(&self, file: PathBuf) {
        let _ = self.file.set(file);
    }

    /// The user's own configuration file, where one was given.
    pub(crate) fn file(&self) -> Option<&Path> {
        self.file.get().map(PathBuf::as_path)
    }

    /// Says yes to `warned`: written into the user's own file, then let go.
    /// Nothing is let go where the file could not be written.
    ///
    /// # Errors
    ///
    /// [`crate::remember::RememberError`] where the file could not be
    /// written, or no file was given to write it in.
    pub fn accept(&self, warned: &Warned) -> Result<(), crate::remember::RememberError> {
        let file = self
            .file
            .get()
            .ok_or_else(|| crate::remember::RememberError::Unwritable {
                file: "the configuration file".into(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "no file was given"),
            })?;
        crate::remember::accepting(file, warned.route)?;
        self.record(warned.route);
        Ok(())
    }

    /// The routes it answers about.
    #[must_use]
    pub fn routes(&self) -> &Routes {
        &self.routes
    }

    /// The warned route `route` is, where it has no yes.
    #[must_use]
    pub fn asks(&self, route: &str) -> Option<&Warned> {
        let warned = self.routes.warned(route)?;
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        (!state.recorded.contains(route) && !state.given.contains(route)).then_some(warned)
    }

    /// The warned route a send by `provider`, asking `model`, would go on
    /// with no yes: the model's own where the model is warned, then the route
    /// that holds an origin the provider is served at, its own or another
    /// provider's there. `None` where nothing is to be asked, and for a
    /// provider served on nothing, whatever its model. Every provider whose
    /// credential was taken out since it was resolved is read again first.
    #[must_use]
    pub fn unanswered(&self, provider: &str, model: &str) -> Option<Warned> {
        // Every provider gone stale, not only this one: a claim kept for a
        // provider nobody sends through would otherwise hold an origin it
        // shares with this one on the strength of a credential that is gone.
        if let Some(Resolving(resolve)) = self.resolving.get() {
            let unsettled: Vec<String> = self
                .state
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .stale
                .iter()
                .cloned()
                .collect();
            for one in unsettled {
                if let Reading::Served(now) = resolve(&one) {
                    self.served(&one, now);
                }
            }
        }
        let served = {
            let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
            state.serving.get(provider)?.clone()
        };
        if let Some(warned) = self.asks(&model_route(provider, model)) {
            return Some(*warned);
        }
        // Whatever holds the origins the provider is served at, which may be
        // another provider's route there as well as its own: what is asked
        // about is what keeps the request back.
        let origins: Vec<Origin> = match (&served.at, &served.route) {
            (Some(at), _) => vec![at.clone()],
            (None, Some(route)) => self
                .routes
                .warned(route)
                .map(|warned| {
                    warned
                        .origins
                        .iter()
                        .filter_map(|at| Origin::of(at))
                        .collect()
                })
                .unwrap_or_default(),
            (None, None) => Vec::new(),
        };
        origins
            .iter()
            .find_map(|origin| self.held(origin))
            .and_then(|route| self.routes.warned(&route).copied())
    }

    /// Takes the routes the user's file says yes to, the first time it is
    /// told this run; later tellings are ignored, since a yes the run has
    /// taken out since must not come back from settings read before it was.
    pub fn recorded(&self, routes: impl IntoIterator<Item = String>) {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        if state.read {
            return;
        }
        state.read = true;
        state.recorded = routes.into_iter().collect();
    }

    /// Records `route` as said yes to, once the file holds it: the crate's
    /// own, so a yes is recorded only through [`Consent::accept`], which
    /// writes it down first. [`Consent::give`] lets a route go for the run
    /// alone, unwritten, while a `/login` waits for its credential.
    pub(crate) fn record(&self, route: &str) {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.given.remove(route);
        state.recorded.insert(route.to_owned());
    }

    /// Lets requests of `route` leave for as long as this run lasts or until
    /// [`Consent::withdraw`], writing nothing down: a `/login` choice, whose
    /// yes is recorded once its credential is stored.
    pub fn give(&self, route: &str) {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.given.insert(route.to_owned());
    }

    /// Whether `route` has a yes given and not yet written down.
    #[must_use]
    pub fn given(&self, route: &str) -> bool {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        state.given.contains(route)
    }

    /// Every route with a yes given and not yet written down.
    fn waiting(&self) -> BTreeSet<String> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        state.given.clone()
    }

    /// Takes back a yes that was given and never recorded.
    pub fn withdraw(&self, route: &str) {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.given.remove(route);
    }

    /// Forgets the yes of every route `gone` picks, given or recorded.
    pub fn forget(&self, gone: impl Fn(&str) -> bool) {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.given.retain(|route| !gone(route));
        state.recorded.retain(|route| !gone(route));
    }

    /// Takes what one provider's requests go on, or that it has nothing to
    /// send with.
    pub fn served(&self, provider: &str, serving: Option<Serving>) {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.stale.remove(provider);
        match serving {
            Some(serving) => state.serving.insert(provider.to_owned(), serving),
            None => state.serving.remove(provider),
        };
    }

    /// Takes what each provider's requests go on, in place of what was
    /// resolved before.
    pub fn serving(&self, serving: BTreeMap<String, Serving>) {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.stale.clear();
        state.serving = serving;
    }

    /// Takes what `provider` was served on as gone stale, to be read again
    /// at the next send or choice, whichever provider it is for. Until then
    /// it still claims its origin on the route it was served on, whose yes
    /// went with the credential, so the origin is held rather than decided by
    /// another provider's claim.
    fn unsettle(&self, provider: &str) {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.stale.insert(provider.to_owned());
    }
}

impl Hold for Consent {
    /// The route a request to `origin` waits on.
    ///
    /// Where a provider is served at `origin`, what it is served on decides: a
    /// route with no yes holds it, and an address crucible does not know, or a
    /// route with a yes, lets it through. Otherwise any warned route sent to
    /// `origin` with no yes holds it, which is what keeps a sign-in back
    /// before its row is chosen.
    fn held(&self, origin: &Origin) -> Option<Box<str>> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        let said = |route: &str| state.recorded.contains(route) || state.given.contains(route);
        let sent_to = |warned: &Warned| {
            warned
                .origins
                .iter()
                .any(|at| Origin::of(at).as_ref() == Some(origin))
        };

        let mut served = false;
        for serving in state.serving.values() {
            let warned = serving
                .route
                .as_deref()
                .and_then(|route| self.routes.warned(route));
            let claims = match (&serving.at, warned) {
                (Some(at), _) => at == origin,
                (None, Some(warned)) => sent_to(warned),
                (None, None) => false,
            };
            if !claims {
                continue;
            }
            match warned {
                Some(warned) if !said(warned.route) => return Some(warned.route.into()),
                Some(_) | None => served = true,
            }
        }
        if served {
            return None;
        }

        self.routes
            .all()
            .iter()
            .find(|warned| sent_to(warned) && !said(warned.route))
            .map(|warned| warned.route.into())
    }
}

/// What a store asks before a write takes a credential out: the yes of the
/// row it was given on, of every model route of its provider, and of the
/// route its provider's `baseUrl` answers for, taken out of the user's own
/// file `file` and then out of `consent`, except a yes a `/login` gave and
/// waits to write with this very credential. What that provider was served
/// on is taken as stale, still holding its origin, and read again at the
/// next send or choice, whichever provider it is for.
///
/// Changing or removing a `baseUrl` moves no yes; the address is read from
/// `settings` once, here, as the run was started with it.
#[must_use]
pub fn letting_go(consent: &Consent, file: PathBuf, rows: Rows, settings: &Settings) -> LettingGo {
    let consent = consent.clone();
    let based: BTreeMap<&'static str, &'static str> = rows
        .all()
        .iter()
        .filter_map(|row| {
            let route = recognised(settings.base_url(row.provider)?)?;
            Some((row.provider, route))
        })
        .collect();
    Arc::new(move |going: &[Dropped]| {
        let mut routes = BTreeSet::new();
        let mut providers = BTreeSet::new();
        for held in going {
            let provider = crucible_auth::provider_of(&held.name).to_owned();
            if let Some(row) = rows.of(held.kind, &held.name) {
                routes.insert(row_route(row));
            }
            if let Some(route) = based.get(provider.as_str()) {
                routes.insert((*route).to_owned());
            }
            providers.insert(provider);
        }
        // A yes given at `/login` and waiting for this very write is not
        // one that goes with it: the credential it waits for is the one
        // being written, whatever route it shares with what is dropped.
        let waiting = consent.waiting();
        let gone = |route: &str| {
            (routes.contains(route)
                || providers
                    .iter()
                    .any(|provider| route.starts_with(&model_route(provider, ""))))
                && !waiting.contains(route)
        };
        crate::remember::forgetting(&file, gone).map_err(|problem| {
            Box::<str>::from(format!(
                "the yes that goes with it could not be taken out: {problem}"
            ))
        })?;
        consent.forget(gone);
        // What each provider was served on may have gone with its
        // credential, or not: another may serve it still, and the write may
        // yet fail. Read again when next asked, from the store as it is then.
        for provider in &providers {
            consent.unsettle(provider);
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests;
