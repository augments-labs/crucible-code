//! `/sandbox`: inspect policy, check platform prerequisites, and choose
//! confinement for future command and MCP process preparations.
//!
//! The panel is only available between turns. A prepared process owns a policy
//! snapshot, so an already-running background command keeps its original
//! boundary. Project requirements survive every interactive choice.

use crucible_core::{
    Ancestry, SandboxEnablement, SandboxId, SandboxManifest, SandboxRequest, SandboxService, ToolId,
};
use crucible_tools::LocalSandbox;
use crucible_tui::{Key, Offered, Pressed, Renderer, SandboxPanel, SandboxTab, Terminal};

use crate::cli::converse::region::{self, Ended, Moved};
use crate::cli::{Fatal, remember};

use super::{Terms, say};

pub(super) fn run<T: Terminal>(
    rest: &str,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    match rest.trim() {
        "enable" => return taken(true, renderer, terms),
        "disable" => return taken(false, renderer, terms),
        "" => {}
        _ => {
            return say(
                renderer,
                "use /sandbox, /sandbox enable, or /sandbox disable",
            );
        }
    }
    let mut standing = Standing::new(terms);
    if !keys {
        return listed(renderer, &standing);
    }
    let ended = region::stand(
        renderer,
        |_| terms.style(),
        &mut standing,
        |standing, columns, room| {
            let shown: Vec<_> = standing
                .items()
                .iter()
                .map(|(name, says)| Offered { name, says })
                .collect();
            let rows = SandboxPanel {
                tab: standing.tab,
                items: &shown,
                chosen: standing.at(),
                summary: Some(&standing.summary),
            }
            .within(columns, room, terms.style().glyphs());
            (rows, None)
        },
        walking,
    )?;
    match ended {
        Ended::Took if standing.tab == SandboxTab::Sandbox && standing.at() < 2 => {
            taken(standing.at() == 0, renderer, terms)
        }
        Ended::Took => {
            if let Some((name, says)) = standing.items().get(standing.at()) {
                say(renderer, &format!("{name}: {says}"))?;
            }
            Ok(())
        }
        Ended::Left => say(renderer, "sandbox settings unchanged"),
        Ended::Cramped => listed(renderer, &standing),
    }
}

fn taken<T: Terminal>(
    enabled: bool,
    renderer: &mut Renderer<T>,
    terms: &Terms,
) -> Result<(), Fatal> {
    let control = terms.settings.sandbox().enablement();
    if let Err(problem) = choose(
        &control,
        enabled,
        || verify(terms),
        || remember::sandboxing(&terms.choosing, enabled).map_err(|problem| problem.to_string()),
    ) {
        return say(renderer, &format!("sandbox unchanged: {problem}"));
    }
    let state = if enabled { "enabled" } else { "disabled" };
    say(
        renderer,
        &format!(
            "sandbox {state} for new commands and MCP processes; active commands keep their original policy"
        ),
    )?;
    Ok(())
}

fn choose(
    control: &SandboxEnablement,
    enabled: bool,
    verify: impl FnOnce() -> Result<(), String>,
    save: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if !enabled && control.required() {
        return Err("project configuration requires confinement".into());
    }
    if enabled {
        verify()?;
    }
    save()?;
    control
        .set_enabled(enabled)
        .map_err(|problem| problem.to_string())
}

fn verify(terms: &Terms) -> Result<(), String> {
    let policy = terms
        .settings
        .sandbox()
        .enforcing_policy(&terms.workspace)
        .map_err(|problem| problem.to_string())?;
    let service = LocalSandbox::new();
    // Preparation checks exact backend capability and filesystem policy. It
    // does not materialize or start a user command; dropping releases admission.
    let prepared = service
        .prepare(SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("sandbox-inspection"),
            policy,
            SandboxManifest::empty(),
        ))
        .map_err(|problem| problem.to_string())?;
    drop(prepared);
    Ok(())
}

struct Standing {
    tab: SandboxTab,
    selected: [usize; 2],
    summary: String,
    settings: Vec<(String, String)>,
    dependencies: Vec<(String, String)>,
}

