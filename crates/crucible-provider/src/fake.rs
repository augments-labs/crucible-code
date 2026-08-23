//! What a test needs that only the rest of the harness can mint.
//!
//! A tool result carrying files is one of them. The files are admitted by the
//! verdict that let the tool run, so a body test cannot write one down — it has
//! to be issued, by the engine that issues every other one.

use crucible_core::{
    Ask, Attachment, Command, Modality, Permission, Remember, Sensitivity, Settled, ToolArgs,
    ToolCall, ToolId, ToolOutput, Verdict,
};

/// Says yes, once, to whatever it is shown.
struct Allows;

impl Ask for Allows {
    fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
        (Verdict::Allow, Remember::Never)
    }
}

/// What a tool answered with, and the files it was permitted to show.
pub(crate) fn found(text: &str, attachments: Vec<Attachment>) -> ToolOutput {
    showing(ToolOutput::ok(text), attachments)
}

/// The same, for a call that failed with something to show anyway.
pub(crate) fn failed(text: &str, attachments: Vec<Attachment>) -> ToolOutput {
    showing(ToolOutput::failed(text), attachments)
}

/// The verdict, issued, and the files it admits.
fn showing(output: ToolOutput, attachments: Vec<Attachment>) -> ToolOutput {
    let call = ToolCall {
        id: ToolId::new("call_1"),
        name: "bash".into(),
        args: ToolArgs::new("{}"),
    };
    let settled = Permission::new().decide(
        &call,
        &Sensitivity::SpawnsProcess {
            command: Command::Understood {
                sent: "ls".into(),
                parts: vec!["ls".into()].into(),
            },
        },
        &mut Allows,
    );

    let Settled::Approved(approved) = settled else {
        panic!("the fake said yes")
    };
    output.with_attachments(&approved, attachments)
}

/// One file a tool found, as the transcript records it.
///
/// No bytes: a provider reads what the runner resolved and never a path, so
/// this is only what says a result has a file at all — and how many.
pub(crate) fn picture() -> Attachment {
    Attachment {
        path: "pictures/holiday.png".into(),
        modality: Modality::Image,
        media_type: "image/png".into(),
        hash: [0xab; 32],
    }
}
