//! `crucible auth status`, `login` and `logout`: the login store said,
//! written and emptied from a command line.
//!
//! What is read, written and refused is [`crucible_app::auth`]'s to decide;
//! this file adapts a terminal to it. A report goes to standard output, and
//! everything said to the person at the terminal on the way, a prompt, a
//! page to visit, a refusal, goes to standard error, written [`visible`].
//!
//! A key arrives one of two ways and never as an argument: piped to
//! `--api-key-stdin`, which refuses a terminal on standard input because a
//! terminal would echo it, or typed at a hidden prompt, which needs a terminal
//! at both ends to hide it in. An account sign-in asks its questions on the
//! terminal too. A run that has no terminal for what it was asked to do says
//! what to run instead and fails, rather than waiting on input that cannot
//! come.
//!
//! [`visible`]: super::visible

use std::fmt::Write as _;
use std::io::{self, BufRead as _, IsTerminal as _, Read as _, Write as _};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use crucible_app::auth::{self, Desk, Login, MAX_SECRET, Refused, Secret, Signed, Signer, Way};
use crucible_app::content_use::Warned;
use crucible_config::Home;
use crucible_tui::{Key, Pressed, Raw};

#[cfg(test)]
mod tests;

/// The longest answer read to a question on the terminal, in bytes.
const MAX_ANSWER: u64 = 256;

/// Why a piped key was refused when standard input is a terminal.
const UNPIPED: &str = "--api-key-stdin reads a key piped to crucible, and standard input is a \
                       terminal, which would show it as it is typed; pipe the key in, or leave \
                       the flag off to be asked for it hidden";

/// Why a login with no pipe and no terminal was refused.
const UNASKED: &str = "there is no terminal to ask for the key in; pipe it to `crucible auth \
                       login PROVIDER --api-key-stdin`, or run the login in a terminal";

/// Why an account sign-in with no terminal was refused.
const UNSIGNED: &str = "signing in to an account asks questions on a terminal, and there is none \
                        here; run the login in a terminal, or store a key with `crucible auth \
                        login PROVIDER --api-key-stdin`";

/// Says which credential a launch would sign each provider in with, or the
/// one `word` names, and exits with what the report says.
pub(super) fn status(word: Option<&str>, json: bool) -> ExitCode {
    let now = now();
    let found = desk().and_then(|desk| {
        let provider = word.map(|word| desk.provider(word)).transpose()?;
        Ok(desk.status(provider, now))
    });
    match found {
        Ok(status) => {
            let report = if json {
                status.json()
            } else {
                super::visible(&status.human(now)).into_bytes()
            };
            let _ = io::stdout().write_all(&report);
            ExitCode::from(status.exit())
        }
        Err(problem) => {
            if json {
                let _ = io::stdout().write_all(&auth::failed(&problem));
            }
            refused(&problem.to_string())
        }
    }
}

/// Stores a key for the provider `word` names, read from standard input
/// where `piped`, or signs in to its account.
pub(super) fn login(word: &str, piped: bool) -> ExitCode {
    let outcome = desk()
        .map_err(|problem| problem.to_string())
        .and_then(|desk| {
            let login = desk
                .login(word, piped)
                .map_err(|problem| problem.to_string())?;
            match login {
                Login::Key(way) if piped => keyed(&desk, &way, &mut io::stdin().lock()),
                Login::Key(way) => typed(&desk, &way),
                Login::Account(way) => signed(&desk, &way),
                Login::Either { key, account } => either(&desk, &key, &account),
            }
        });
    match outcome {
        Ok(said) => {
            let _ = io::stdout().write_all(super::visible(&said).as_bytes());
            ExitCode::SUCCESS
        }
        Err(problem) => refused(&problem),
    }
}

/// Takes the credentials crucible stored for the provider `word` names out of
/// its login store, and says what a launch would still find.
pub(super) fn logout(word: &str) -> ExitCode {
    let outcome = desk().and_then(|desk| {
        let provider = desk.provider(word)?;
        desk.forget(provider)
    });
    match outcome {
        Ok(forgotten) => {
            let said = auth::forgotten(&forgotten);
            let _ = io::stdout().write_all(super::visible(&said).as_bytes());
            ExitCode::SUCCESS
        }
        Err(problem) => refused(&problem.to_string()),
    }
}

fn desk() -> Result<Desk, Refused> {
    Desk::open(
        Home::find(&|name| std::env::var_os(name)),
        Box::new(|name| std::env::var(name).ok()),
    )
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Writes `text` to standard error, where a person reads it.
fn said(text: &str) {
    let _ = io::stderr().write_all(super::visible(text).as_bytes());
}

fn refused(problem: &str) -> ExitCode {
    said(&format!("crucible: {problem}\n"));
    ExitCode::FAILURE
}

/// Stores the one key `input` holds under `way`, where it is not a terminal.
fn keyed(desk: &Desk, way: &Way, input: &mut dyn io::Read) -> Result<String, String> {
    if io::stdin().is_terminal() {
        return Err(UNPIPED.to_owned());
    }
    let secret = Secret::read(input).map_err(|problem| problem.to_string())?;
    desk.keep(way, &secret)
        .map(|kept| auth::kept(&kept))
        .map_err(|problem| problem.to_string())
}

/// Asks for `way`'s key at a hidden prompt and stores it.
fn typed(desk: &Desk, way: &Way) -> Result<String, String> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(UNASKED.to_owned());
    }
    let Some(typed) = hidden(way.shown())? else {
        return Err("nothing was typed, so nothing was stored".to_owned());
    };
    let secret = Secret::typed(&typed).map_err(|problem| problem.to_string())?;
    desk.keep(way, &secret)
        .map(|kept| auth::kept(&kept))
        .map_err(|problem| problem.to_string())
}

