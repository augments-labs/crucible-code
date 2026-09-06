//! Bounded HTTP proxy request validation before any DNS or socket operation.

use std::io;

use crucible_core::{SandboxDomainPolicy, SandboxNetworkEndpoint};

pub(super) const MAX_HEADER_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Body {
    Fixed(u64),
    Chunked,
}

pub(super) struct Request {
    pub(super) endpoint: SandboxNetworkEndpoint,
    pub(super) header: Vec<u8>,
    pub(super) body: Body,
    pub(super) tunnel: bool,
    pub(super) expect_continue: bool,
}

pub(super) fn parse(
    bytes: &[u8],
    credential: &[u8],
    policy: &SandboxDomainPolicy,
) -> io::Result<Request> {
    if bytes.len() > MAX_HEADER_BYTES {
        return Err(invalid());
    }
    let mut slots = [httparse::EMPTY_HEADER; 64];
    let mut parsed = httparse::Request::new(&mut slots);
    if parsed.parse(bytes).map_err(|_| invalid())? != httparse::Status::Complete(bytes.len()) {
        return Err(invalid());
    }
    if parsed.version != Some(1) && parsed.version != Some(0) {
        return Err(invalid());
    }
    let method = parsed.method.ok_or_else(invalid)?;
    let target = parsed.path.ok_or_else(invalid)?;
    let tunnel = method == "CONNECT";
    let (endpoint, path) = target_endpoint(target, tunnel, policy.provenance())?;
    let authorization = unique(parsed.headers, "proxy-authorization")?.ok_or_else(unauthorized)?;
    if !same_credential(authorization, credential) {
        return Err(unauthorized());
    }
    if !policy.permits_host(endpoint.host()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "sandbox network target denied",
        ));
    }
    if let Some(host) = unique(parsed.headers, "host")? {
        let host = std::str::from_utf8(host).map_err(|_| invalid())?;
        let host = authority(
            host,
            if tunnel { endpoint.port() } else { 80 },
            policy.provenance(),
        )?;
        if host != endpoint {
            return Err(invalid());
        }
    }
    let content_length = unique(parsed.headers, "content-length")?;
    let transfer_encoding = unique(parsed.headers, "transfer-encoding")?;
    let body = match (content_length, transfer_encoding) {
        (Some(value), None) => {
            if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
                return Err(invalid());
            }
            Body::Fixed(
                std::str::from_utf8(value)
                    .map_err(|_| invalid())?
                    .parse()
                    .map_err(|_| invalid())?,
            )
        }
        (None, Some(value)) if value.eq_ignore_ascii_case(b"chunked") => Body::Chunked,
        (_, Some(_)) => return Err(invalid()),
        (None, None) => Body::Fixed(0),
    };
    let expect_continue = match unique(parsed.headers, "expect")? {
        None => false,
        Some(value) if value.eq_ignore_ascii_case(b"100-continue") => true,
        Some(_) => return Err(invalid()),
    };
    if tunnel && (body != Body::Fixed(0) || expect_continue) {
        return Err(invalid());
    }
    let connection = connection_tokens(parsed.headers)?;
    if tunnel {
        return Ok(Request {
            endpoint,
            header: Vec::new(),
            body,
            tunnel,
            expect_continue,
        });
    }
    let mut header = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
        host_header(&endpoint)
    )
    .into_bytes();
    for field in parsed.headers.iter() {
        let name = field.name.to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "host"
                | "connection"
                | "proxy-authorization"
                | "proxy-connection"
                | "proxy-authenticate"
                | "keep-alive"
                | "te"
                | "trailer"
                | "upgrade"
                | "expect"
        ) || connection.iter().any(|token| token == &name)
        {
            continue;
        }
        header.extend_from_slice(field.name.as_bytes());
        header.extend_from_slice(b": ");
        header.extend_from_slice(field.value);
        header.extend_from_slice(b"\r\n");
    }
    header.extend_from_slice(b"\r\n");
    // Normalization adds a small fixed header, but never removes the retained-data bound.
    if header.len() > MAX_HEADER_BYTES {
        return Err(invalid());
    }
    Ok(Request {
        endpoint,
        header,
        body,
        tunnel,
        expect_continue,
    })
}

