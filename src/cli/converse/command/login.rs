//! `/login`: a subscription or console credential, selected without exposing
//! a secret on the command line.
//!
//! With no argument, the first panel asks how the account is billed. A real
//! subscription implementation starts its own bounded worker and reports the
//! page the user must visit; the terminal thread continues serving resize and
//! cancellation. Console credentials reach the provider panel and a key box.
//! A vendor named directly keeps the existing API-key shortcut.
//!
//! A key typed after a command is a key in the shell's history file, in the
//! process listing while the command runs, and in that shell's own scrollback
//! afterwards — three places it was never meant to be, none of which this
//! program could clear. So the two halves are asked for separately: who the key
//! is for, out loud, and the key itself into a box.
//!
//! What that writes is then read straight back, and the session is handed the
//! provider it buys — unless another provider is already answering, in which
//! case the session keeps the provider and the model it has and the line points
//! at `/model`, where switching is chosen rather than implied. A key is given
//! by somebody who wants to type at the screen in front of them, and the file
//! it lands in is what makes the run after this one ask the same thing rather
//! than what makes this one ask at all.
//!
//! A run with no key for anything is left alone to draw its prompt. The warning
//! under the welcome names this command and `/model` both, which is the whole of
//! what somebody meeting crucible for the first time has to read; a panel
//! standing in front of that prompt would be this program answering a question
//! nobody asked it, on the one screen where the reader is still finding out
//! where they are.
//!
//! Naming somebody this build has never heard of, a panel that was left, and a
//! window with no room to stand one in all come out the same way: which names
//! crucible knows and which variable each of them signs a request from, written
//! into the transcript where it can be scrolled back to. Every one of those
//! halves comes off [`PROVIDERS`], so a vendor this build serves and cannot be
//! logged in to is not a state that exists.

use std::borrow::Cow;
use std::time::Duration;

use crucible_auth::{LoginAttempt, LoginUpdate};
use crucible_runner::Runner;
use crucible_tui::{
    Caret, Glyphs, Key, Offered, Panel, Pressed, Renderer, Row, Slot, Terminal, characters, clip,
    pressed,
};

use crate::cli::converse::picking::{self, Picked, Taken};
use crate::cli::converse::secret;
use crate::cli::subscription::{Account, Route};
use crate::cli::{Fatal, PROVIDERS, Served, remember};

use super::{Terms, about, say};

/// One way Crucible can receive a credential.
#[derive(Clone, Copy)]
struct Way {
    shown: &'static str,
    plan: &'static str,
    says: &'static str,
    reaches: Reaches,
}

/// What selecting a way does.
#[derive(Clone, Copy)]
enum Reaches {
    Account(Account),
    Console,
}

/// The sentence under the account-kind panel.
const HOW: &str = "Choose how crucible signs its requests.";

/// The sentence under the provider panel.
const SAID: &str = concat!(
    "Choose the provider whose API key you have. The key is typed into a box ",
    "that does not echo it, and signs requests from the next turn on."
);

/// The one key worth naming on either panel: the arrows and Enter are what a
/// list with a mark on it is already saying.
const CANCEL: &str = "esc to cancel";

/// What escape leaves behind, in place of the rows it used to write.
const LEFT: &str = "cancelled, nothing signed in";

/// Manual callback input is transient credential material. It has the same
/// bound as the key box and is never committed or echoed.
const MAX_MANUAL: usize = 16 * 1024;

