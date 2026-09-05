//! Persistent outbound-network denial scoped to one sandbox account.

use std::ffi::c_void;
use std::io;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    FWP_E_ALREADY_EXISTS, FWP_E_FILTER_NOT_FOUND, FWP_E_NOT_FOUND, HANDLE, HLOCAL, LocalFree,
};
use windows_sys::Win32::NetworkManagement::WindowsFilteringPlatform::{
    FWP_ACTION_BLOCK, FWP_ACTRL_MATCH_FILTER, FWP_BYTE_BLOB, FWP_CONDITION_VALUE0,
    FWP_CONDITION_VALUE0_0, FWP_EMPTY, FWP_MATCH_EQUAL, FWP_SECURITY_DESCRIPTOR_TYPE, FWP_VALUE0,
    FWPM_ACTION0, FWPM_ACTION0_0, FWPM_CONDITION_ALE_USER_ID, FWPM_DISPLAY_DATA0,
    FWPM_FILTER_CONDITION0, FWPM_FILTER_FLAG_PERSISTENT, FWPM_FILTER0, FWPM_FILTER0_0,
    FWPM_LAYER_ALE_AUTH_CONNECT_V4, FWPM_LAYER_ALE_AUTH_CONNECT_V6,
    FWPM_LAYER_ALE_RESOURCE_ASSIGNMENT_V4, FWPM_LAYER_ALE_RESOURCE_ASSIGNMENT_V6,
    FWPM_PROVIDER_FLAG_PERSISTENT, FWPM_PROVIDER0, FWPM_SESSION0, FWPM_SUBLAYER_FLAG_PERSISTENT,
    FWPM_SUBLAYER0, FwpmEngineClose0, FwpmEngineOpen0, FwpmFilterAdd0, FwpmFilterDeleteByKey0,
    FwpmFilterGetByKey0, FwpmFreeMemory0, FwpmProviderAdd0, FwpmProviderGetByKey0,
    FwpmSubLayerAdd0, FwpmSubLayerGetByKey0, FwpmTransactionAbort0, FwpmTransactionBegin0,
    FwpmTransactionCommit0,
};
use windows_sys::Win32::Security::Authorization::{
    BuildExplicitAccessWithNameW, BuildSecurityDescriptorW, EXPLICIT_ACCESS_W, GRANT_ACCESS,
};
use windows_sys::Win32::Security::PSECURITY_DESCRIPTOR;
use windows_sys::Win32::System::Rpc::RPC_C_AUTHN_DEFAULT;
use windows_sys::core::GUID;

use super::identity::SetupIdentity;
use super::winutil::wide;

const PROVIDER_KEY: GUID = GUID::from_u128(0x73945374_6372_4a97_9ffc_8eef76204297);
const SUBLAYER_KEY: GUID = GUID::from_u128(0x55ea8e91_c0ab_49a4_a8ec_8c84fa3c17d8);
const TRANSACTION_WAIT_MILLIS: u32 = 10_000;
const MAX_SECURITY_DESCRIPTOR_BYTES: u32 = 64 * 1024;

struct FilterSpec {
    name: &'static str,
    description: &'static str,
    layer: GUID,
}

const FILTERS: [FilterSpec; 4] = [
    FilterSpec {
        name: "crucible_sandbox_deny_connect_v4",
        description: "Deny sandbox-account outbound IPv4 connections",
        layer: FWPM_LAYER_ALE_AUTH_CONNECT_V4,
    },
    FilterSpec {
        name: "crucible_sandbox_deny_connect_v6",
        description: "Deny sandbox-account outbound IPv6 connections",
        layer: FWPM_LAYER_ALE_AUTH_CONNECT_V6,
    },
    FilterSpec {
        name: "crucible_sandbox_deny_assignment_v4",
        description: "Deny sandbox-account IPv4 endpoint assignment",
        layer: FWPM_LAYER_ALE_RESOURCE_ASSIGNMENT_V4,
    },
    FilterSpec {
        name: "crucible_sandbox_deny_assignment_v6",
        description: "Deny sandbox-account IPv6 endpoint assignment",
        layer: FWPM_LAYER_ALE_RESOURCE_ASSIGNMENT_V6,
    },
];

