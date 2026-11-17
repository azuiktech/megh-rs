//! Use case C: the host app already knows the client (as kyrios does). It issues a client token; the browser chats
//! and books with it, with no guest at all.
//!
//! `cargo run --example identity_known`. Needs `DATABASE_URL`.

#[path = "common/identity.rs"]
mod common;

use common::{Browser, Fallible, Salon};

#[tokio::main]
async fn main() -> Fallible<()> {
    let mut salon = Salon::open().await?;
    let token = salon.jwt.issue_client(salon.tenant, salon.ada)?.jwt;
    let mut browser = Browser::signed_in(&salon, salon.ada, token).await?;
    browser.say("I'd like to book a haircut").await?;
    salon.heard().await?;
    let choice = salon.choose(&mut browser).await?;
    let booked = salon.book(&mut browser, salon.ada, &choice).await?;

    println!("client {} booked {}; messages: {}", salon.ada, booked["id"], salon.messages_of(salon.ada).await?);
    Ok(())
}
