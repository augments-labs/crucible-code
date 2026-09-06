//! Native Windows sandbox setup, verification, and launch boundary.
//!
//! Setup is deliberately explicit and elevated. Normal sandbox launches read
//! broker-derived HKLM state for the calling user; no request can choose a
//! credential or setup-state location.

#![allow(
    unsafe_code,
    reason = "Windows exposes account, registry, DPAPI and WFP policy only through its native system API"
)]

mod account;
mod acl;
mod desktop;
mod identity;
mod lock;
mod plan;
mod process;
mod record;
mod registry;
mod runtime;
mod secret;
mod token;
mod wfp;
mod winutil;

use std::ffi::OsString;
use std::io;

use identity::SetupIdentity;
use record::{SetupRecord, Status};
use secret::SecretWide;

struct LaunchSetup {
    identity: SetupIdentity,
    record: SetupRecord,
    password: SecretWide,
}

pub(super) enum Action {
    Setup(Option<OsString>),
    Uninstall(Option<OsString>),
    Probe,
}

pub(super) fn parse(mut arguments: impl Iterator<Item = OsString>) -> io::Result<Action> {
    let Some(mode) = arguments.next() else {
        return Err(invalid_mode());
    };
    let setup = mode == super::WINDOWS_SETUP_MODE;
    let uninstall = mode == super::WINDOWS_UNINSTALL_MODE;
    if setup || uninstall {
        let owner = match arguments.next() {
            None => None,
            Some(flag) if flag == "--owner" => Some(arguments.next().ok_or_else(invalid_mode)?),
            Some(_) => return Err(invalid_mode()),
        };
        if arguments.next().is_some() {
            return Err(invalid_mode());
        }
        return Ok(if setup {
            Action::Setup(owner)
        } else {
            Action::Uninstall(owner)
        });
    }
    if mode == super::WINDOWS_PROBE_MODE && arguments.next().is_none() {
        return Ok(Action::Probe);
    }
    Err(invalid_mode())
}

pub(super) fn run(action: Action) -> io::Result<String> {
    match action {
        Action::Setup(owner) => setup(owner.as_deref()),
        Action::Uninstall(owner) => uninstall(owner.as_deref()),
        Action::Probe => probe(),
    }
}

pub(super) fn launch_host(request: &crate::WindowsLaunchRequest) -> io::Result<u32> {
    runtime::host(request)
}

pub(super) fn launch_child(request: &crate::WindowsLaunchRequest) -> io::Result<u32> {
    runtime::child(request)
}

pub(super) fn setup(owner: Option<&std::ffi::OsStr>) -> io::Result<String> {
    winutil::require_elevated()?;
    let owner_sid = owner_sid(owner)?;
    let identity = SetupIdentity::for_owner(&owner_sid);
    let _maintenance_lock = lock::SetupLock::acquire(&identity)?;
    let owner_sid_string = winutil::sid_string(&owner_sid)?;

    let existing = registry::load(&identity)?;
    if existing.is_none() && account::exists(&identity.account_name)? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the deterministic Windows sandbox account exists without trusted setup state",
        ));
    }
    let (record, password) = if let Some(record) = existing {
        validate_record_owner(&record, &identity, &owner_sid)?;
        let password = SecretWide::unprotect(&record.protected_password, &identity.entropy)?;
        (record, password)
    } else {
        let password = SecretWide::generate()?;
        let protected = password.protect(&identity.entropy)?;
        let record =
            SetupRecord::pending(owner_sid.clone(), identity.account_name.clone(), protected)?;
        registry::store(&identity, &owner_sid_string, &record)?;
        (record, password)
    };

    let created = account::ensure(&identity.account_name, &password)?;
    let result = (|| {
        let account_sid = account::sid(&identity.account_name)?;
        wfp::install(&identity, &identity.account_name)?;
        let installed = SetupRecord::pending(
            record.owner_sid,
            record.account_name,
            record.protected_password,
        )?
        .installed(account_sid.clone())?;
        registry::store(&identity, &owner_sid_string, &installed)?;
        account::probe(&identity.account_name, &account_sid, &password)?;
        wfp::probe(&identity, &identity.account_name)?;
        Ok(())
    })();
    if let Err(source) = result {
        if created && let Err(cleanup) = account::delete(&identity.account_name) {
            return Err(io::Error::other(format!(
                "{source}; Windows sandbox account rollback also failed: {cleanup}"
            )));
        }
        return Err(source);
    }
    Ok(format!(
        "Windows sandbox setup is ready for {owner_sid_string} ({})",
        identity.account_name
    ))
}