/// Runs it: a key taken for the one named, one chosen off the panel, or where
/// each of them reads a key from.
///
/// `keys` is whether there is a keyboard to take one from. Down a pipe there is
/// not, and a panel or a box waiting for something nobody can type is a session
/// that stopped — so what a piped run gets is the rows naming the variables
/// instead.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    let named = PROVIDERS.into_iter().find(|one| one.name == said);

    if keys {
        if let Some(named) = named {
            return given(named, renderer, runner, terms);
        }

        // Nobody named and a keyboard to walk the account and provider lists.
        if said.is_empty() && walked(renderer, runner, terms)? {
            return Ok(());
        }
    }

    // The word came off the line and was never shape-checked — anything at all
    // can follow `/login ` — so it goes out the way arrived text goes out, and
    // the names that would have worked go under it.
    if named.is_none() && !said.is_empty() {
        renderer.commit(&format!("! no provider called {said}"))?;
    }

    let columns = renderer.columns();
    let rows: Vec<Row> = PROVIDERS
        .into_iter()
        .filter(|one| named.is_none_or(|only| only.name == one.name))
        .map(|one| {
            let said = about(
                &format!("/login {}", one.name),
                &format!("a key from {}", one.key),
                terms.style().glyphs(),
            );

            Row::new().then(Slot::Quiet, clip(&said, columns))
        })
        .collect();

    Ok(renderer.present(&rows)?)
}

/// Walks the account route, then only the provider question that route needs.
///
/// `false` is a window with no room to stand a panel in, and only that: the
/// caller draws the rows instead, which is the one answer a short window can be
/// given. Escape comes back `true`, because leaving a panel is an answer — the
/// screen from before it is what was asked for, and a list of every provider
/// underneath would be the same question put a second time.
fn walked<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<bool, Fatal> {
    let ways = ways(terms);
    let way = match asked(renderer, terms, &ways)?.of(&ways) {
        Taken::Took(way) => way,
        Taken::Left => {
            say(renderer, LEFT)?;
            return Ok(true);
        }
        Taken::Cramped => return Ok(false),
    };

    match way.reaches {
        Reaches::Account(account) => {
            let routes = terms.subscriptions.routes(account.provider());
            let route = match routes.as_slice() {
                [only] => *only,
                _ => match method(renderer, terms, account, &routes)?.of(&routes) {
                    Taken::Took(route) => route,
                    Taken::Left => {
                        say(renderer, LEFT)?;
                        return Ok(true);
                    }
                    Taken::Cramped => return Ok(false),
                },
            };
            subscribed(route, renderer, runner, terms)?;
            return Ok(true);
        }
        Reaches::Console => {}
    }

    let named = match chosen(renderer, terms)?.of(&PROVIDERS) {
        Taken::Took(named) => named,
        Taken::Left => {
            say(renderer, LEFT)?;
            return Ok(true);
        }
        Taken::Cramped => return Ok(false),
    };

    given(named, renderer, runner, terms)?;

    Ok(true)
}

fn ways(terms: &Terms) -> Vec<Way> {
    let mut ways: Vec<_> = terms
        .subscriptions
        .accounts()
        .iter()
        .map(|account| Way {
            shown: account.shown,
            plan: account.plan,
            says: account.says,
            reaches: Reaches::Account(*account),
        })
        .collect();
    ways.push(Way {
        shown: "Console account",
        plan: "API usage billing",
        says: "enter a provider API key",
        reaches: Reaches::Console,
    });
    ways
}

/// What each way says at the right of its row: the plan the account is billed
/// under, and what taking the row does.
///
/// Joined here rather than written out whole, so the mark between the two
/// halves is the one the setting names — the same mark that stands between a
/// command and the sentence about it in the listing a run with no keyboard is
/// given.
fn sentences(ways: &[Way], glyphs: Glyphs) -> Vec<String> {
    ways.iter()
        .map(|way| about(way.plan, way.says, glyphs))
        .collect()
}

/// Stands the account-kind panel.
fn asked<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    ways: &[Way],
) -> Result<Picked, Fatal> {
    let says = sentences(ways, terms.style().glyphs());
    let shown: Vec<_> = ways
        .iter()
        .zip(&says)
        .map(|(way, says)| Offered {
            name: way.shown,
            says,
        })
        .collect();
    picking::pick(
        renderer,
        terms.style(),
        Panel {
            title: "Log in",
            said: Some(HOW),
            shown: &shown,
            chosen: 0,
            footer: CANCEL,
        },
    )
}

