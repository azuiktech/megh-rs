//! Who may follow a thread's events: the grants of the token that opened the stream.

use crate::auth::{AccessToken, Grant};
use crate::event::Topic;

/// For `Broker::with_authorizer`: `tenants/{t}/threads/{id}` needs `tenants:read:{t}` and `threads:read:{id}`,
/// `tenants/{t}/threads` needs `tenants:read:{t}` and `threads:read`; no other topic, and no token, is allowed.
pub fn authorizer() -> impl Fn(Option<&AccessToken>, &Topic) -> bool + Send + Sync + 'static {
    |token, topic| token.is_some_and(|token| follows(&token.grants(), topic))
}

fn follows(grants: &[Grant], topic: &Topic) -> bool {
    let implied = |grant: String| grants.iter().any(|granted| granted.implies(&Grant::new(grant.as_str())));
    match topic.split('/').collect::<Vec<_>>()[..] {
        ["tenants", tenant, "threads", thread] => implied(format!("tenants:read:{tenant}")) && implied(format!("threads:read:{thread}")),
        ["tenants", tenant, "threads"] => implied(format!("tenants:read:{tenant}")) && implied("threads:read".into()),
        _ => false,
    }
}
