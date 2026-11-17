//! Use case A: a visitor books as a guest; the intake shows who they are, and the guest's chat moves to that client.
//!
//! `cargo run --example identity_guest` (Ada, already a client: the chat moves to her), or with `-- new` (Grace, not
//! a client yet: the guest becomes her). Needs `DATABASE_URL`; each run makes its own tenant.

#[path = "common/identity.rs"]
mod common;

use common::{Browser, Fallible, Salon, action};
use megh::messaging::Surface;
use serde_json::json;

#[tokio::main]
async fn main() -> Fallible<()> {
    let (first, last, email) = match std::env::args().any(|arg| arg == "new") {
        true => ("Grace", "Hopper", "grace@example.com"),
        false => ("Ada", "Lovelace", "ada@example.com"),
    };
    let mut salon = Salon::open().await?;
    let mut browser = Browser::visit(&salon).await?;
    let guest = browser.client;
    browser.say("I'd like to book a haircut").await?;
    salon.heard().await?;
    let choice = salon.choose(&mut browser).await?;

    salon.say(Surface::intake(), guest).await?;
    let form = browser.next().await?;
    browser.press(&form, "submit", json!({"first_name": first, "last_name": last, "name": format!("{first} {last}"), "email": email})).await?;
    let details = action(&salon.heard().await?);
    salon.verify(&mut browser).await?;
    let browser = match salon.known(email).await? {
        Some(client) => salon.identified(browser, client).await?,
        None => {
            salon.api("PATCH", &format!("clients/{guest}"), details).await?;
            browser
        }
    };
    let mut browser = browser;
    let client = browser.client;
    let booked = salon.book(&mut browser, client, &choice).await?;

    println!("guest {guest} is now client {client}; booked {} for that client", booked["id"]);
    println!("messages: the client has {}, the guest {}", salon.messages_of(client).await?, salon.messages_of(guest).await?);
    Ok(())
}
