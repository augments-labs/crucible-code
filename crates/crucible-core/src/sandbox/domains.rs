//! Canonical domain grants, overriding denies, and local socket authority.
//!
//! A mediator checks the requested host before DNS and the resolved address
//! before connecting. Native backends separately prevent direct-network bypass.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::policy::{SandboxNetworkEndpoint, SandboxNetworkProvenance, SandboxPolicyError};

/// Maximum entries in each domain or Unix-socket list.
pub const MAX_SANDBOX_NETWORK_RULES: usize = 64;

/// One canonical hostname, literal IP address, `*.domain` pattern, or `*`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SandboxDomainPattern {
    spelling: Box<str>,
}

impl SandboxDomainPattern {
    /// Parses the complete bounded pattern; URL, port and interior wildcards are refused.
    ///
    /// # Errors
    ///
    /// The pattern is not a bounded DNS name, address or supported wildcard.
    pub fn new(value: &str) -> Result<Self, SandboxPolicyError> {
        if value == "*" {
            return Ok(Self {
                spelling: value.into(),
            });
        }
        let suffix = value.strip_prefix("*.");
        let endpoint = SandboxNetworkEndpoint::new(
            suffix.unwrap_or(value),
            443,
            SandboxNetworkProvenance::User,
        )?;
        if suffix.is_some() && endpoint.host().parse::<IpAddr>().is_ok() {
            return Err(SandboxPolicyError::InvalidEndpoint);
        }
        let spelling = if suffix.is_some() {
            format!("*.{}", endpoint.host()).into_boxed_str()
        } else {
            canonical_address(endpoint.host()).into()
        };
        Ok(Self { spelling })
    }

    /// Canonical spelling. Keep this out of persisted inspection and diagnostic logs.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.spelling
    }

    /// Matches a canonical host at complete DNS label boundaries.
    #[must_use]
    pub fn matches(&self, host: &str) -> bool {
        self.spelling.as_ref() == "*"
            || self.spelling.as_ref() == host
            || self.spelling.strip_prefix("*.").is_some_and(|suffix| {
                host.strip_suffix(suffix)
                    .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.'))
            })
    }

    /// Whether every host named by this pattern is also named by `parent`.
    #[must_use]
    pub fn is_no_wider_than(&self, parent: &Self) -> bool {
        self == parent
            || parent.spelling.as_ref() == "*"
            || (self.spelling.as_ref() != "*"
                && (!self.spelling.starts_with("*.") || parent.spelling.starts_with("*."))
                && parent.matches(self.spelling.strip_prefix("*.").unwrap_or(&self.spelling)))
    }
}

impl std::fmt::Debug for SandboxDomainPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SandboxDomainPattern([host pattern])")
    }
}

/// Immutable domain mediation and local-connection policy.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxDomainPolicy {
    allowed: Box<[SandboxDomainPattern]>,
    denied: Box<[SandboxDomainPattern]>,
    local_binding: bool,
    unix_sockets: Box<[PathBuf]>,
    provenance: SandboxNetworkProvenance,
}

impl SandboxDomainPolicy {
    /// Canonicalizes bounded lists. Socket paths must already use absolute native syntax.
    ///
    /// # Errors
    ///
    /// A list exceeds its bound or a socket path is non-normalized or oversized.
    pub fn new(
        allowed: impl IntoIterator<Item = SandboxDomainPattern>,
        denied: impl IntoIterator<Item = SandboxDomainPattern>,
        local_binding: bool,
        unix_sockets: impl IntoIterator<Item = PathBuf>,
        provenance: SandboxNetworkProvenance,
    ) -> Result<Self, SandboxPolicyError> {
        let allowed = bounded(allowed)?;
        let denied = bounded(denied)?;
        let unix_sockets = bounded(unix_sockets)?;
        for path in &unix_sockets {
            super::policy::validate_absolute_path(path)?;
        }
        Ok(Self {
            allowed,
            denied,
            local_binding,
            unix_sockets,
            provenance,
        })
    }

    /// Requested hosts admitted before DNS. Denials always win.
    #[must_use]
    pub fn permits_host(&self, host: &str) -> bool {
        let Ok(host) = SandboxNetworkEndpoint::new(host, 443, self.provenance) else {
            return false;
        };
        let host = canonical_address(host.host());
        self.allowed.iter().any(|rule| rule.matches(&host))
            && !self.denied.iter().any(|rule| rule.matches(&host))
    }

