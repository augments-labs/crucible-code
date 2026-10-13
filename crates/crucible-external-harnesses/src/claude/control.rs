//! The questions the agent asks before a tool runs, and their answers.
//!
//! The agent asks with a `can_use_tool` request, and each is raised as a
//! pending permission described by what the tool would do. An allow sends the
//! call's own input back unchanged, so what runs is what was shown; a deny
//! sends a refusal the model reads. An allow for the rest of the session
//! carries only the rules the agent proposed that allow more of the same
//! tool, each kept for this session alone: a proposal to change the mode,
//! reach another directory or keep a rule in a settings file is dropped, so
//! the answer grants no more than the user chose and writes nothing to the
//! project.
//!
//! Every other request the agent makes of its host is refused, since this
//! host serves none. A question is answered once; at most [`PENDINGS`] are
//! open, and a turn's questions close with it.

use std::path::{Component, Path, PathBuf};

use crucible_execution::{Decision, Detail, Failure, Lasting, PENDINGS, Pending, Text};
use serde_json::{Value, json};

use super::wire::Ask;

/// What a question keeps until it is answered.
#[derive(Debug)]
struct Open {
    /// The id its answer carries.
    request: Box<str>,
    /// The call it is about.
    call: Box<str>,
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
                    && let Some(rules) = open.lasting.as_ref().map(for_the_session)
                    && !rules.is_empty()
                {
                    allow["updatedPermissions"] = Value::Array(rules);
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

/// Of the rules the agent proposed, the ones that only allow a tool, each
/// kept for this session alone.
fn for_the_session(proposed: &Value) -> Vec<Value> {
    proposed
        .as_array()
        .into_iter()
        .flatten()
        .filter(|rule| {
            rule.get("type").and_then(Value::as_str) == Some("addRules")
                && rule.get("behavior").and_then(Value::as_str) == Some("allow")
        })
        .filter_map(|rule| {
            let rules = rule.get("rules").filter(|rules| rules.is_array())?;
            Some(json!({
                "type": "addRules",
                "rules": rules,
                "behavior": "allow",
                "destination": "session",
            }))
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
            let path = ["file_path", "path", "notebook_path"]
                .into_iter()
                .find_map(field);
            if within(path, workspace) {
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

/// Whether `path` is inside `workspace`: a relative path is, unless it climbs
/// out, and an absolute one is when it begins with the workspace and never
/// climbs. With no path, or no workspace, it is not known to be.
fn within(path: Option<&str>, workspace: Option<&Path>) -> bool {
    let (Some(path), Some(workspace)) = (path, workspace) else {
        return false;
    };
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
    path.starts_with(workspace)
}