impl Standing {
    fn new(terms: &Terms) -> Self {
        let settings = terms.settings.sandbox();
        let state = if settings.enabled() {
            "enabled"
        } else {
            "disabled"
        };
        let source = if settings.required_by_project() {
            "required by project configuration"
        } else {
            "user choice"
        };
        let mut choices = vec![
            (
                "Enable sandbox".into(),
                "Require the native OS boundary before new commands start".into(),
            ),
            (
                "Disable sandbox".into(),
                if settings.required_by_project() {
                    "Unavailable: project configuration requires confinement"
                } else {
                    "Keep approvals and command limits; run without OS confinement"
                }
                .into(),
            ),
        ];
        match settings.policy(&terms.workspace) {
            Ok(policy) => {
                choices.push((
                    "Filesystem".into(),
                    format!(
                        "Effective path rules: {}; edit sandbox.filesystem in configuration",
                        policy.filesystem().len()
                    ),
                ));
                let limits = policy.limits();
                choices.push((
                    "Command limits".into(),
                    format!(
                        "{} seconds, {} output bytes, {} concurrent commands",
                        limits.command_time.unwrap_or_default().as_secs(),
                        limits.output_bytes.unwrap_or_default(),
                        limits.concurrent_commands.unwrap_or_default()
                    ),
                ));
                choices.push((
                    "Network".into(),
                    match policy.network() {
                        crucible_core::SandboxNetworkPolicy::Closed => "Configured closed while sandbox is enabled".into(),
                        crucible_core::SandboxNetworkPolicy::Domains(network) => format!(
                            "{} allowed domains, {} denied domains, local binding {}, {} Unix sockets",
                            network.allowed().len(), network.denied().len(),
                            if network.allow_local_binding() { "allowed" } else { "denied" }, network.unix_sockets().len(),
                        ),
                    },
                ));
            }
            Err(problem) => choices.push(("Policy unavailable".into(), problem.to_string())),
        }
        Self {
            tab: SandboxTab::Sandbox,
            selected: [usize::from(!settings.enabled()), 0],
            summary: format!("Sandbox {state} · {source}"),
            settings: choices,
            dependencies: dependencies(),
        }
    }

    const fn index(&self) -> usize {
        match self.tab {
            SandboxTab::Sandbox => 0,
            SandboxTab::Dependencies => 1,
        }
    }
    fn at(&self) -> usize {
        self.selected.get(self.index()).copied().unwrap_or_default()
    }
    fn items(&self) -> &[(String, String)] {
        match self.tab {
            SandboxTab::Sandbox => &self.settings,
            SandboxTab::Dependencies => &self.dependencies,
        }
    }
}

fn dependencies() -> Vec<(String, String)> {
    let status = match LocalSandbox::new().probe() {
        Ok((identity, _)) => format!(
            "available: {} {}",
            identity.id().as_str(),
            identity.version()
        ),
        Err(problem) => format!("unavailable: {problem}"),
    };
    let (backend, setup) = platform();
    vec![(backend.into(), status), ("Setup".into(), setup.into())]
}

const fn platform() -> (&'static str, &'static str) {
    if cfg!(target_os = "linux") {
        (
            "Linux / WSL2",
            "Bubblewrap 0.11.0 or newer and the packaged crucible-sandbox-broker are required",
        )
    } else if cfg!(target_os = "macos") {
        (
            "macOS Seatbelt",
            "Uses the built-in /usr/bin/sandbox-exec; no extra sandbox package is needed",
        )
    } else if cfg!(target_os = "windows") {
        (
            "Native Windows",
            "Run crucible-sandbox-broker --windows-sandbox-setup once in an Administrator terminal",
        )
    } else {
        (
            "Unsupported platform",
            "Enabled confinement is unavailable on this operating system",
        )
    }
}

fn listed<T: Terminal>(renderer: &mut Renderer<T>, standing: &Standing) -> Result<(), Fatal> {
    say(renderer, &standing.summary)?;
    for (name, description) in standing.settings.iter().chain(&standing.dependencies) {
        say(renderer, &format!("{name}: {description}"))?;
    }
    say(
        renderer,
        "use /sandbox enable or /sandbox disable to choose",
    )
}

