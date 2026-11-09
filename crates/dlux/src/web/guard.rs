//! Request checks that run before every handler.
//!
//! A local web server is reachable from any website open in the user's browser, so even on
//! 127.0.0.1 without a token dlux defends against:
//! - **DNS rebinding**: without a token, only loopback `Host` names are accepted;
//! - **cross-site requests / WebSocket hijacking**: a request with an `Origin` must come from
//!   the same host:port it is sent to (WebSockets are not covered by CORS, so this matters);
//! - **form-based CSRF**: bodies with a content type an HTML form can send cross-site
//!   (`text/plain`, urlencoded, multipart) are refused.
//!
//! With a token configured (required when binding to a non-loopback address), every request
//! must also carry it: `Authorization: Bearer <token>`, a `dlux_token` cookie, or `?token=`
//! (for the first page load and WebSockets). A valid `?token=` sets the cookie.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

pub const COOKIE: &str = "dlux_token";

#[derive(Debug, Clone, Default)]
pub struct Guard {
    pub token: Option<String>,
}

/// Compare without leaking how many leading bytes matched.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A refused request: the status and the reason sent as `{"error": …}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Denied(pub StatusCode, pub &'static str);

impl IntoResponse for Denied {
    fn into_response(self) -> Response {
        (self.0, axum::Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

fn deny(status: StatusCode, msg: &'static str) -> Denied {
    Denied(status, msg)
}

/// The host part of a `Host` header value ("localhost:8080" → "localhost", "[::1]:80" → "::1").
fn host_name(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("");
    }
    host.rsplit_once(':').map_or(host, |(h, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            host
        }
    })
}

fn is_loopback_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "localhost"
        || name.ends_with(".localhost")
        || name
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// `Origin` → its "host[:port]" authority, as sent in `Host`.
fn origin_authority(origin: &str) -> Option<String> {
    let url = url::Url::parse(origin).ok()?;
    // `host_str` already brackets IPv6 addresses ("[::1]"), as the Host header does.
    let host = url.host_str()?;
    Some(match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_owned(),
    })
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|kv| {
            let (k, v) = kv.trim().split_once('=')?;
            (k == COOKIE).then(|| v.to_owned())
        })
}

fn query_token(query: Option<&str>) -> Option<String> {
    url::form_urlencoded::parse(query?.as_bytes())
        .find(|(k, _)| k == "token")
        .map(|(_, v)| v.into_owned())
}

/// Content types an HTML form can submit cross-site without a CORS preflight.
fn form_content_type(headers: &HeaderMap) -> bool {
    let Some(ct) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let ct = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    matches!(
        ct.as_str(),
        "text/plain" | "application/x-www-form-urlencoded" | "multipart/form-data"
    )
}

impl Guard {
    /// Check a request. `Ok(Some(token))` means a valid `?token=` that should be stored in a
    /// cookie.
    pub fn check(
        &self,
        method: &Method,
        headers: &HeaderMap,
        query: Option<&str>,
    ) -> Result<Option<String>, Denied> {
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if self.token.is_none() && !is_loopback_name(host_name(host)) {
            return Err(deny(
                StatusCode::FORBIDDEN,
                "dlux only answers to localhost unless a web token is configured",
            ));
        }
        if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
            let same = origin_authority(origin).is_some_and(|a| a.eq_ignore_ascii_case(host));
            if !same {
                return Err(deny(StatusCode::FORBIDDEN, "cross-origin request refused"));
            }
        }
        if *method != Method::GET && *method != Method::HEAD && form_content_type(headers) {
            return Err(deny(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "send JSON (or the raw file for uploads)",
            ));
        }
        let Some(expected) = &self.token else {
            return Ok(None);
        };
        let bearer = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::to_owned);
        let from_query = query_token(query);
        let ok = |t: &Option<String>| {
            t.as_deref()
                .is_some_and(|t| constant_time_eq(t.as_bytes(), expected.as_bytes()))
        };
        if ok(&bearer) || ok(&cookie_token(headers)) {
            Ok(None)
        } else if ok(&from_query) {
            // Remember it so the page's own requests don't need it in the URL.
            Ok(from_query)
        } else {
            Err(deny(
                StatusCode::UNAUTHORIZED,
                "a valid web token is required",
            ))
        }
    }
}