pub(super) fn install(identity: &SetupIdentity, account: &str) -> io::Result<()> {
    let engine = Engine::open(TRANSACTION_WAIT_MILLIS)?;
    let mut transaction = engine.transaction()?;
    ensure_provider(engine.handle)?;
    ensure_sublayer(engine.handle)?;
    let user = UserCondition::new(account)?;
    for (key, spec) in identity.filter_keys.iter().zip(&FILTERS) {
        delete_filter(engine.handle, key)?;
        add_filter(engine.handle, key, spec, &user)?;
    }
    transaction.commit()
}

pub(super) fn remove(identity: &SetupIdentity) -> io::Result<()> {
    let engine = Engine::open(1_000)?;
    let mut transaction = engine.transaction()?;
    for key in &identity.filter_keys {
        delete_filter(engine.handle, key)?;
    }
    transaction.commit()
}

pub(super) fn probe(identity: &SetupIdentity, account: &str) -> io::Result<()> {
    let engine = Engine::open(1_000)?;
    verify_provider(engine.handle)?;
    verify_sublayer(engine.handle)?;
    let user = UserCondition::new(account)?;
    for (key, spec) in identity.filter_keys.iter().zip(&FILTERS) {
        let mut filter = null_mut::<FWPM_FILTER0>();
        // SAFETY: the engine and key are live and `filter` is initialized
        // out-pointer storage owned by FwpmFreeMemory0 on success.
        let status = unsafe { FwpmFilterGetByKey0(engine.handle, key, &raw mut filter) };
        if status != 0 {
            return Err(wfp_error("FwpmFilterGetByKey0", status));
        }
        let valid = if filter.is_null() {
            false
        } else {
            // SAFETY: the successful get call returned a live FWPM_FILTER0.
            let filter = unsafe { &*filter };
            let condition = if filter.numFilterConditions == 1 && !filter.filterCondition.is_null()
            {
                // SAFETY: the returned filter reports one condition and a
                // non-null pointer owned by the same WFP allocation.
                Some(unsafe { &*filter.filterCondition })
            } else {
                None
            };
            !filter.providerKey.is_null()
                // SAFETY: the pointer was checked non-null and belongs to the
                // still-live returned WFP filter allocation.
                && guid_eq(unsafe { &*filter.providerKey }, &PROVIDER_KEY)
                && guid_eq(&filter.filterKey, key)
                && guid_eq(&filter.layerKey, &spec.layer)
                && guid_eq(&filter.subLayerKey, &SUBLAYER_KEY)
                && filter.flags == FWPM_FILTER_FLAG_PERSISTENT
                && filter.action.r#type == FWP_ACTION_BLOCK
                && condition.is_some_and(|condition| {
                    guid_eq(&condition.fieldKey, &FWPM_CONDITION_ALE_USER_ID)
                        && condition.matchType == FWP_MATCH_EQUAL
                        && condition.conditionValue.r#type == FWP_SECURITY_DESCRIPTOR_TYPE
                        && condition_matches_user(condition, &user)
                })
        };
        // SAFETY: WFP returned this allocation through `filter`; the pointer
        // variable is passed exactly as FwpmFreeMemory0 requires.
        unsafe {
            FwpmFreeMemory0((&raw mut filter).cast::<*mut c_void>());
        }
        if !valid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Windows sandbox WFP filter does not match the installed policy",
            ));
        }
    }
    Ok(())
}

struct Engine {
    handle: HANDLE,
}

impl Engine {
    fn open(wait_milliseconds: u32) -> io::Result<Self> {
        let name = wide("Crucible Windows Sandbox WFP");
        let session = FWPM_SESSION0 {
            displayData: FWPM_DISPLAY_DATA0 {
                name: name.as_ptr().cast_mut(),
                description: null_mut(),
            },
            txnWaitTimeoutInMSec: wait_milliseconds,
            ..FWPM_SESSION0::default()
        };
        let mut handle = null_mut();
        // SAFETY: `session` and the out-handle storage remain live for the
        // synchronous call. Null server/auth identity selects the local engine.
        let status = unsafe {
            FwpmEngineOpen0(
                null(),
                RPC_C_AUTHN_DEFAULT.cast_unsigned(),
                null(),
                &raw const session,
                &raw mut handle,
            )
        };
        wfp_success("FwpmEngineOpen0", status)?;
        Ok(Self { handle })
    }

