use crucible_tools::{Command, Host, Target};
use crucible_types::ToolArgs;

use super::*;
use crate::client::deciding::{effect, operation, outside};

/// A call to `name` sending `args`.
fn call(name: &str, args: &str) -> ToolCall {
    ToolCall {
        id: ToolId::new("a"),
        name: name.into(),
        args: ToolArgs::new(args),
    }
}

/// An action a native call raises and the same action an installed agent
/// raises are put to a client as one action: what a native call is, told as
/// the contract's detail and read back on the agent's side, is the effect
/// and the operation the native call is put as.
#[test]
fn a_detail_told_and_read_back_is_put_as_the_native_call_is() {
    let line = r#"{"command":"ls && rm -f important.txt"}"#;
    let left = r#"{"command":"make serve","run_in_background":true}"#;
    let cases = [
        (
            call("bash", line),
            Sensitivity::SpawnsProcess {
                command: Command::Understood {
                    sent: "ls && rm -f important.txt".into(),
                    parts: Box::new(["ls".into(), "rm -f important.txt".into()]),
                },
            },
        ),
        (
            call("bash", left),
            Sensitivity::SpawnsProcess {
                command: Command::Understood {
                    sent: "make serve".into(),
                    parts: Box::new(["make serve".into()]),
                },
            },
        ),
        (
            call("fetch", r#"{"url":"https://example.com/a?q=1"}"#),
            Sensitivity::ReachesNetwork {
                host: Host::Named {
                    sent: "https://example.com/a?q=1".into(),
                    host: "example.com".into(),
                },
            },
        ),
        (
            call("read", "{}"),
            Sensitivity::ReadOnly {
                target: Target::unresolved(),
            },
        ),
        (
            call("read", "{}"),
            Sensitivity::ReadsOutside {
                target: Target::unresolved(),
            },
        ),
        (
            call("write", "{}"),
            Sensitivity::MutatesFile {
                target: Target::unresolved(),
            },
        ),
    ];
    for (call, sensitivity) in cases {
        let (told, asked) = outside(&detail(&call, &sensitivity));

        assert_eq!(told, effect(&sensitivity), "{sensitivity:?}");
        assert_eq!(asked, operation(&call, &sensitivity), "{sensitivity:?}");
    }
}
