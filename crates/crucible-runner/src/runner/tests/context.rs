//! Per-pass context assembly, including the history-rewrite adversary.

use std::fs;

use crucible_core::{ContextSection, Fragment, Revealed, Seen, ToolOutput, WorkspaceSection};

use super::*;

fn contexts(transcript: &Transcript) -> Vec<&Fragment> {
    transcript
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::Context(fragment) => Some(fragment),
            Message::User { .. } | Message::Agent { .. } | Message::ToolResults(_) => None,
        })
        .collect()
}

#[test]
fn static_context_is_assembled_once_in_stable_order_and_charged_before_fullness() {
    let mut scripted = Scripted::new(Script::new(Vec::new()), Tools::new(), Verdict::Allow);
    let before = scripted.runner.load;
    let ancestry = Ancestry::new();

    scripted
        .runner
        .assemble_context(ancestry)
        .expect("the first context");

    let first = contexts(scripted.runner.transcript());
    assert_eq!(
        first
            .iter()
            .map(|fragment| fragment.section())
            .collect::<Vec<_>>(),
        [
            "workspace",
            "skills",
            "environment",
            "model",
            "tools",
            "permissions",
        ]
    );
    let old_full_preamble = first
        .iter()
        .map(|fragment| fragment.text().len())
        .sum::<usize>();
    assert!(old_full_preamble > 0);
    assert!(scripted.runner.load.tokens() > before.tokens());

    // Put the exact charged load on the compaction boundary. If fragments
    // were recorded after reserve/fullness was read, this comparison would
    // still see the empty `before` value and let the request through.
    scripted.runner.policy.compaction.reserve = Some(100);
    let window = u32::try_from(scripted.runner.load.tokens() + 100).unwrap();
    scripted.runner.spec.model.window = Some(window);
    let reserve = scripted
        .runner
        .reserve(scripted.runner.policy.compaction, Some(window));
    assert_eq!(reserve, 100);
    assert!(!before.full(Some(window), reserve));
    assert!(scripted.runner.load.full(Some(window), reserve));

    let snapshot = scripted.runner.session.context_snapshot().unwrap().clone();
    let messages = scripted.runner.transcript().len();
    let charged = scripted.runner.load.tokens();
    scripted
        .runner
        .assemble_context(ancestry)
        .expect("unchanged context");

    assert_eq!(scripted.runner.transcript().len(), messages);
    assert_eq!(scripted.runner.load.tokens(), charged);
    assert_eq!(scripted.runner.session.context_snapshot(), Some(&snapshot));
    let later_assembled = contexts(scripted.runner.transcript())
        .into_iter()
        .skip(6)
        .map(|fragment| fragment.text().len())
        .sum::<usize>();
    assert!(
        later_assembled < old_full_preamble,
        "a later static pass must assemble fewer context bytes than the old full preamble"
    );
    assert_eq!(
        snapshot
            .sections()
            .map(|(section, _)| section)
            .collect::<Vec<_>>(),
        [
            "environment",
            "model",
            "permissions",
            "skills",
            "tools",
            "workspace",
        ],
        "the model-visible fact set changed while its rendering was deferred"
    );
}