fn target_endpoint(
    target: &str,
    tunnel: bool,
    provenance: crucible_core::SandboxNetworkProvenance,
) -> io::Result<(SandboxNetworkEndpoint, String)> {
    if tunnel {
        let parsed = target
            .parse::<http::uri::Authority>()
            .map_err(|_| invalid())?;
        if parsed.port_u16().is_none() {
            return Err(invalid());
        }
        return Ok((authority(target, 443, provenance)?, String::new()));
    }
    let parsed = target.parse::<http::Uri>().map_err(|_| invalid())?;
    if parsed.scheme_str() != Some("http") || target.contains('#') {
        return Err(invalid());
    }
    let endpoint = authority(
        parsed.authority().ok_or_else(invalid)?.as_str(),
        80,
        provenance,
    )?;
    let path = parsed
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str);
    if !path.starts_with('/') {
        return Err(invalid());
    }
    Ok((endpoint, path.to_owned()))
}

fn authority(
    value: &str,
    default_port: u16,
    provenance: crucible_core::SandboxNetworkProvenance,
) -> io::Result<SandboxNetworkEndpoint> {
    let parsed = value
        .parse::<http::uri::Authority>()
        .map_err(|_| invalid())?;
    let suffix = value.strip_prefix(parsed.host()).ok_or_else(invalid)?;
    if value.contains('@')
        || (!suffix.is_empty() && (!suffix.starts_with(':') || parsed.port_u16().is_none()))
    {
        return Err(invalid());
    }
    let host = parsed.host();
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    SandboxNetworkEndpoint::new(host, parsed.port_u16().unwrap_or(default_port), provenance)
        .map_err(|_| invalid())
}

fn host_header(endpoint: &SandboxNetworkEndpoint) -> String {
    if endpoint.host().contains(':') {
        format!("[{}]:{}", endpoint.host(), endpoint.port())
    } else {
        format!("{}:{}", endpoint.host(), endpoint.port())
    }
}

fn unique<'a>(headers: &[httparse::Header<'a>], name: &str) -> io::Result<Option<&'a [u8]>> {
    let mut found = None;
    for field in headers
        .iter()
        .filter(|field| field.name.eq_ignore_ascii_case(name))
    {
        if found.replace(field.value).is_some() {
            return Err(invalid());
        }
    }
    Ok(found)
}

fn connection_tokens(headers: &[httparse::Header<'_>]) -> io::Result<Vec<String>> {
    let mut tokens = Vec::new();
    for field in headers
        .iter()
        .filter(|field| field.name.eq_ignore_ascii_case("connection"))
    {
        let value = std::str::from_utf8(field.value).map_err(|_| invalid())?;
        for token in value.split(',').map(str::trim) {
            let name =
                http::header::HeaderName::from_bytes(token.as_bytes()).map_err(|_| invalid())?;
            if matches!(
                name.as_str(),
                "host" | "content-length" | "transfer-encoding" | "proxy-authorization"
            ) || tokens.len() == 64
            {
                return Err(invalid());
            }
            tokens.push(name.as_str().to_owned());
        }
    }
    Ok(tokens)
}

fn same_credential(actual: &[u8], expected: &[u8]) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected)
            .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid or oversized sandbox proxy request",
    )
}

