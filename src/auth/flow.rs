//! OAuth attempt state: the `state` and PKCE cookies set when a flow starts and checked at the callback.

use axum::http::{HeaderMap, StatusCode};
use oauth2::{CsrfToken, PkceCodeChallenge, PkceCodeVerifier};

use super::http::extract_cookie;

const MAX_AGE_SECS: u32 = 600;

/// What an OAuth round trip is for; carried in the `state` value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Login,
    Connect,
}

impl Flow {
    fn prefix(self) -> &'static str {
        match self {
            Flow::Login => "login",
            Flow::Connect => "connect",
        }
    }

    fn of_state(state: &str) -> Option<Self> {
        match state.split_once('.')?.0 {
            "login" => Some(Flow::Login),
            "connect" => Some(Flow::Connect),
            _ => None,
        }
    }
}

/// A started flow: the values for the authorization URL and the cookies that bind the browser to them.
pub struct Attempt {
    pub csrf: CsrfToken,
    pub challenge: PkceCodeChallenge,
    pub cookies: [String; 2],
}

fn state_cookie(provider: &str) -> String {
    format!("_oauth_state_{provider}")
}

fn pkce_cookie(provider: &str) -> String {
    format!("_oauth_pkce_{provider}")
}

fn cookie(name: &str, value: &str, max_age: u32, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")
}

pub fn begin(provider: &str, flow: Flow, secure: bool) -> Attempt {
    let csrf = CsrfToken::new(format!("{}.{}", flow.prefix(), CsrfToken::new_random().secret()));
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let cookies = [
        cookie(&state_cookie(provider), csrf.secret(), MAX_AGE_SECS, secure),
        cookie(&pkce_cookie(provider), verifier.secret(), MAX_AGE_SECS, secure),
    ];
    Attempt { csrf, challenge, cookies }
}

/// Accepts the callback only when its `state` equals the cookie set at the start, and returns the flow and PKCE verifier.
pub fn verify(headers: &HeaderMap, provider: &str, state: Option<&str>) -> Result<(Flow, PkceCodeVerifier), (StatusCode, &'static str)> {
    let expected = extract_cookie(headers, &state_cookie(provider)).map(CsrfToken::new);
    let given = state.map(|s| CsrfToken::new(s.to_string()));
    let flow = match (expected, given) {
        (Some(expected), Some(given)) if expected == given => Flow::of_state(given.secret()),
        _ => None,
    }
    .ok_or((StatusCode::FORBIDDEN, "invalid OAuth state"))?;
    let verifier = extract_cookie(headers, &pkce_cookie(provider))
        .map(PkceCodeVerifier::new)
        .ok_or((StatusCode::BAD_REQUEST, "missing PKCE verifier"))?;
    Ok((flow, verifier))
}

pub fn clear_cookies(provider: &str) -> [String; 2] {
    [cookie(&state_cookie(provider), "", 0, false), cookie(&pkce_cookie(provider), "", 0, false)]
}
