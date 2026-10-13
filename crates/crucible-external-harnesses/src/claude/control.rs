//! The questions the agent asks before a tool runs, and their answers.
//!
//! The agent asks with a `can_use_tool` request, and each is raised as a
//! pending permission described by what the tool would do. An allow sends the
//! call's own input back unchanged, so what runs is what was shown; a deny
//! sends a refusal the model reads. An allow for the rest of the session
//! carries only the agent's proposed rules for the command asked about, each
//! kept for this session alone: the command itself, or every command that
//! begins with the words the rule names, such as every `git` command for
//! `git:*`. Any other tool's rule, a rule with a wildcard, and a proposal to
//! change the mode, reach another directory or keep a rule in a settings file
//! is dropped, so the answer grants no more than the user chose and writes
//! nothing to the project. Where none is left, or the agent marks its rule as
//! allowing more than the ask, the allow lasts for the call alone.
//!
//! Every other request the agent makes of its host is refused, since this
//! host serves none. A question is answered once; at most [`PENDINGS`] are
//! open, and a turn's questions close with it.

use std::path::{Component, Path, PathBuf};

use crucible_execution::{Decision, Detail, Failure, Lasting, PENDINGS, Pending, Text};
use serde_json::{Value, json};

use super::wire::Ask;

/// The longest path a read is checked for on the filesystem, the most Linux
/// opens; a longer one is taken to be outside without looking.
const LONGEST_PATH: usize = 4096;

/// What a question keeps until it is answered.
#[derive(Debug)]
struct Open {
    /// The id its answer carries.
    request: Box<str>,
    /// The call it is about.
    call: Box<str>,
    /// The tool the call is of.
    tool: Box<str>,
    /// The call's input, sent back with an allow.
    input: Value,
    /// The rules the agent proposed for an answer that lasts.
    lasting: Option<Value>,
}

/// The questions the running turn has open.
#[derive(Debug, Default)]
pub(crate) struct Questions {
    /// Each, in the order asked.
    open: Vec<Open>,
}

impl Questions {
    /// Keeps `ask` until it is answered, and says what it asks, the workspace
    /// being `workspace` where one is known.
    ///
    /// # Errors
    ///
    /// A [`Failure`] where [`PENDINGS`] are open already.
    pub(crate) fn asked(&mut self, ask: Ask, workspace: Option<&Path>) -> Result<Pending, Failure> {
        if self.open.len() >= PENDINGS {
            return Err(Failure::new(
                "The agent asked more questions than a turn keeps open.",
            ));
        }
        let pending = pending(&ask, workspace);
        self.open.push(Open {
            request: ask.request.into(),
            call: ask.call.into(),
            tool: ask.tool.into(),
            input: ask.input,
            lasting: ask.lasting,
        });
        Ok(pending)
    }

    /// The frame that answers `native` with `decision`, which settles it.
    ///
    /// # Errors
    ///
    /// A [`Failure`] where `native` is not open, or `decision` is a written
    /// answer, which a permission cannot take.
    pub(crate) fn answer(&mut self, native: &str, decision: &Decision) -> Result<String, Failure> {
        if matches!(decision, Decision::Answer(_)) {
            return Err(Failure::new(
                "The agent asked for a permission, which a written answer cannot give.",
            ));
        }
        let place = self
            .open
            .iter()
            .position(|open| &*open.request == native)
            .ok_or_else(|| Failure::new("The agent has no question open under that name."))?;
        let open = self.open.remove(place);
        let behavior = match decision {
            Decision::Allow(lasting) => {
                let mut allow = json!({
                    "behavior": "allow",
                    "updatedInput": open.input,
                    "toolUseID": &*open.call,
                });
                if *lasting == Lasting::Session
                    && let Some(rules) = open
                        .lasting
                        .as_ref()
                        .map(|proposed| for_the_session(proposed, &open.tool, &open.input))
                    && !rules.is_empty()
                    && let Some(allow) = allow.as_object_mut()
                {
                    allow.insert("updatedPermissions".to_owned(), Value::Array(rules));
                }
                allow
            }
            Decision::Deny | Decision::Answer(_) => json!({
                "behavior": "deny",
                "message": "The user did not allow this.",
                "toolUseID": &*open.call,
            }),
        };
        Ok(json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": native, "response": behavior},
        })
        .to_string())
    }

    /// Forgets every question, as a new turn begins.
    pub(crate) fn clear(&mut self) {
        self.open.clear();
    }
}

/// The frame that refuses the request `request`, which this host does not
/// serve.
pub(crate) fn refusal(request: &str) -> String {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "error",
            "request_id": request,
            "error": "This host does not serve that request.",
        },
    })
    .to_string()
}