    fn transaction(&self) -> io::Result<Transaction<'_>> {
        // SAFETY: `self.handle` is a live WFP engine handle.
        wfp_success("FwpmTransactionBegin0", unsafe {
            FwpmTransactionBegin0(self.handle, 0)
        })?;
        Ok(Transaction {
            engine: self,
            committed: false,
        })
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: this object uniquely owns the live WFP engine handle.
        unsafe {
            FwpmEngineClose0(self.handle);
        }
    }
}

struct Transaction<'a> {
    engine: &'a Engine,
    committed: bool,
}

impl Transaction<'_> {
    fn commit(&mut self) -> io::Result<()> {
        // SAFETY: this transaction is open on the live engine handle.
        wfp_success("FwpmTransactionCommit0", unsafe {
            FwpmTransactionCommit0(self.engine.handle)
        })?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.committed {
            // SAFETY: an uncommitted transaction remains open on this engine.
            unsafe {
                FwpmTransactionAbort0(self.engine.handle);
            }
        }
    }
}

struct UserCondition {
    descriptor: PSECURITY_DESCRIPTOR,
    blob: FWP_BYTE_BLOB,
}

impl UserCondition {
    fn new(account: &str) -> io::Result<Self> {
        let account = wide(account);
        let mut access = EXPLICIT_ACCESS_W::default();
        // SAFETY: the account name is NUL terminated and `access` is writable
        // initialized storage for one explicit access entry.
        unsafe {
            BuildExplicitAccessWithNameW(
                &raw mut access,
                account.as_ptr(),
                FWP_ACTRL_MATCH_FILTER,
                GRANT_ACCESS,
                0,
            );
        }
        let mut descriptor = null_mut();
        let mut length = 0_u32;
        // SAFETY: `access` remains live and both descriptor outputs are
        // initialized storage. The returned descriptor uses LocalAlloc.
        let status = unsafe {
            BuildSecurityDescriptorW(
                null(),
                null(),
                1,
                &raw const access,
                0,
                null(),
                null_mut(),
                &raw mut length,
                &raw mut descriptor,
            )
        };
        if status != 0 {
            return Err(super::winutil::code_error(
                "BuildSecurityDescriptorW",
                status,
            ));
        }
        let condition = Self {
            descriptor,
            blob: FWP_BYTE_BLOB {
                size: length,
                data: descriptor.cast(),
            },
        };
        if condition.descriptor.is_null()
            || condition.blob.size == 0
            || condition.blob.size > MAX_SECURITY_DESCRIPTOR_BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "BuildSecurityDescriptorW returned an invalid security descriptor",
            ));
        }
        Ok(condition)
    }
}

impl Drop for UserCondition {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            // SAFETY: BuildSecurityDescriptorW returned this LocalAlloc
            // descriptor and ownership remains with this object.
            unsafe {
                LocalFree(self.descriptor as HLOCAL);
            }
        }
    }
}

fn ensure_provider(engine: HANDLE) -> io::Result<()> {
    let name = wide("Crucible Windows Sandbox");
    let description = wide("Persistent network policy for Crucible sandbox accounts");
    let provider = FWPM_PROVIDER0 {
        providerKey: PROVIDER_KEY,
        displayData: FWPM_DISPLAY_DATA0 {
            name: name.as_ptr().cast_mut(),
            description: description.as_ptr().cast_mut(),
        },
        flags: FWPM_PROVIDER_FLAG_PERSISTENT,
        providerData: empty_blob(),
        serviceName: null_mut(),
    };
    // SAFETY: the provider and all referenced display strings remain live for
    // the synchronous add operation.
    wfp_success_or_exists("FwpmProviderAdd0", unsafe {
        FwpmProviderAdd0(engine, &raw const provider, null_mut())
    })?;
    verify_provider(engine)
}

