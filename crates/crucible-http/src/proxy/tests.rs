use super::{ProxyEnv, Route, select};

/// Variables set, a target, and where a request for it goes.
type Row<'a> = (&'a [(&'a str, &'a str)], &'a str, &'a str);

/// The environment that holds only `vars`.
fn env(vars: &[(&str, &str)]) -> ProxyEnv {
    ProxyEnv::read(|name| {
        vars.iter()
            .find(|(set, _)| *set == name)
            .map(|(_, value)| (*value).to_owned())
    })
}

/// Where a request for `target` goes when the environment holds only `vars`:
/// `direct`, `refused`, or the proxy it is tunnelled through and the
/// credential it is sent.
fn route(vars: &[(&str, &str)], target: &str) -> String {
    let env = env(vars);
    match select(&env, &target.parse().unwrap()) {
        Route::Direct => "direct".to_owned(),
        Route::Refused => "refused".to_owned(),
        Route::Tunnel(proxy) => match &proxy.authorization {
            Some(basic) => format!("{} {}", proxy.uri, basic.to_str().unwrap()),
            None => proxy.uri.to_string(),
        },
    }
}

/// The choices the previous client (ureq 3.4.2) made from the environment,
/// as its source makes them: the six variables in order, the first one that is
/// set and parses serving every scheme; a missing scheme taken as `http`; a
/// SOCKS proxy connected past or not at all; the user information sent as a
/// `Basic` credential split at its last `:`; and `NO_PROXY` read before
/// `no_proxy`, split on commas untrimmed, matched as a whole host, a prefix,
/// a suffix or everything, ASCII case-insensitively and without CIDR.
#[test]
fn a_proxy_is_chosen_from_the_environment_as_before() {
    let p = "http://p:3128/";
    let rows: &[Row<'_>] = &[
        (&[], "https://api.test/", "direct"),
        (&[("ALL_PROXY", "http://p:3128")], "https://api.test/", p),
        (&[("all_proxy", "http://p:3128")], "http://api.test/", p),
        (&[("HTTPS_PROXY", "http://p:3128")], "http://api.test/", p),
        (&[("https_proxy", "http://p:3128")], "http://api.test/", p),
        (&[("HTTP_PROXY", "http://p:3128")], "https://api.test/", p),
        (&[("http_proxy", "http://p:3128")], "https://api.test/", p),
        (
            &[("ALL_PROXY", "http://p:3128"), ("all_proxy", "http://q:1")],
            "https://a.test/",
            p,
        ),
        (
            &[
                ("all_proxy", "http://p:3128"),
                ("HTTPS_PROXY", "http://q:1"),
            ],
            "https://a.test/",
            p,
        ),
        (
            &[
                ("HTTPS_PROXY", "http://p:3128"),
                ("https_proxy", "http://q:1"),
            ],
            "https://a.test/",
            p,
        ),
        (
            &[
                ("https_proxy", "http://p:3128"),
                ("HTTP_PROXY", "http://q:1"),
            ],
            "https://a.test/",
            p,
        ),
        (
            &[
                ("HTTP_PROXY", "http://p:3128"),
                ("http_proxy", "http://q:1"),
            ],
            "https://a.test/",
            p,
        ),
        (
            &[("ALL_PROXY", "ftp://q:1"), ("HTTPS_PROXY", "http://p:3128")],
            "https://a.test/",
            p,
        ),
        (
            &[("ALL_PROXY", ""), ("HTTPS_PROXY", "http://p:3128")],
            "https://a.test/",
            p,
        ),
        (
            &[("ALL_PROXY", "/p"), ("HTTPS_PROXY", "http://p:3128")],
            "https://a.test/",
            p,
        ),
        (
            &[("ALL_PROXY", "http://u@"), ("HTTPS_PROXY", "http://p:3128")],
            "https://a.test/",
            p,
        ),
        (
            &[
                ("ALL_PROXY", "http://[a@p]:1"),
                ("HTTPS_PROXY", "http://q:1"),
            ],
            "https://a.test/",
            "refused",
        ),
        (&[("ALL_PROXY", "p:3128")], "https://a.test/", p),
        (&[("ALL_PROXY", "HTTP://p:3128")], "https://a.test/", p),
        (
            &[("ALL_PROXY", "http://p:3128/a/path")],
            "https://a.test/",
            p,
        ),
        (&[("ALL_PROXY", "p")], "https://a.test/", "http://p/"),
        (
            &[("ALL_PROXY", "https://p")],
            "https://a.test/",
            "https://p/",
        ),
        (
            &[("ALL_PROXY", "http://[::1]:3128")],
            "https://a.test/",
            "http://[::1]:3128/",
        ),
        (
            &[("ALL_PROXY", "socks5://p:1080")],
            "https://a.test/",
            "direct",
        ),
        (
            &[("ALL_PROXY", "socks://p:1080")],
            "https://a.test/",
            "direct",
        ),
        (
            &[("ALL_PROXY", "socks4://p:1080")],
            "https://a.test/",
            "direct",
        ),
        (
            &[("ALL_PROXY", "socks4a://p:1080")],
            "https://a.test/",
            "refused",
        ),
        (
            &[("ALL_PROXY", "SOCKS5H://p:1080")],
            "https://a.test/",
            "refused",
        ),
        (
            &[("ALL_PROXY", "http://user:pass@p:3128")],
            "https://a.test/",
            "http://p:3128/ Basic dXNlcjpwYXNz",
        ),
        (
            &[("ALL_PROXY", "http://u@p:1")],
            "https://a.test/",
            "http://p:1/ Basic dTo=",
        ),
        (
            &[("ALL_PROXY", "http://a:b:c@p:1")],
            "https://a.test/",
            "http://p:1/ Basic YTpiOmM=",
        ),
        (
            &[("ALL_PROXY", "http://a@b@p:1")],
            "https://a.test/",
            "http://p:1/ Basic YUBiOg==",
        ),
    ];
    let no_proxy: &[(&str, &str, &str)] = &[
        ("example.com", "https://example.com/", "direct"),
        ("example.com", "https://sub.example.com/", p),
        (".example.com", "https://sub.example.com/", "direct"),
        (".example.com", "https://example.com/", p),
        ("*.example.com", "https://a.b.example.com/", "direct"),
        ("*example.com", "https://myexample.com/", "direct"),
        ("*", "https://anything.test/", "direct"),
        ("api*", "https://api.vendor.test/", "direct"),
        ("10.", "http://10.1.2.3/", "direct"),
        ("Example.COM", "https://EXAMPLE.com/", "direct"),
        ("a.test, b.test", "https://b.test/", p),
        ("a.test,b.test", "https://b.test/", "direct"),
        ("a.test", "https://a.test:8443/", "direct"),
        ("a.test:8443", "https://a.test:8443/", p),
        ("[::1]", "http://[::1]:8080/", "direct"),
        ("192.168.0.0/16", "http://192.168.1.1/", p),
        ("", "https://a.test/", p),
    ];
    let mut wrong = Vec::new();
    for (vars, target, expected) in rows {
        let chosen = route(vars, target);
        if chosen != *expected {
            wrong.push(format!("{vars:?} {target}: {chosen}, expected {expected}"));
        }
    }
    for (list, target, expected) in no_proxy {
        let vars = [("ALL_PROXY", "http://p:3128"), ("NO_PROXY", *list)];
        let chosen = route(&vars, target);
        if chosen != *expected {
            wrong.push(format!(
                "NO_PROXY={list:?} {target}: {chosen}, expected {expected}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The upper-case variable is read first and wins even when it is empty, and
/// a matched host bypasses a SOCKS proxy that would otherwise be refused.
#[test]
fn no_proxy_is_read_upper_case_first_even_when_empty() {
    let proxied = [("ALL_PROXY", "http://p:3128")];
    let both = [proxied[0], ("NO_PROXY", ""), ("no_proxy", "a.test")];
    assert_eq!(route(&both, "https://a.test/"), "http://p:3128/");
    let lower = [proxied[0], ("no_proxy", "a.test")];
    assert_eq!(route(&lower, "https://a.test/"), "direct");
    let socks = [("ALL_PROXY", "socks5h://p:1080"), ("NO_PROXY", "a.test")];
    assert_eq!(route(&socks, "https://a.test/"), "direct");
    assert_eq!(route(&socks, "https://b.test/"), "refused");
}

/// A `CONNECT` names the previous client, asks the proxy to keep the
/// connection, and carries the credential marked sensitive, which is also
/// what is registered for redaction. The address a tunnel is opened to holds
/// no user information, and neither the snapshot's `Debug` nor the header
/// map's shows the credential.
#[test]
fn a_tunnel_says_what_the_previous_client_said_and_keeps_its_credential_hidden() {
    let env = env(&[("HTTPS_PROXY", "http://user:secret@p:3128")]);
    let Route::Tunnel(proxy) = select(&env, &"https://a.test/".parse().unwrap()) else {
        panic!("expected a tunnel");
    };
    let headers = proxy.headers();
    assert_eq!(headers.get("user-agent").unwrap(), "ureq/3.4.2");
    assert_eq!(headers.get("proxy-connection").unwrap(), "Keep-Alive");
    let basic = headers.get("proxy-authorization").unwrap();
    assert_eq!(basic, "Basic dXNlcjpzZWNyZXQ=");
    assert!(basic.is_sensitive());
    assert_eq!(proxy.uri.to_string(), "http://p:3128/");

    let mut outgoing = crucible_credentials::Outgoing::new();
    proxy.protect(&mut outgoing);
    let shown = outgoing.redactions().redact("echoed dXNlcjpzZWNyZXQ=");
    assert!(!shown.contains("dXNlcjpzZWNyZXQ="), "{shown}");

    for shown in [format!("{env:?}"), format!("{headers:?}")] {
        assert!(!shown.contains("secret"), "{shown}");
        assert!(!shown.contains("dXNlcjpzZWNyZXQ="), "{shown}");
    }
}