/// Of the rules the agent proposed, the ones for `tool` that allow the call
/// whose input is `input`, each kept for this session alone.
fn for_the_session(proposed: &Value, tool: &str, input: &Value) -> Vec<Value> {
    proposed
        .as_array()
        .into_iter()
        .flatten()
        .filter(|rule| {
            rule.get("type").and_then(Value::as_str) == Some("addRules")
                && rule.get("behavior").and_then(Value::as_str) == Some("allow")
        })
        .filter_map(|rule| {
            let rules: Vec<&Value> = rule
                .get("rules")
                .and_then(Value::as_array)?
                .iter()
                .filter(|rule| {
                    rule.get("toolName").and_then(Value::as_str) == Some(tool)
                        && rule
                            .get("ruleContent")
                            .and_then(Value::as_str)
                            .is_some_and(|content| allows_the_ask(content, tool, input))
                })
                .collect();
            (!rules.is_empty()).then(|| {
                json!({
                    "type": "addRules",
                    "rules": rules,
                    "behavior": "allow",
                    "destination": "session",
                })
            })
        })
        .collect()
}

/// What `ask` asks, the workspace being `workspace` where one is known.
fn pending(ask: &Ask, workspace: Option<&Path>) -> Pending {
    let field = |name: &str| ask.input.get(name).and_then(Value::as_str);
    let detail = match ask.tool.as_str() {
        "Bash" => Detail::Command {
            sent: Text::new(field("command").unwrap_or_default()),
            left_running: ask.input.get("run_in_background").and_then(Value::as_bool) == Some(true),
        },
        "Read" | "Grep" | "Glob" | "LS" | "NotebookRead" => {
            // Each tool reads the field it names; a search given no place
            // starts at the workspace.
            let path = match ask.tool.as_str() {
                "Read" => field("file_path"),
                "NotebookRead" => field("notebook_path"),
                _ => ask.input.get("path").map_or(Some("."), Value::as_str),
            };
            // A glob's pattern is a path of its own, read from where the
            // glob starts.
            let pattern = (ask.tool == "Glob").then(|| field("pattern")).flatten();
            if within(path, workspace)
                && pattern.is_none_or(|pattern| {
                    within(Some(pattern), workspace)
                        && path.is_some_and(|path| {
                            within(Some(&format!("{path}/{pattern}")), workspace)
                        })
                })
            {
                Detail::Reads
            } else {
                Detail::ReadsOutside
            }
        }
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => Detail::MutatesFile,
        "WebFetch" => Detail::Network {
            sent: Text::new(field("url").unwrap_or_default()),
        },
        "WebSearch" => Detail::Network {
            sent: Text::new(field("query").unwrap_or_default()),
        },
        // A tool of no reviewed kind may do anything, so it is asked about as
        // the most a call can do, with its input as it would be sent.
        _ => Detail::Command {
            sent: Text::new(&ask.input.to_string()),
            left_running: false,
        },
    };
    Pending::Permission {
        tool: Text::new(&ask.tool),
        summary: Text::new(ask.title.as_deref().unwrap_or(&ask.tool)),
        detail,
    }
}

/// Whether a rule for `tool` whose content is `content` allows the call whose
/// input is `input` and is no wider than the user can see it to be. Only a
/// command's rule is: the command itself, or with `:*` after it the commands
/// that begin with those words, each separated by a plain space. A rule with a
/// wildcard or another kind of space in what it names is never kept, nor one
/// with no letter or digit, nor any other tool's, whose rules the agent may
/// read as more than they say.
fn allows_the_ask(content: &str, tool: &str, input: &Value) -> bool {
    let prefix = content.strip_suffix(":*");
    let named = prefix.unwrap_or(content);
    tool == "Bash"
        && named.chars().any(char::is_alphanumeric)
        && named.trim_matches(' ') == named
        && !named.chars().any(|character| {
            matches!(character, '*' | '?' | '[' | '{' | '\u{feff}')
                || (character.is_whitespace() && character != ' ')
        })
        && input
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| {
                command == named
                    || prefix.is_some_and(|words| {
                        command
                            .strip_prefix(words)
                            .is_some_and(|rest| rest.starts_with(' '))
                    })
            })
}

/// Whether `path` is inside `workspace`: a relative path is, unless it climbs
/// out or starts at the home, and an absolute one is when it begins with the
/// workspace and never climbs. Where the place exists, or a directory above
/// it does, a link is followed, and it must lead inside too; a link that
/// leads nowhere is not known to. With no path, no workspace, or a path
/// longer than [`LONGEST_PATH`], it is not known to be.
fn within(path: Option<&str>, workspace: Option<&Path>) -> bool {
    let (Some(path), Some(workspace)) = (path, workspace) else {
        return false;
    };
    if path.starts_with('~') || path.len() > LONGEST_PATH {
        return false;
    }
    let path = Path::new(path);
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return false;
    }
    let path: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    if !path.starts_with(workspace) {
        return false;
    }
    let Ok(workspace) = workspace.canonicalize() else {
        return true;
    };
    path.ancestors()
        .find(|place| place.symlink_metadata().is_ok())
        .is_none_or(|place| {
            place
                .canonicalize()
                .is_ok_and(|place| place.starts_with(&workspace))
        })
}