    /// Resolved addresses admitted after host authorization.
    ///
    /// A hostname or `*` never grants private, loopback, link-local or reserved
    /// address space. Those addresses require an exact literal allow entry, and
    /// address denies still win. The mediator connects to this exact address.
    #[must_use]
    pub fn permits_address(&self, address: IpAddr) -> bool {
        let address = match address {
            IpAddr::V6(value) => value.to_ipv4_mapped().map_or(address, IpAddr::V4),
            IpAddr::V4(_) => address,
        };
        let name = address.to_string();
        !self.denied.iter().any(|rule| rule.matches(&name))
            && (public_address(address) || self.allowed.iter().any(|rule| rule.as_str() == name))
    }

    /// Host grants, canonically sorted.
    #[must_use]
    pub fn allowed(&self) -> &[SandboxDomainPattern] {
        &self.allowed
    }
    /// Overriding host/address denies, canonically sorted.
    #[must_use]
    pub fn denied(&self) -> &[SandboxDomainPattern] {
        &self.denied
    }
    /// Whether confined workloads may bind local listeners.
    #[must_use]
    pub const fn allow_local_binding(&self) -> bool {
        self.local_binding
    }
    /// Exact host Unix sockets which may be reached.
    #[must_use]
    pub fn unix_sockets(&self) -> &[PathBuf] {
        &self.unix_sockets
    }
    /// Authority of the effective request.
    #[must_use]
    pub const fn provenance(&self) -> SandboxNetworkProvenance {
        self.provenance
    }
    /// Whether no network reach is granted at all.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.allowed.is_empty() && !self.local_binding && self.unix_sockets.is_empty()
    }
    /// Whether one exact host socket is authorized.
    #[must_use]
    pub fn permits_socket(&self, path: &Path) -> bool {
        self.unix_sockets.iter().any(|allowed| allowed == path)
    }

    /// Proves narrowing conservatively without expanding wildcard sets.
    #[must_use]
    pub fn is_no_wider_than(&self, parent: &Self) -> bool {
        self.provenance >= parent.provenance
            && (!self.local_binding || parent.local_binding)
            && self
                .unix_sockets
                .iter()
                .all(|path| parent.permits_socket(path))
            && self
                .allowed
                .iter()
                .all(|rule| parent.allowed.iter().any(|p| rule.is_no_wider_than(p)))
            && parent
                .denied
                .iter()
                .all(|rule| self.denied.iter().any(|d| rule.is_no_wider_than(d)))
    }

    pub(super) fn update_digest(&self, digest: &mut Sha256) {
        digest.update(b"\0domains");
        for rules in [&self.allowed, &self.denied] {
            digest.update((rules.len() as u64).to_be_bytes());
            for rule in rules {
                digest.update(rule.as_str().as_bytes());
                digest.update([0]);
            }
        }
        digest.update([u8::from(self.local_binding), self.provenance as u8]);
        for socket in &self.unix_sockets {
            digest.update(socket.as_os_str().as_encoded_bytes());
            digest.update([0]);
        }
    }
}

impl std::fmt::Debug for SandboxDomainPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxDomainPolicy")
            .field("allowed", &self.allowed.len())
            .field("denied", &self.denied.len())
            .field("local_binding", &self.local_binding)
            .field("unix_sockets", &self.unix_sockets.len())
            .field("provenance", &self.provenance)
            .finish()
    }
}

fn canonical_address(host: &str) -> String {
    match host
        .parse::<Ipv6Addr>()
        .ok()
        .and_then(|address| address.to_ipv4_mapped())
    {
        Some(address) => address.to_string(),
        None => host.to_owned(),
    }
}

fn bounded<T: Ord>(items: impl IntoIterator<Item = T>) -> Result<Box<[T]>, SandboxPolicyError> {
    let mut items: Vec<_> = items
        .into_iter()
        .take(MAX_SANDBOX_NETWORK_RULES + 1)
        .collect();
    if items.len() > MAX_SANDBOX_NETWORK_RULES {
        return Err(SandboxPolicyError::InvalidNetworkRuleCount);
    }
    items.sort();
    items.dedup();
    Ok(items.into_boxed_slice())
}

fn public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => public_v4(address),
        IpAddr::V6(address) => public_v6(address),
    }
}