/// Asks how a provider with more than one authorization method should open.
fn method<T: Terminal>(
    renderer: &mut Renderer<T>,
    terms: &Terms,
    account: Account,
    routes: &[Route],
) -> Result<Picked, Fatal> {
    let shown: Vec<_> = routes
        .iter()
        .map(|route| Offered {
            name: route.shown,
            says: route.says,
        })
        .collect();
    picking::pick(
        renderer,
        terms.style(),
        Panel {
            title: account.shown,
            said: Some("Choose where to finish account authorization."),
            shown: &shown,
            chosen: 0,
            footer: CANCEL,
        },
    )
}

/// Stands the provider panel and says how it ended.
fn chosen<T: Terminal>(renderer: &mut Renderer<T>, terms: &Terms) -> Result<Picked, Fatal> {
    // Where else the same key can come from — and the one thing that differs
    // between rows that would otherwise read identically.
    let says: Vec<String> = PROVIDERS
        .iter()
        .map(|one| format!("typed here, or set in {}", one.key))
        .collect();

    let shown: Vec<Offered<'_>> = PROVIDERS
        .iter()
        .zip(&says)
        .map(|(one, says)| Offered {
            name: one.shown,
            says,
        })
        .collect();

    let panel = Panel {
        title: "Log in",
        said: Some(SAID),
        shown: &shown,
        chosen: 0,
        footer: CANCEL,
    };

    picking::pick(renderer, terms.style(), panel)
}

/// Runs a registered subscription flow and switches this session on success.
fn subscribed<T: Terminal>(
    route: Route,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    let provider = route.provider();
    if !terms.subscriptions.supports(provider) {
        return say(renderer, &format!("! no subscription login for {provider}"));
    }
    let attempt = match terms.subscriptions.start(route, terms.logins.clone()) {
        Ok(attempt) => attempt,
        Err(problem) => return say(renderer, &format!("! {problem}")),
    };

    let mut view = LoginView::new(terms.style().glyphs());
    view.show(renderer, terms, route.title())?;
    loop {
        match attempt.wait(Duration::from_millis(50)) {
            Ok(Some(update)) => {
                if view.apply(update) {
                    let Some(named) = PROVIDERS.into_iter().find(|one| one.name == provider) else {
                        return say(renderer, "! the signed-in provider is unavailable");
                    };
                    return taken(named, renderer, runner, terms);
                }
                view.show(renderer, terms, route.title())?;
            }
            Ok(None) => {}
            Err(problem) => return say(renderer, &format!("! {problem}")),
        }

        let Some(arrived) = view.key(renderer)? else {
            continue;
        };
        match arrived {
            Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => {
                attempt.cancel();
                return say(renderer, LEFT);
            }
            Pressed::Resized => renderer.resized()?,
            pressed => {
                if !view.press(pressed, &attempt)? {
                    continue;
                }
            }
        }
        view.show(renderer, terms, route.title())?;
    }
}

struct LoginView {
    page: Option<(Box<str>, Option<Box<str>>)>,
    status: Cow<'static, str>,
    browser_failed: bool,
    accepts_manual: bool,
    manual: String,
    limited: bool,
    following: Option<Pressed>,
}

impl LoginView {
    fn new(glyphs: Glyphs) -> Self {
        Self {
            page: None,
            status: Cow::Owned(about("waiting", CANCEL, glyphs)),
            browser_failed: false,
            accepts_manual: false,
            manual: String::new(),
            limited: false,
            following: None,
        }
    }

    fn apply(&mut self, update: LoginUpdate) -> bool {
        match update {
            LoginUpdate::Authorize {
                browser_uri,
                shown_uri,
                user_code,
                manual,
            } => {
                self.browser_failed = crate::cli::browser::open(&browser_uri).is_err();
                self.page = Some((shown_uri, user_code));
                self.accepts_manual = manual;
                self.status = Cow::Borrowed("a browser should open; waiting for authorization…");
                false
            }
            LoginUpdate::Progress { message } => {
                self.accepts_manual = false;
                self.manual.clear();
                self.status = Cow::Borrowed(message);
                false
            }
            LoginUpdate::Complete => true,
        }
    }