/// What was typed at a prompt that shows none of it, or `None` where it was
/// left with Esc, Ctrl-C or Ctrl-D.
fn hidden(shown: &str) -> Result<Option<String>, String> {
    let raw = match Raw::enter() {
        Ok(Some(raw)) => raw,
        Ok(None) => return Err(UNASKED.to_owned()),
        Err(problem) => {
            return Err(format!(
                "the terminal would not hide what is typed: {problem}"
            ));
        }
    };
    said(&format!(
        "{shown} API key (it does not show; Enter to store it, Esc to cancel): "
    ));
    let outcome = typing(std::iter::repeat_with(crucible_tui::pressed));
    drop(raw);
    said("\n");
    outcome
}

/// What `presses` type before Enter, or `None` where they leave with Esc,
/// Ctrl-C or Ctrl-D.
///
/// Held to one byte past [`MAX_SECRET`]. What arrives past that is not kept,
/// and the key it belonged to is refused as too long when Enter is pressed,
/// even after Backspace: the whitespace a trim would set aside may have made
/// room, and what is held then is a key cut to fit, not the one typed.
fn typing<E: std::fmt::Display>(
    presses: impl IntoIterator<Item = Result<Pressed, E>>,
) -> Result<Option<String>, String> {
    let room = MAX_SECRET.saturating_add(1);
    let mut typed = String::new();
    let mut overflowed = false;
    for pressed in presses {
        match pressed {
            Ok(Pressed::Key(Key::Enter)) if overflowed => {
                return Err(Refused::Oversized.to_string());
            }
            Ok(Pressed::Key(Key::Enter)) => return Ok(Some(typed)),
            Ok(Pressed::Key(Key::Char(one))) if typed.len() < room => typed.push(one),
            Ok(Pressed::Key(Key::Char(_))) => overflowed = true,
            Ok(Pressed::Pasted(text)) => {
                let left = room.saturating_sub(typed.len());
                let kept = cut(&text, left);
                overflowed |= kept.len() < text.len();
                typed.push_str(kept);
            }
            Ok(Pressed::Key(Key::Backspace)) => {
                typed.pop();
            }
            Ok(Pressed::Key(Key::Interrupt | Key::Eof) | Pressed::Escape) => return Ok(None),
            Ok(_) => {}
            Err(problem) => return Err(format!("the terminal could not be read: {problem}")),
        }
    }
    Ok(None)
}

/// `text` at most `bound` bytes long, cut on a character boundary.
fn cut(text: &str, bound: usize) -> &str {
    let mut end = bound.min(text.len());
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    text.get(..end).unwrap_or_default()
}

/// Asks whether `key` or `account` is meant, then logs in that way.
fn either(desk: &Desk, key: &Way, account: &Way) -> Result<String, String> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(UNASKED.to_owned());
    }
    let ways = [
        (
            key.shown(),
            "type an API key at a prompt that does not show it",
        ),
        (account.shown(), "sign in to the account"),
    ];
    match Asking.chooses("How do you want to sign in?", &ways) {
        Some(0) => typed(desk, key),
        Some(_) => signed(desk, account),
        None => Err("no way was chosen, so nothing was stored".to_owned()),
    }
}

/// Signs in to `way`'s account, asking on the terminal.
fn signed(desk: &Desk, way: &Way) -> Result<String, String> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(UNSIGNED.to_owned());
    }
    match desk.sign_in(way, &mut Asking) {
        Ok(Signed::In(kept)) => Ok(auth::kept(&kept)),
        Ok(Signed::Declined) => Err(format!(
            "the sign-in to {} was not started, so nothing was stored",
            way.shown()
        )),
        Err(problem) => Err(problem.to_string()),
    }
}

/// The terminal, asked what an account sign-in needs to know.
struct Asking;

impl Asking {
    /// One line from standard input, bounded at [`MAX_ANSWER`] bytes.
    fn answer() -> String {
        let mut line = String::new();
        let _ = io::stdin().lock().take(MAX_ANSWER).read_line(&mut line);
        line.trim().to_owned()
    }
}

impl Signer for Asking {
    fn agrees(&mut self, warned: &Warned) -> bool {
        said(&format!(
            "{}\n  {}\n  {}\n  {}\nSign in anyway? [y/N] ",
            warned.shown,
            warned.warning.sentence,
            warned.warning.cited(),
            warned.warning.link,
        ));
        matches!(Self::answer().to_ascii_lowercase().as_str(), "y" | "yes")
    }

    fn chooses(&mut self, title: &str, ways: &[(&'static str, &'static str)]) -> Option<usize> {
        let mut asked = format!("{title}\n");
        for (at, (shown, says)) in ways.iter().enumerate() {
            let _ = writeln!(asked, "  {}. {shown}: {says}", at.saturating_add(1));
        }
        let _ = write!(asked, "Which one? [1-{}, Enter for none] ", ways.len());
        said(&asked);
        Self::answer()
            .parse::<usize>()
            .ok()
            .and_then(|chosen| chosen.checked_sub(1))
            .filter(|at| *at < ways.len())
    }

    fn visit(&mut self, page: &str, code: Option<&str>, browser: &str, withheld: &[&str]) {
        let mut asked = format!("Finish signing in at {page}\n");
        if let Some(code) = code {
            let _ = writeln!(asked, "  and enter the code {code}");
        }
        said(&asked);
        if let Err(problem) = super::browser::open(browser, withheld.iter().copied()) {
            said(&format!(
                "  no browser could be opened ({problem}); open the page yourself\n"
            ));
        }
    }

    fn progress(&mut self, message: &str) {
        said(&format!("{message}\n"));
    }
}