pub(super) fn probe() -> io::Result<String> {
    let setup = current_launch_setup()?;
    Ok(format!(
        "Windows sandbox setup is ready ({})",
        setup.identity.account_name
    ))
}

fn current_launch_setup() -> io::Result<LaunchSetup> {
    let owner_sid = winutil::current_user_sid()?;
    let identity = SetupIdentity::for_owner(&owner_sid);
    let record = registry::load(&identity)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Windows sandbox setup is missing; run Crucible's setup command in an Administrator PowerShell",
        )
    })?;
    validate_record_owner(&record, &identity, &owner_sid)?;
    if record.status != Status::Installed {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Windows sandbox setup is incomplete; rerun Crucible's setup command",
        ));
    }
    let password = SecretWide::unprotect(&record.protected_password, &identity.entropy)?;
    account::probe(&record.account_name, &record.account_sid, &password)?;
    wfp::probe(&identity, &record.account_name)?;
    Ok(LaunchSetup {
        identity,
        record,
        password,
    })
}

pub(super) fn uninstall(owner: Option<&std::ffi::OsStr>) -> io::Result<String> {
    winutil::require_elevated()?;
    let owner_sid = owner_sid(owner)?;
    let identity = SetupIdentity::for_owner(&owner_sid);
    let _maintenance_lock = lock::SetupLock::acquire(&identity)?;
    let Some(record) = registry::load(&identity)? else {
        return Ok("Windows sandbox setup is already absent".into());
    };
    validate_record_owner(&record, &identity, &owner_sid)?;
    // Disable first. A failed WFP cleanup therefore leaves an unusable account
    // and its recoverable record instead of a network-capable sandbox identity.
    account::disable(&record.account_name)?;
    wfp::remove(&identity)?;
    account::delete(&record.account_name)?;
    registry::delete(&identity)?;
    Ok(format!(
        "Windows sandbox setup was removed ({})",
        identity.account_name
    ))
}

fn owner_sid(owner: Option<&std::ffi::OsStr>) -> io::Result<Vec<u8>> {
    owner.map_or_else(winutil::current_user_sid, winutil::account_sid)
}

fn validate_record_owner(
    record: &SetupRecord,
    identity: &SetupIdentity,
    owner_sid: &[u8],
) -> io::Result<()> {
    if record.owner_sid != owner_sid || record.account_name != identity.account_name {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Windows sandbox setup state belongs to a different identity",
        ));
    }
    Ok(())
}

fn invalid_mode() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "invalid Windows sandbox broker mode",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_modes_reject_ambiguous_argument_shapes() {
        let setup = [OsString::from(super::super::WINDOWS_SETUP_MODE)];
        assert!(matches!(
            parse(setup.into_iter()).expect("setup"),
            Action::Setup(None)
        ));

        let targeted = [
            OsString::from(super::super::WINDOWS_UNINSTALL_MODE),
            OsString::from("--owner"),
            OsString::from("person"),
        ];
        assert!(matches!(
            parse(targeted.into_iter()).expect("targeted uninstall"),
            Action::Uninstall(Some(owner)) if owner == "person"
        ));

        for invalid in [
            vec![OsString::from("--unknown")],
            vec![
                OsString::from(super::super::WINDOWS_SETUP_MODE),
                OsString::from("--owner"),
            ],
            vec![
                OsString::from(super::super::WINDOWS_PROBE_MODE),
                OsString::from("extra"),
            ],
        ] {
            assert!(parse(invalid.into_iter()).is_err());
        }
    }
}