    fn key<T: Terminal>(&mut self, renderer: &mut Renderer<T>) -> Result<Option<Pressed>, Fatal> {
        if let Some(following) = self.following.take() {
            return Ok(renderer.took(following)?);
        }
        if !renderer.waiting(Duration::ZERO)? {
            return Ok(None);
        }
        Ok(renderer.took(pressed()?)?)
    }

    // An event token is handed over, not lent: the handler takes the one thing
    // the reader produced, and a reference would say the caller kept a say in it.
    #[allow(clippy::needless_pass_by_value)]
    fn press(&mut self, pressed: Pressed, attempt: &LoginAttempt) -> Result<bool, Fatal> {
        match pressed {
            Pressed::Key(Key::Char(first)) if self.accepts_manual => {
                let room = MAX_MANUAL.saturating_sub(self.manual.len());
                let (text, refused, after) = characters(first, room)?.into_parts();
                self.manual.push_str(&text);
                self.limited = refused;
                self.following = after;
                Ok(true)
            }
            Pressed::Key(Key::Backspace) if self.accepts_manual => {
                self.limited = false;
                Ok(self.manual.pop().is_some())
            }
            Pressed::Key(Key::Enter) if self.accepts_manual && !self.manual.trim().is_empty() => {
                match attempt.submit(&self.manual) {
                    Ok(()) => {
                        self.manual.clear();
                        self.accepts_manual = false;
                        self.status = Cow::Borrowed("checking pasted authorization…");
                    }
                    Err(problem) => self.status = Cow::Owned(format!("! {problem}")),
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn show<T: Terminal>(
        &self,
        renderer: &mut Renderer<T>,
        terms: &Terms,
        title: &str,
    ) -> Result<(), Fatal> {
        let (rows, caret) = self.frame(renderer.columns(), title, terms.style().glyphs());
        renderer.live(&rows, caret, terms.style().palette())?;
        Ok(())
    }

    fn frame(&self, columns: usize, title: &str, glyphs: Glyphs) -> (Vec<Row>, Caret) {
        let mut rows = Vec::with_capacity(7);
        rows.push(Row::new().then(Slot::Strong, clip(title, columns)));
        if let Some((url, code)) = &self.page {
            rows.push(Row::new().then(Slot::Plain, clip(&format!("Open {url}"), columns)));
            if let Some(code) = code {
                rows.push(
                    Row::new().then(Slot::Strong, clip(&format!("Enter code {code}"), columns)),
                );
            }
            if self.browser_failed {
                rows.push(Row::new().then(
                    Slot::Quiet,
                    clip("browser did not open; use the page above", columns),
                ));
            }
        } else {
            rows.push(Row::new().then(Slot::Quiet, clip("starting sign-in…", columns)));
        }
        if !self.accepts_manual {
            rows.push(Row::new().then(Slot::Quiet, clip(&self.status, columns)));
            let caret = Caret {
                row: rows.len().saturating_sub(1),
                column: 0,
            };
            return (rows, caret);
        }

        rows.push(Row::new().then(
            Slot::Quiet,
            clip("Paste the callback URL or code, then press Enter.", columns),
        ));
        // Both marks are one column in either set, so the mark and the space
        // after it take two columns wherever this is drawn, and the caret below
        // sits one column past as many of them as there are characters.
        let mark = format!("{} ", glyphs.caret());
        let room = columns.saturating_sub(2);
        let dots = glyphs
            .hidden()
            .repeat(self.manual.chars().count().min(room));
        rows.push(
            Row::new()
                .then(Slot::Accent, clip(&mark, columns))
                .then(Slot::Plain, &dots),
        );
        let hint = if self.limited {
            "authorization input is limited to 16 KiB"
        } else {
            &self.status
        };
        rows.push(Row::new().then(Slot::Quiet, clip(hint, columns)));
        let caret = Caret {
            row: rows.len().saturating_sub(2),
            column: (2 + dots.chars().count()).min(columns.saturating_sub(1)),
        };
        (rows, caret)
    }
}

/// Asks for a key, writes it down, and sets this session up with it.
fn given<T: Terminal>(
    named: Served,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    let Some(key) = secret::ask(renderer, terms.style(), &format!("{} api key", named.name))?
    else {
        return say(renderer, "nothing was written");
    };

    match terms.logins.keep(named.name, &key) {
        Ok(()) => taken(named, renderer, runner, terms),

        // The key is still in hand and the box is gone, so there is nothing to
        // retry from — which is why this says what stopped rather than only
        // that something did.
        Err(problem) => say(renderer, &format!("! {problem}")),
    }
}

/// Hands this session the provider whose credential was just written down —
/// unless another is already answering, which keeps everything it has.
///
/// The credential is on disk, so this run is now the run the next launch would
/// be, and reading it back through the same resolution is what makes that true
/// here instead of only at the next start. What it costs is a second read of a
/// file written a line ago; what it buys is that somebody who has just stored a
/// credential can type at the session in front of them.
///
/// Authentication never chooses a model or effort, and never retires the pair
/// in force. Where nothing names a model, the line says so; where a provider is
/// already answering, the line points at `/model` instead — that is the other
/// half of a first minute, and a session that stopped at "credential stored"
/// would leave the reader to find that out from the next refusal.
fn taken<T: Terminal>(
    named: Served,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    let set = match (terms.serving)(named, &terms.logins.read()) {
        Ok(set) => set,

        // Written and unusable, which is exactly what the next run would meet.
        // Said now rather than left for it: a session that looked configured and
        // refused every turn is the state this whole command exists to end.
        Err(problem) => return say(renderer, &format!("! {problem}")),
    };

    // A credential says a provider can be reached and never which to ask, so
    // where another provider is already answering, it keeps the session it
    // has: the model in force belongs to that vendor, and pulling both out
    // from under the reader to honour a stored key would turn "add a second
    // credential" into "lose the conversation's setup". `/model` is the one
    // place the provider and the model change, and they change together there.
    if terms
        .provider
        .get()
        .is_some_and(|serving| serving != named.name)
    {
        drop(set);
        return say(
            renderer,
            &format!("logged in to {}; /model switches to it", named.name),
        );
    }

    let changed = terms.provider.get() != Some(named.name);
    runner.serve(set.provider);
    terms.provider.set(Some(named.name));

    // Written down as well as switched to, so the next run here opens on the
    // provider whose credential was stored instead of asking again — a
    // credential says a provider can be reached and never which to ask, and
    // this command is somebody saying which. A failure loses the half that
    // outlives the process and not the session in front of the reader, which is
    // the bargain `/model` is on.
    if let Err(problem) = remember::asking(&terms.choosing, named.name) {
        say(renderer, &format!("! {problem}"))?;
    }

    // A model belongs to the vendor serving it, so a session that had no
    // provider retires any name left over from before it signed out: sending
    // it to the vendor just served is the mismatch the refusal would otherwise
    // arrive with.
    if changed {
        runner.ask("", crate::cli::startup::UNKNOWN_CEILING, None, None);
    }

    let said = if runner.model().is_empty() {
        format!("logged in to {}; choose a model with /model", named.name)
    } else {
        format!("logged in to {}, asking {}", named.name, runner.model())
    };

    say(renderer, &said)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use crucible_auth::Store;
    use crucible_core::{AgentId, Cancel, Revealed};
    use crucible_runner::{AgentSpec, Model, Tools};
    use crucible_tools::{Ledger, Plan};
    use crucible_tui::Recording;

    use crate::cli::fake::Script;
    use crate::cli::sample::Sample;
    use crate::cli::style::Style;

    use super::*;

    /// Terms whose session is already answered by `anthropic`, and whose
    /// serving closure resolves any credential without reaching a network.
    fn in_force(sample: &Sample) -> Terms {
        Terms {
            style: Cell::new(Style::plain()),
            chosen: Cell::new(None),
            reading: std::cell::RefCell::default(),
            cancel: Cancel::new(),
            steer: crucible_core::Steer::new(),
            aside: crucible_core::Aside::new(),
            ledger: Ledger::new(),
            revealed: Revealed::new(),
            plan: Plan::new(),
            putting: crate::cli::seen::Putting::new(),
            leaving: crucible_tools::Background::new(),
            provider: Cell::new(Some("anthropic")),
            pending_model: Cell::new(None),
            pending_mode: Cell::new(None),
            settings: crucible_config::Settings::default(),
            choosing: sample.root().join("unwritten-home.json"),
            logins: Store::in_home(&sample.root()),
            subscriptions: crate::cli::subscription::Subscriptions::production(),
            serving: Box::new(|named, _| {
                Ok(crate::cli::Resolved {
                    provider: Box::new(crucible_provider::Unavailable::new(
                        crate::cli::NOTHING_TO_ASK,
                    )),
                    source: crate::cli::CredentialSource::Environment(named.key.into()),
                })
            }),
            sessions: sample.logs(),
            workspace: sample.workspace(),
            sending: crucible_tui::Sending::default(),
        }
    }

    /// A runner already asking `model`, answering from an empty script.
    fn asking(model: &str) -> Runner {
        Runner::new(
            Box::new(Script::new(Vec::new())),
            Tools::new(),
            AgentSpec::new(
                AgentId::new("test"),
                Model {
                    name: model.into(),
                    max_tokens: 64,
                    window: None,
                    accepts: None,
                    effort: None,
                },
            ),
            crucible_runner::ContextInputs::new(std::env::temp_dir()),
            crucible_runner::Session::nowhere(),
        )
    }

    #[test]
    fn a_credential_stored_while_a_provider_is_in_force_leaves_the_session_as_it_is() {
        // A credential says a provider can be reached and never which to ask.
        // The session in front of the reader is already answering, and a login
        // that pulled its provider and model out from under it would turn
        // "store a second key" into "lose the conversation's setup".
        let sample = Sample::new("login-in-force");
        let terms = in_force(&sample);
        let mut runner = asking("claude-test-1");
        let mut renderer = Renderer::new(Recording::new(80, 24));

        let named = *PROVIDERS
            .iter()
            .find(|served| served.name == "openai")
            .expect("a provider this build has an arm for");
        taken(named, &mut renderer, &mut runner, &terms).expect("the terminal to be written");

        assert_eq!(runner.model(), "claude-test-1");
        assert_eq!(terms.provider.get(), Some("anthropic"));

        let written = renderer.terminal().written().to_string();
        assert!(
            written.contains("/model"),
            "the way to switch is named: {written}"
        );
        assert!(
            !sample.root().join("unwritten-home.json").exists(),
            "which provider to open on is still the reader's standing choice"
        );
    }

    #[test]
    fn a_credential_stored_for_the_provider_in_force_keeps_its_model() {
        // Re-entering a key for the provider already answering is a renewal,
        // not a switch: the new credential signs the next request, and the
        // model in force goes on being the one asked.
        let sample = Sample::new("login-renewed");
        let terms = in_force(&sample);
        let mut runner = asking("claude-test-1");
        let mut renderer = Renderer::new(Recording::new(80, 24));

        let named = *PROVIDERS
            .iter()
            .find(|served| served.name == "anthropic")
            .expect("a provider this build has an arm for");
        taken(named, &mut renderer, &mut runner, &terms).expect("the terminal to be written");

        assert_eq!(runner.model(), "claude-test-1");
        let written = renderer.terminal().written().to_string();
        assert!(written.contains("asking claude-test-1"), "{written}");
    }

    #[test]
    fn browser_login_shows_the_short_page_and_masks_manual_input() {
        let mut view = LoginView::new(Glyphs::Unicode);
        view.page = Some(("http://localhost:1455/launch".into(), None));
        view.status = Cow::Borrowed("a browser should open; waiting for authorization…");
        view.accepts_manual = true;
        view.manual = "secret-callback".to_owned();
        let (rows, caret) = view.frame(80, "Log in to ChatGPT", Glyphs::Unicode);
        let text: Vec<_> = rows.iter().map(Row::text).collect();

        assert_eq!(text.first().map(String::as_str), Some("Log in to ChatGPT"));
        assert!(text.iter().any(|row| row.contains("localhost:1455/launch")));
        let input = text.iter().find(|row| row.starts_with("› ")).unwrap();
        assert_eq!(input.chars().skip(2).count(), "secret-callback".len());
        assert!(input.chars().skip(2).all(|character| character == '•'));
        assert!(!text.iter().any(|row| row.contains("secret-callback")));
        assert!(!text.iter().any(|row| row.contains("oauth/authorize")));
        assert_eq!(caret.row, rows.len() - 2);
    }

    #[test]
    fn the_paste_box_draws_its_mark_and_its_dots_out_of_the_glyph_set() {
        // A second box a line is typed into, so it takes the same mark the
        // prompt takes and hides what is typed with the same one the key box
        // hides a key with. A terminal whose font has neither would otherwise
        // get hollow squares on the row where the sign-in is asking for the one
        // thing it will not show back.
        for (glyphs, mark, hidden) in [(Glyphs::Unicode, "› ", '•'), (Glyphs::Ascii, "> ", '*')]
        {
            let mut view = LoginView::new(glyphs);
            view.page = Some(("http://localhost:1455/launch".into(), None));
            view.accepts_manual = true;
            view.manual = "pasted".to_owned();

            let (rows, caret) = view.frame(80, "Log in to ChatGPT", glyphs);
            let text: Vec<_> = rows.iter().map(Row::text).collect();
            let input = text
                .iter()
                .find(|row| row.starts_with(mark))
                .unwrap_or_else(|| panic!("{glyphs:?}: {text:?}"));

            assert!(
                input.chars().skip(2).all(|character| character == hidden),
                "{glyphs:?}: {input}"
            );
            assert_eq!(caret.column, 2 + "pasted".len(), "{glyphs:?}");
        }
    }

    #[test]
    fn what_parts_the_plan_from_what_taking_a_row_does_comes_out_of_the_glyph_set() {
        // The row names the plan the account is billed under and what taking it
        // does, and the mark is what says they are two. A terminal that cannot
        // draw it gets a question mark in the middle of the sentence somebody
        // is reading to decide how they are about to sign in.
        let ways = [Way {
            shown: "Console account",
            plan: "API usage billing",
            says: "enter a provider API key",
            reaches: Reaches::Console,
        }];

        assert_eq!(
            sentences(&ways, Glyphs::Unicode),
            ["API usage billing — enter a provider API key"]
        );
        assert_eq!(
            sentences(&ways, Glyphs::Ascii),
            ["API usage billing -- enter a provider API key"]
        );
    }

    #[test]
    fn the_row_saying_the_sign_in_is_waiting_takes_its_mark_from_the_set() {
        // The row under a sign-in that has not finished is a state and the key
        // that leaves it, parted by the same mark. It is the row somebody looks
        // at while nothing is happening, so it is the one that would sit there
        // with a hollow square in it the longest.
        for (glyphs, said) in [
            (Glyphs::Unicode, "waiting — esc to cancel"),
            (Glyphs::Ascii, "waiting -- esc to cancel"),
        ] {
            let view = LoginView::new(glyphs);
            let (rows, _) = view.frame(80, "Log in to ChatGPT", glyphs);
            let text: Vec<String> = rows.iter().map(Row::text).collect();

            assert!(text.iter().any(|row| row == said), "{glyphs:?}: {text:?}");
        }
    }

    #[test]
    fn every_login_row_and_its_caret_fit_a_narrow_terminal() {
        let mut view = LoginView::new(Glyphs::Unicode);
        view.page = Some((
            "http://localhost:1455/launch".into(),
            Some("ABCD-EFGH".into()),
        ));
        view.status = Cow::Borrowed("waiting");
        view.browser_failed = true;
        view.accepts_manual = true;
        view.manual = "pasted-code".to_owned();
        view.limited = true;
        for columns in 0..=32 {
            let (rows, caret) = view.frame(columns, "Log in to ChatGPT", Glyphs::Unicode);
            assert!(rows.iter().all(|row| row.columns() <= columns));
            assert!(caret.column <= columns.saturating_sub(1));
            assert!(caret.row < rows.len());
        }
    }
}