fn unauthorized() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "sandbox proxy authentication refused",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crucible_core::{SandboxDomainPattern, SandboxNetworkProvenance};

    const AUTH: &[u8] = b"Basic c3ludGhldGljOm9ubHk=";

    fn policy() -> SandboxDomainPolicy {
        SandboxDomainPolicy::new(
            [SandboxDomainPattern::new("*.example.com").unwrap()],
            [SandboxDomainPattern::new("denied.example.com").unwrap()],
            false,
            [],
            SandboxNetworkProvenance::User,
        )
        .unwrap()
    }

    fn parse_request(first: &str, headers: &str) -> io::Result<Request> {
        parse(
            format!("{first}\r\nProxy-Authorization: Basic c3ludGhldGljOm9ubHk=\r\n{headers}\r\n")
                .as_bytes(),
            AUTH,
            &policy(),
        )
    }

    #[test]
    fn connect_authorizes_the_exact_target_and_never_forwards_proxy_credentials() {
        let request = parse_request(
            "CONNECT BUILD.EXAMPLE.COM:443 HTTP/1.1",
            "Host: build.example.com:443\r\n",
        )
        .unwrap();
        assert!(request.tunnel);
        assert_eq!(request.endpoint.host(), "build.example.com");
        assert_eq!(request.endpoint.port(), 443);
        assert!(request.header.is_empty());
        assert_eq!(request.body, Body::Fixed(0));
    }

    #[test]
    fn a_plain_request_has_one_origin_authority_and_bounded_body_framing() {
        let request = parse_request("POST http://build.example.com:8080/source?q=one HTTP/1.1", "Host: BUILD.EXAMPLE.COM:8080\r\nContent-Length: 7\r\nConnection: keep-alive, x-remove\r\nX-Remove: secret\r\nX-Keep: value\r\n").unwrap();
        assert!(!request.tunnel);
        assert_eq!(request.endpoint.port(), 8080);
        assert_eq!(request.body, Body::Fixed(7));
        let header = String::from_utf8(request.header).unwrap();
        assert!(header.starts_with("POST /source?q=one HTTP/1.1\r\n"));
        assert!(header.contains("Host: build.example.com:8080\r\n"));
        assert!(header.contains("Connection: close\r\n"));
        assert!(header.contains("X-Keep: value\r\n"));
        for secret in ["Proxy-Authorization", "c3ludGh", "X-Remove", "keep-alive"] {
            assert!(!header.contains(secret), "{header}");
        }
    }

    #[test]
    fn ambiguous_or_unauthorized_requests_are_refused_before_network_work() {
        for (first, headers) in [
            ("CONNECT denied.example.com:443 HTTP/1.1", ""),
            ("CONNECT elsewhere.test:443 HTTP/1.1", ""),
            ("CONNECT example.com:443 HTTP/1.1", ""),
            ("CONNECT build.example.com HTTP/1.1", ""),
            ("CONNECT build.example.com:0 HTTP/1.1", ""),
            ("GET http://build.example.com:invalid/ HTTP/1.1", ""),
            ("GET http://build.example.com:99999/ HTTP/1.1", ""),
            ("CONNECT user@build.example.com:443 HTTP/1.1", ""),
            ("CONNECT build.example.com:443/path HTTP/1.1", ""),
            (
                "CONNECT build.example.com:443 HTTP/1.1",
                "Content-Length: 1\r\n",
            ),
            ("GET /relative HTTP/1.1", "Host: build.example.com\r\n"),
            ("GET https://build.example.com/ HTTP/1.1", ""),
            (
                "GET http://build.example.com/ HTTP/1.1",
                "Host: denied.example.com\r\n",
            ),
            (
                "GET http://build.example.com/ HTTP/1.1",
                "Host: build.example.com\r\nHost: build.example.com\r\n",
            ),
            (
                "POST http://build.example.com/ HTTP/1.1",
                "Content-Length: 1\r\nContent-Length: 1\r\n",
            ),
            (
                "POST http://build.example.com/ HTTP/1.1",
                "Content-Length: 1\r\nTransfer-Encoding: chunked\r\n",
            ),
            (
                "POST http://build.example.com/ HTTP/1.1",
                "Transfer-Encoding: gzip, chunked\r\n",
            ),
            (
                "POST http://build.example.com/ HTTP/1.1",
                "Connection: content-length\r\nContent-Length: 2\r\n",
            ),
            (
                "GET http://build.example.com/ HTTP/1.1",
                "Proxy-Authorization: another\r\n",
            ),
        ] {
            assert!(parse_request(first, headers).is_err(), "{first}\n{headers}");
        }
        assert!(
            parse(
                b"CONNECT build.example.com:443 HTTP/1.1\r\n\r\n",
                AUTH,
                &policy()
            )
            .is_err()
        );
        assert!(parse(&vec![b'x'; MAX_HEADER_BYTES + 1], AUTH, &policy()).is_err());
    }

    #[test]
    fn header_count_and_trailing_request_bytes_are_bounded() {
        let extra = "X-Field: value\r\n".repeat(64);
        assert!(parse_request("GET http://build.example.com/ HTTP/1.1", &extra).is_err());
        assert!(
            parse_request(
                "GET http://build.example.com/ HTTP/1.1",
                "\r\nGET http://denied.example.com/ HTTP/1.1\r\n"
            )
            .is_err()
        );
    }

    #[test]
    fn chunked_uploads_are_preserved_and_expect_continue_is_handled_by_the_mediator() {
        let request = parse_request(
            "POST http://build.example.com/ HTTP/1.1",
            "Transfer-Encoding: chunked\r\nExpect: 100-continue\r\n",
        )
        .unwrap();
        assert_eq!(request.body, Body::Chunked);
        assert!(request.expect_continue);
        let header = String::from_utf8(request.header).unwrap();
        assert!(header.contains("Transfer-Encoding: chunked\r\n"));
        assert!(!header.contains("Expect:"));
    }
}
