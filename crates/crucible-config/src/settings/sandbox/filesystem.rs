//! Compile path grants and restrictions without laundering project authority.
//!
//! Explicit user grants are assembled before denies. A project read-only
//! restriction covers existing writable descendants as well as its named root.
//! Protected control files survive every additional grant; an unreadable
//! ancestor can hide them, but can never make them writable.

use super::{
    Access, ConfigError, PathBuf, PathSetting, Provenance, SandboxFilesystemRule, Workspace,
};

pub(super) fn resolve(
    base: &[SandboxFilesystemRule],
    settings: &[PathSetting],
    workspace: &Workspace,
) -> Result<Vec<SandboxFilesystemRule>, ConfigError> {
    let mut rules = base.to_vec();
    for setting in settings {
        let path: PathBuf = workspace.root().join(&setting.path).components().collect();
        let rule = SandboxFilesystemRule::new(&path, setting.access, setting.source.provenance())
            .map_err(|_| {
            setting
                .source
                .error("path is not a bounded absolute native path")
        })?;
        let inherited = rules
            .iter()
            .filter(|rule| path.starts_with(rule.path()))
            .max_by_key(|rule| rule.path().components().count());
        if setting.source.origin.in_the_workspace()
            && matches!(setting.access, Access::ReadOnly | Access::Protected)
            && !inherited.is_some_and(|rule| rule.access() != Access::Unreadable)
        {
            return Err(setting.source.error("a project restriction cannot grant readable access outside the inherited filesystem view"));
        }
        if rules.iter().any(|ancestor| {
            path.starts_with(ancestor.path())
                && match ancestor.access() {
                    Access::Unreadable => setting.access != Access::Unreadable,
                    Access::Protected => setting.access == Access::ReadWrite,
                    Access::ReadOnly | Access::ReadWrite => false,
                }
        }) {
            return Err(setting
                .source
                .error("a path grant cannot reopen an unreadable or protected ancestor"));
        }
        if matches!(setting.access, Access::Unreadable | Access::Protected)
            || (setting.source.origin.in_the_workspace() && setting.access == Access::ReadOnly)
        {
            for nested in &mut rules {
                if nested.path().starts_with(&path) && nested.path() != path {
                    let access = match (setting.access, nested.access()) {
                        (Access::Unreadable, _) => Access::Unreadable,
                        (_, Access::ReadWrite) => setting.access,
                        (_, access) => access,
                    };
                    if access != nested.access() {
                        *nested = SandboxFilesystemRule::new(
                            nested.path(),
                            access,
                            setting.source.provenance(),
                        )
                        .map_err(|_| setting.source.error("invalid narrowed filesystem path"))?;
                    }
                }
            }
        }
        // An exact protected carve-out cannot be downgraded to an ordinary
        // read-only rule that would subsequently admit writable descendants.
        if rules
            .iter()
            .any(|old| old.path() == path && old.access() == Access::Protected)
            && setting.access == Access::ReadOnly
        {
            continue;
        }
        rules.retain(|old| old.path() != path);
        rules.push(rule);
        if setting.access == Access::ReadWrite {
            for name in [".git", ".agents", ".codex", ".crucible"] {
                let metadata = path.join(name);
                if std::fs::symlink_metadata(&metadata).is_ok()
                    && !rules.iter().any(|old| old.path() == metadata)
                {
                    rules.push(
                        SandboxFilesystemRule::new(
                            metadata,
                            Access::Protected,
                            Provenance::ProtectedMetadata,
                        )
                        .map_err(|_| setting.source.error("invalid protected metadata path"))?,
                    );
                }
            }
        }
        if rules.len() > crucible_core::MAX_SANDBOX_FILESYSTEM_RULES {
            return Err(setting
                .source
                .error("effective filesystem rules exceed their bound"));
        }
    }
    Ok(rules)
}