fn ensure_sublayer(engine: HANDLE) -> io::Result<()> {
    let name = wide("Crucible Windows Sandbox");
    let description = wide("Persistent deny sublayer for Crucible sandbox accounts");
    let provider_key = PROVIDER_KEY;
    let sublayer = FWPM_SUBLAYER0 {
        subLayerKey: SUBLAYER_KEY,
        displayData: FWPM_DISPLAY_DATA0 {
            name: name.as_ptr().cast_mut(),
            description: description.as_ptr().cast_mut(),
        },
        flags: FWPM_SUBLAYER_FLAG_PERSISTENT,
        providerKey: (&raw const provider_key).cast_mut(),
        providerData: empty_blob(),
        weight: u16::MAX,
    };
    // SAFETY: the sublayer, provider key, and display strings remain live for
    // the synchronous add operation.
    wfp_success_or_exists("FwpmSubLayerAdd0", unsafe {
        FwpmSubLayerAdd0(engine, &raw const sublayer, null_mut())
    })?;
    verify_sublayer(engine)
}

fn verify_provider(engine: HANDLE) -> io::Result<()> {
    let provider_key = PROVIDER_KEY;
    let mut provider = null_mut::<FWPM_PROVIDER0>();
    // SAFETY: the engine and fixed provider key are live and `provider` is
    // initialized out-pointer storage owned by FwpmFreeMemory0 on success.
    let status =
        unsafe { FwpmProviderGetByKey0(engine, &raw const provider_key, &raw mut provider) };
    if status != 0 {
        return Err(wfp_error("FwpmProviderGetByKey0", status));
    }
    let valid = if provider.is_null() {
        false
    } else {
        // SAFETY: the successful get returned a live FWPM_PROVIDER0.
        let provider = unsafe { &*provider };
        guid_eq(&provider.providerKey, &PROVIDER_KEY)
            && provider.flags == FWPM_PROVIDER_FLAG_PERSISTENT
            && provider.providerData.size == 0
            && provider.serviceName.is_null()
    };
    // SAFETY: WFP returned this allocation through `provider`; the pointer
    // variable is passed exactly as FwpmFreeMemory0 requires.
    unsafe {
        FwpmFreeMemory0((&raw mut provider).cast::<*mut c_void>());
    }
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Windows sandbox WFP provider does not match the installed policy",
        ))
    }
}

fn verify_sublayer(engine: HANDLE) -> io::Result<()> {
    let sublayer_key = SUBLAYER_KEY;
    let mut sublayer = null_mut::<FWPM_SUBLAYER0>();
    // SAFETY: the engine and fixed sublayer key are live and `sublayer` is
    // initialized out-pointer storage owned by FwpmFreeMemory0 on success.
    let status =
        unsafe { FwpmSubLayerGetByKey0(engine, &raw const sublayer_key, &raw mut sublayer) };
    if status != 0 {
        return Err(wfp_error("FwpmSubLayerGetByKey0", status));
    }
    let valid = if sublayer.is_null() {
        false
    } else {
        // SAFETY: the successful get returned a live FWPM_SUBLAYER0.
        let sublayer = unsafe { &*sublayer };
        !sublayer.providerKey.is_null()
            // SAFETY: the provider pointer was checked non-null and belongs
            // to the still-live WFP sublayer allocation.
            && guid_eq(unsafe { &*sublayer.providerKey }, &PROVIDER_KEY)
            && guid_eq(&sublayer.subLayerKey, &SUBLAYER_KEY)
            && sublayer.flags == FWPM_SUBLAYER_FLAG_PERSISTENT
            && sublayer.providerData.size == 0
            && sublayer.weight == u16::MAX
    };
    // SAFETY: WFP returned this allocation through `sublayer`; the pointer
    // variable is passed exactly as FwpmFreeMemory0 requires.
    unsafe {
        FwpmFreeMemory0((&raw mut sublayer).cast::<*mut c_void>());
    }
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Windows sandbox WFP sublayer does not match the installed policy",
        ))
    }
}