#[allow(clippy::needless_pass_by_value)]
fn walking(key: Pressed, standing: &mut Standing) -> Moved {
    match key {
        Pressed::Key(Key::Left | Key::Right) => {
            standing.tab = match standing.tab {
                SandboxTab::Sandbox => SandboxTab::Dependencies,
                SandboxTab::Dependencies => SandboxTab::Sandbox,
            };
            Moved::Redraw
        }
        Pressed::Up | Pressed::Down => {
            let next = if matches!(key, Pressed::Up) {
                standing.at().saturating_sub(1)
            } else {
                standing
                    .at()
                    .saturating_add(1)
                    .min(standing.items().len().saturating_sub(1))
            };
            let axis = standing.index();
            if let Some(selected) = standing.selected.get_mut(axis) {
                *selected = next;
            }
            Moved::Redraw
        }
        Pressed::Key(Key::Enter) => Moved::Took,
        Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
        Pressed::Resized => Moved::Redraw,
        _ => Moved::Still,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unavailable_backend_cannot_change_the_choice() {
        let control = SandboxEnablement::new(false, false);
        assert!(
            choose(
                &control,
                true,
                || Err("native boundary unavailable".into()),
                || panic!("an unavailable boundary cannot be saved")
            )
            .is_err()
        );
        assert!(!control.enabled());
        choose(&control, true, || Ok(()), || Ok(())).unwrap();
        assert!(control.enabled());
    }

    #[test]
    fn an_unsaved_choice_preserves_the_effective_policy() {
        let sample = crate::cli::sample::Sample::new("sandbox-unwritable-choice");
        let mut terms = crate::cli::converse::tests::plain();
        terms.settings = sample.user(r#"{"sandbox":{"enabled":true}}"#);
        terms.workspace = sample.workspace();
        terms.choosing = sample.user_file();
        std::fs::remove_file(&terms.choosing).expect("remove the fixture configuration");
        std::fs::create_dir(&terms.choosing).expect("a directory cannot be replaced as a file");
        let mut renderer = Renderer::new(crucible_tui::Recording::new(80, 24));

        taken(false, &mut renderer, &terms).expect("report the persistence failure");

        assert!(terms.settings.sandbox().enabled());
        assert!(renderer.terminal().written().contains("sandbox unchanged"));
        assert!(terms.choosing.is_dir());
    }

    #[test]
    fn a_project_requirement_survives_interactive_disabling() {
        let control = SandboxEnablement::new(true, true);
        assert!(
            choose(
                &control,
                false,
                || panic!("disabling must not probe"),
                || panic!("a required boundary cannot be disabled")
            )
            .is_err()
        );
        assert!(control.enabled());
        let optional = SandboxEnablement::new(true, false);
        choose(
            &optional,
            false,
            || panic!("disabling needs no enforcing backend"),
            || Ok(()),
        )
        .unwrap();
        assert!(!optional.enabled());
    }
}

#[cfg(test)]
mod navigation_tests {
    use super::*;

    #[test]
    fn tabs_keep_their_selection_and_escape_does_not_take_a_choice() {
        let mut panel = Standing {
            tab: SandboxTab::Sandbox,
            selected: [0, 0],
            summary: "fixture".into(),
            settings: vec![
                ("enable".into(), String::new()),
                ("disable".into(), String::new()),
            ],
            dependencies: vec![("native".into(), String::new())],
        };
        walking(Pressed::Down, &mut panel);
        assert_eq!(panel.at(), 1);
        walking(Pressed::Key(Key::Right), &mut panel);
        assert_eq!(panel.tab, SandboxTab::Dependencies);
        walking(Pressed::Down, &mut panel);
        assert_eq!(panel.at(), 0);
        walking(Pressed::Key(Key::Left), &mut panel);
        assert_eq!(panel.at(), 1);
        assert_eq!(walking(Pressed::Resized, &mut panel), Moved::Redraw);
        assert_eq!(walking(Pressed::Escape, &mut panel), Moved::Left);
    }
}