#[test]
fn a_compaction_that_removes_context_forces_a_full_render_on_the_next_pass() {
    let script = Script::new(vec![
        saying("first"),
        saying("second"),
        recap("kept context"),
        saying("third"),
    ]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.runner.policy.compaction = Compaction {
        keep_tokens: 1,
        ..Compaction::default()
    };
    scripted.turn("first").expect("a first turn");
    scripted.turn("second").expect("a second turn");
    assert_eq!(contexts(scripted.runner.transcript()).len(), 6);

    let room = scripted.compacting().expect("a structured compaction");
    let Room::Made(compacted) = room else {
        panic!("the two-turn transcript was not compacted: {room:?}");
    };
    assert_eq!(
        compacted.replaced, 2,
        "typed harness fragments were reported as conversation messages"
    );
    assert!(contexts(scripted.runner.transcript()).is_empty());

    let workspace = std::env::temp_dir();
    let section = WorkspaceSection::new(workspace.as_path());
    let recorded = scripted
        .runner
        .session
        .context_snapshot()
        .expect("the typed state survived compaction");
    assert!(matches!(
        recorded.seen(&section, scripted.runner.transcript()),
        Seen::Stale
    ));

    // Negative control for the load-bearing distinction. A two-state resolver
    // sees only the recorded snapshot, calls it Known, and suppresses the
    // render. The integrated assertion below therefore fails if reconciliation
    // is simplified to that deliberately wrong resolver.
    let two_state = recorded
        .get(WorkspaceSection::ID)
        .map_or(Seen::Fresh, Seen::Known);
    assert!(section.render(two_state).is_none());

    scripted.turn("third").expect("the turn after compaction");
    let rendered = contexts(scripted.runner.transcript());
    assert_eq!(rendered.len(), 6);
    let workspace = rendered
        .iter()
        .find(|fragment| fragment.section() == WorkspaceSection::ID)
        .expect("workspace was re-rendered");
    assert!(workspace.text().contains("The workspace root is"));
    assert!(!workspace.text().contains("changed"));
    assert!(!workspace.text().contains("supersedes"));
}

#[test]
fn a_pre_context_session_supersedes_every_unknown_section_on_its_first_pass() {
    let sample = Sample::new("runner-legacy-context");
    let workspace = sample.workspace();
    let session = Session::start(&sample.logs(), &workspace, None).unwrap();
    let path = session.path().to_owned();
    drop(session);

    let current = fs::read_to_string(&path).unwrap();
    let legacy = current.replacen(r#""format":11"#, r#""format":9"#, 1);
    assert_ne!(legacy, current, "the fixture header was not downgraded");
    fs::write(&path, legacy).unwrap();

    let (session, transcript) = Session::resume(&sample.logs(), &workspace).unwrap();
    assert_eq!(session.context_snapshot(), None);
    let script = Script::new(vec![saying("continued")]);
    let mut scripted = Scripted::recording(script, Tools::new(), Verdict::Allow, session);
    scripted.runner = scripted.runner.resuming(transcript);

    scripted.turn("continue").expect("the first upgraded turn");

    let sent = scripted.sent.lock().unwrap();
    let first = &sent.first().expect("the first upgraded request").context;
    assert_eq!(first.len(), 6);
    assert!(
        first
            .iter()
            .all(|fragment| fragment.text().contains("supersedes")),
        "an unknown section was not stated defensively: {first:?}"
    );
}

#[derive(Clone)]
struct ToggleReveal {
    name: &'static str,
    revealed: Revealed,
    present: bool,
}

impl DescribeTool for ToggleReveal {
    fn name(&self) -> &str {
        self.name
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for ToggleReveal {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new(self.name)
    }

    fn run(
        &self,
        _approved: Approved,
        _context: &ToolContext<'_>,
    ) -> Result<ToolOutput, ToolError> {
        if self.present {
            self.revealed.reveal("web_search");
        } else {
            self.revealed.forget();
        }
        Ok(ToolOutput::ok("done"))
    }
}

#[test]
fn tool_reveal_and_disposal_are_reported_from_the_exact_generation_used_next() {
    let revealed = Revealed::new();
    let mut offered = Tools::looking_up(revealed.clone());
    offered
        .add_builtin(ToggleReveal {
            name: "reveal",
            revealed: revealed.clone(),
            present: true,
        })
        .unwrap();
    offered
        .add_builtin(ToggleReveal {
            name: "dispose",
            revealed: revealed.clone(),
            present: false,
        })
        .unwrap();
    offered.defer_builtin(Fixed::new("web_search")).unwrap();
    let script = Script::new(vec![
        calling("one", "reveal", "{}"),
        calling("two", "dispose", "{}"),
        saying("done"),
    ]);
    let mut scripted = Scripted::new(script, offered, Verdict::Allow);

    scripted.turn("go").expect("a reveal and disposal turn");

    let sent = scripted.sent.lock().unwrap();
    assert_eq!(sent.len(), 3);
    let first_request = sent.first().expect("the initial request");
    let revealed_request = sent.get(1).expect("the request after reveal");
    let disposed_request = sent.get(2).expect("the request after disposal");
    assert_eq!(
        first_request
            .tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        ["reveal", "dispose"]
    );
    assert_eq!(
        revealed_request
            .tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        ["reveal", "dispose", "web_search"]
    );
    assert_eq!(
        disposed_request
            .tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        ["reveal", "dispose"]
    );

    let latest_tools = |at: usize| {
        sent.get(at)
            .unwrap_or_else(|| panic!("request {at}"))
            .context
            .iter()
            .rev()
            .find(|fragment| fragment.section() == "tools")
            .expect("a tools fragment")
            .text()
            .to_owned()
    };
    let initial = latest_tools(0);
    let added = latest_tools(1);
    let removed = latest_tools(2);
    assert!(!initial.contains("web_search"), "{initial}");
    assert!(
        added.contains("Tools now advertised: web_search."),
        "{added}"
    );
    assert!(
        removed.contains("Tools no longer advertised: web_search."),
        "{removed}"
    );
    assert_ne!(
        initial, added,
        "the stale initial generation was re-advertised"
    );
    assert_ne!(added, removed, "the revealed generation survived disposal");
}

#[test]
fn a_permission_remembered_mid_turn_is_reported_on_the_immediately_following_pass() {
    let script = Script::new(vec![calling("one", "edit", "{}"), saying("done")]);
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("edit").risking(changing())]),
        Verdict::Allow,
    );
    scripted.says = Says::for_the_session();

    scripted.turn("go").expect("an approved tool turn");

    let sent = scripted.sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    let first_request = sent.first().expect("the first permission request");
    let next_request = sent.get(1).expect("the request after permission");
    assert_eq!(
        first_request
            .context
            .iter()
            .filter(|fragment| fragment.section() == "permissions")
            .count(),
        1
    );
    let changed = next_request
        .context
        .iter()
        .rev()
        .find(|fragment| fragment.section() == "permissions")
        .expect("the next pass permission delta");
    assert!(
        changed.text().contains("New session-scoped approvals"),
        "{changed:?}"
    );
    assert_eq!(scripted.says.asked, 1);
}