fn add_filter(
    engine: HANDLE,
    key: &GUID,
    spec: &FilterSpec,
    user: &UserCondition,
) -> io::Result<()> {
    let name = wide(spec.name);
    let description = wide(spec.description);
    let mut condition = FWPM_FILTER_CONDITION0 {
        fieldKey: FWPM_CONDITION_ALE_USER_ID,
        matchType: FWP_MATCH_EQUAL,
        conditionValue: FWP_CONDITION_VALUE0 {
            r#type: FWP_SECURITY_DESCRIPTOR_TYPE,
            Anonymous: FWP_CONDITION_VALUE0_0 {
                sd: (&raw const user.blob).cast_mut(),
            },
        },
    };
    let provider_key = PROVIDER_KEY;
    let filter = FWPM_FILTER0 {
        filterKey: *key,
        displayData: FWPM_DISPLAY_DATA0 {
            name: name.as_ptr().cast_mut(),
            description: description.as_ptr().cast_mut(),
        },
        flags: FWPM_FILTER_FLAG_PERSISTENT,
        providerKey: (&raw const provider_key).cast_mut(),
        providerData: empty_blob(),
        layerKey: spec.layer,
        subLayerKey: SUBLAYER_KEY,
        weight: empty_value(),
        numFilterConditions: 1,
        filterCondition: &raw mut condition,
        action: FWPM_ACTION0 {
            r#type: FWP_ACTION_BLOCK,
            Anonymous: FWPM_ACTION0_0 {
                filterType: GUID::from_u128(0),
            },
        },
        Anonymous: FWPM_FILTER0_0 { rawContext: 0 },
        reserved: null_mut(),
        filterId: 0,
        effectiveWeight: empty_value(),
    };
    let mut id = 0_u64;
    // SAFETY: the filter and every pointer it contains reference live values
    // for the synchronous add call; `id` is initialized out storage.
    wfp_success("FwpmFilterAdd0", unsafe {
        FwpmFilterAdd0(engine, &raw const filter, null_mut(), &raw mut id)
    })
}

fn delete_filter(engine: HANDLE, key: &GUID) -> io::Result<()> {
    // SAFETY: the engine and deterministic filter key are live for the call.
    let status = unsafe { FwpmFilterDeleteByKey0(engine, key) };
    if status == 0
        || status == FWP_E_FILTER_NOT_FOUND.cast_unsigned()
        || status == FWP_E_NOT_FOUND.cast_unsigned()
    {
        Ok(())
    } else {
        Err(wfp_error("FwpmFilterDeleteByKey0", status))
    }
}

fn wfp_success(operation: &str, status: u32) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(wfp_error(operation, status))
    }
}

fn wfp_success_or_exists(operation: &str, status: u32) -> io::Result<()> {
    if status == 0 || status == FWP_E_ALREADY_EXISTS.cast_unsigned() {
        Ok(())
    } else {
        Err(wfp_error(operation, status))
    }
}

fn wfp_error(operation: &str, status: u32) -> io::Error {
    io::Error::other(format!("{operation} failed: 0x{status:08X}"))
}

fn empty_blob() -> FWP_BYTE_BLOB {
    FWP_BYTE_BLOB {
        size: 0,
        data: null_mut(),
    }
}

fn empty_value() -> FWP_VALUE0 {
    FWP_VALUE0 {
        r#type: FWP_EMPTY,
        ..FWP_VALUE0::default()
    }
}

fn guid_eq(left: &GUID, right: &GUID) -> bool {
    left.data1 == right.data1
        && left.data2 == right.data2
        && left.data3 == right.data3
        && left.data4 == right.data4
}

fn condition_matches_user(condition: &FWPM_FILTER_CONDITION0, expected: &UserCondition) -> bool {
    // SAFETY: the caller checked that the tagged union contains an SD blob.
    let actual = unsafe { condition.conditionValue.Anonymous.sd };
    if actual.is_null() {
        return false;
    }
    // SAFETY: WFP returned the pointed-to blob as part of the live filter.
    let actual = unsafe { &*actual };
    if actual.size != expected.blob.size
        || actual.size == 0
        || actual.size > MAX_SECURITY_DESCRIPTOR_BYTES
        || actual.data.is_null()
        || expected.blob.data.is_null()
    {
        return false;
    }
    let length = actual.size as usize;
    // SAFETY: both descriptors are live and readable for their equal bounded
    // advertised sizes during this comparison.
    unsafe {
        std::slice::from_raw_parts(actual.data, length)
            == std::slice::from_raw_parts(expected.blob.data, length)
    }
}