pub async fn middleware(
    State(guard): State<Arc<Guard>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let set_cookie = match guard.check(req.method(), req.headers(), req.uri().query()) {
        Ok(c) => c,
        Err(denied) => return denied.into_response(),
    };
    let mut resp = next.run(req).await;
    if let Some(token) = set_cookie {
        let cookie = format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict");
        if let Ok(v) = HeaderValue::from_str(&cookie) {
            resp.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    fn status(r: Result<Option<String>, Denied>) -> StatusCode {
        match r {
            Ok(_) => StatusCode::OK,
            Err(Denied(s, _)) => s,
        }
    }

    #[test]
    fn hosts_and_origins() {
        let g = Guard::default();
        let get = Method::GET;
        for host in [
            "localhost:8080",
            "127.0.0.1:8080",
            "[::1]:8080",
            "app.localhost",
        ] {
            assert_eq!(
                status(g.check(&get, &headers(&[("host", host)]), None)),
                StatusCode::OK,
                "{host}"
            );
        }
        assert_eq!(
            status(g.check(&get, &headers(&[("host", "evil.example:8080")]), None)),
            StatusCode::FORBIDDEN,
            "DNS rebinding"
        );
        assert_eq!(
            status(g.check(&get, &HeaderMap::new(), None)),
            StatusCode::FORBIDDEN
        );
        let same = headers(&[
            ("host", "127.0.0.1:8080"),
            ("origin", "http://127.0.0.1:8080"),
        ]);
        assert_eq!(status(g.check(&Method::POST, &same, None)), StatusCode::OK);
        let cross = headers(&[
            ("host", "127.0.0.1:8080"),
            ("origin", "https://evil.example"),
        ]);
        assert_eq!(
            status(g.check(&Method::POST, &cross, None)),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(g.check(&get, &cross, None)),
            StatusCode::FORBIDDEN,
            "WebSocket hijack"
        );
        let other_port = headers(&[
            ("host", "localhost:8080"),
            ("origin", "http://localhost:3000"),
        ]);
        assert_eq!(
            status(g.check(&Method::POST, &other_port, None)),
            StatusCode::FORBIDDEN
        );
        let null = headers(&[("host", "localhost:8080"), ("origin", "null")]);
        assert_eq!(
            status(g.check(&Method::POST, &null, None)),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn form_bodies_are_refused() {
        let g = Guard::default();
        for ct in [
            "text/plain",
            "application/x-www-form-urlencoded",
            "multipart/form-data; boundary=x",
        ] {
            let h = headers(&[("host", "localhost"), ("content-type", ct)]);
            assert_eq!(
                status(g.check(&Method::POST, &h, None)),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "{ct}"
            );
        }
        let json = headers(&[("host", "localhost"), ("content-type", "application/json")]);
        assert_eq!(status(g.check(&Method::POST, &json, None)), StatusCode::OK);
    }

    #[test]
    fn tokens() {
        let g = Guard {
            token: Some("s3cret".into()),
        };
        let lan = |extra: &[(&'static str, &str)]| {
            let mut pairs = vec![("host", "192.168.1.5:8080")];
            pairs.extend_from_slice(extra);
            headers(&pairs)
        };
        assert_eq!(
            status(g.check(&Method::GET, &lan(&[]), None)),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(g.check(
                &Method::GET,
                &lan(&[("authorization", "Bearer s3cret")]),
                None
            )),
            StatusCode::OK,
            "any host is fine with a token"
        );
        assert_eq!(
            status(g.check(
                &Method::GET,
                &lan(&[("authorization", "Bearer nope")]),
                None
            )),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(g.check(
                &Method::GET,
                &lan(&[("cookie", "a=b; dlux_token=s3cret")]),
                None
            )),
            StatusCode::OK
        );
        assert_eq!(
            g.check(&Method::GET, &lan(&[]), Some("x=1&token=s3cret"))
                .unwrap(),
            Some("s3cret".into()),
            "query token sets the cookie"
        );
        assert!(
            g.check(&Method::GET, &lan(&[]), Some("token=s3cre"))
                .is_err()
        );
    }

    #[test]
    fn helpers() {
        assert_eq!(host_name("localhost:8080"), "localhost");
        assert_eq!(host_name("[::1]:8080"), "::1");
        assert_eq!(host_name("example.com"), "example.com");
        assert_eq!(
            origin_authority("http://[::1]:8080").as_deref(),
            Some("[::1]:8080")
        );
        assert_eq!(
            origin_authority("http://localhost").as_deref(),
            Some("localhost")
        );
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