fn public_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    // Exclude special-use, documentation, benchmarking and multicast ranges.
    !(matches!(a, 0 | 10 | 127 | 224..=255)
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && (b == 168 || (b == 0 && matches!(c, 0 | 2)) || (b == 88 && c == 99)))
        || (a == 198 && (matches!(b, 18 | 19) || (b == 51 && c == 100)))
        || (a == 203 && b == 0 && c == 113))
}

fn public_v6(address: Ipv6Addr) -> bool {
    let [a, b, _, _, _, _, _, _] = address.segments();
    // Admit global unicast only; exclude protocol assignments, documentation,
    // 6to4 and the documentation prefix. Mapped IPv4 is checked as IPv4 above.
    (a & 0xe000) == 0x2000
        && !(a == 0x2001 && (b < 0x200 || b == 0xdb8))
        && a != 0x2002
        && !(a == 0x3fff && (b & 0xf000) == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SandboxNetworkProvenance as Source;

    fn rule(value: &str) -> SandboxDomainPattern {
        SandboxDomainPattern::new(value).unwrap()
    }

    fn policy(allowed: &[&str], denied: &[&str]) -> SandboxDomainPolicy {
        SandboxDomainPolicy::new(
            allowed.iter().map(|value| rule(value)),
            denied.iter().map(|value| rule(value)),
            false,
            [],
            Source::User,
        )
        .unwrap()
    }

    #[test]
    fn domains_are_canonical_and_wildcards_respect_label_boundaries() {
        assert_eq!(rule("EXAMPLE.COM."), rule("example.com"));
        assert_eq!(rule("::ffff:127.0.0.1"), rule("127.0.0.1"));
        let wildcard = rule("*.example.com");
        assert!(wildcard.matches("a.example.com"));
        assert!(wildcard.matches("a.b.example.com"));
        assert!(!wildcard.matches("example.com"));
        assert!(!wildcard.matches("badexample.com"));
        assert!(!wildcard.matches("example.com.attacker.test"));
        assert!(!wildcard.is_no_wider_than(&rule("example.com")));
        for bad in [
            "",
            "http://example.com",
            "example.com:443",
            "a.*.test",
            "*.127.0.0.1",
            "a/b",
            "a\0b",
            "999.0.0.1",
        ] {
            assert!(SandboxDomainPattern::new(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn denies_win_and_an_empty_allowlist_never_allows_egress() {
        let policy = policy(&["*.example.com"], &["blocked.example.com"]);
        assert!(policy.permits_host("build.example.com"));
        assert!(!policy.permits_host("blocked.example.com"));
        assert!(!policy.permits_host("elsewhere.test"));
        assert!(!super::tests::policy(&[], &[]).permits_host("example.com"));
        assert!(super::tests::policy(&["*"], &[]).permits_host("example.com"));
    }

    #[test]
    fn descendants_cannot_drop_denies_or_gain_local_authority() {
        let parent = policy(&["*.example.com"], &["blocked.example.com"]);
        assert!(policy(&["build.example.com"], &["blocked.example.com"]).is_no_wider_than(&parent));
        assert!(!policy(&["*.example.com"], &[]).is_no_wider_than(&parent));
        assert!(!policy(&["*"], &["blocked.example.com"]).is_no_wider_than(&parent));
        let binding = SandboxDomainPolicy::new(
            [rule("*.example.com")],
            [rule("blocked.example.com")],
            true,
            [],
            Source::User,
        )
        .unwrap();
        assert!(!binding.is_no_wider_than(&parent));
    }

    #[test]
    fn a_public_name_cannot_rebind_to_private_network_authority() {
        let policy = policy(&["example.com"], &[]);
        assert!(policy.permits_address("93.184.216.34".parse().unwrap()));
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "192.168.1.1",
            "::1",
            "::ffff:127.0.0.1",
            "fe80::1",
            "fc00::1",
            "0.0.0.0",
        ] {
            assert!(
                !policy.permits_address(address.parse().unwrap()),
                "{address}"
            );
        }
        assert!(
            super::tests::policy(&["127.0.0.1"], &[]).permits_address("127.0.0.1".parse().unwrap())
        );
        assert!(!super::tests::policy(&["*"], &[]).permits_address("127.0.0.1".parse().unwrap()));
        assert!(
            !super::tests::policy(&["*"], &["93.184.216.34"])
                .permits_address("93.184.216.34".parse().unwrap())
        );
    }
}
