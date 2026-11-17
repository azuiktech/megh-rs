//! Use case B: the widget asks who the visitor is first. A known email gets a code and the chat moves to that
//! client; an unknown one gets a new-client form with the email filled in and a code; then the booking.
//!
//! `cargo run --example identity_upfront` (Ada, a client) or with `-- new` (Grace, not yet). Needs `DATABASE_URL`.

#[path = "common/identity.rs"]
mod common;

use common::{Browser, Fallible, Salon, action};
use megh::messaging::Surface;
use serde_json::json;

#[tokio::main]
async fn main() -> Fallible<()> {
    let email = match std::env::args().any(|arg| arg == "new") {
        true => "grace@example.com",
        false => "ada@example.com",
    };
    let mut salon = Salon::open().await?;
    let mut browser = Browser::visit(&salon).await?;
    let guest = browser.client;
    browser.say("hi").await?;
    salon.heard().await?;

    salon.say(Surface::new().text("Welcome! Your email or phone?").input("Email or phone", "contact").button("Send", "submit"), guest).await?;
    let asked = browser.next().await?;
    browser.press(&asked, "submit", json!({"contact": email})).await?;
    let contact = action(&salon.heard().await?)["contact"].as_str().unwrap_or_default().to_string();
    let browser = match salon.known(&contact).await? {
        Some(client) => {
            salon.verify(&mut browser).await?;
            salon.identified(browser, client).await?
        }
        None => {
            salon.say(Surface::intake().filled(&json!({"email": contact})), guest).await?;
            let form = browser.next().await?;
            browser.press(&form, "submit", json!({"first_name": "Grace", "last_name": "Hopper", "name": "Grace Hopper", "email": contact})).await?;
            let details = action(&salon.heard().await?);
            salon.verify(&mut browser).await?;
            salon.api("PATCH", &format!("clients/{guest}"), details).await?;
            browser
        }
    };
    let mut browser = browser;
    let client = browser.client;
    let choice = salon.choose(&mut browser).await?;
    let booked = salon.book(&mut browser, client, &choice).await?;

    println!("guest {guest} is now client {client}; booked {} for that client", booked["id"]);
    println!("messages: the client has {}, the guest {}", salon.messages_of(client).await?, salon.messages_of(guest).await?);
    Ok(())
}
