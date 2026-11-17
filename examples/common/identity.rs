//! What the identity examples share: a salon on Postgres served over real HTTP, its agent (a machine member that
//! listens with `on` and books through the booking API), and a browser played over HTTP and SSE with its own cookies.
//! Each example uses part of it.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use axum::middleware::{from_fn, from_fn_with_state};
use axum::{Extension, Router};
use chrono::{Datelike, NaiveDate, Utc};
use megh::auth::rest::AppContext;
use megh::auth::{JwtSession, jwt_session};
use megh::messaging::{History, Message, Messaging, Party, Surface};
use megh::{Broker, UserRepo, authorizer, events_router};
use reqwest::header::{AUTHORIZATION, COOKIE, SET_COOKIE};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tower::Layer;
use tower::util::MapRequestLayer;
use tower_sessions::{MemoryStore, SessionManagerLayer};
use uuid::Uuid;

pub type Fallible<T> = Result<T, Box<dyn Error>>;

/// The one-time code the salon "sends": this month and year (`112026`). Sending and checking codes is the app's.
pub fn code() -> String {
    Utc::now().format("%m%Y").to_string()
}

/// A salon open on weekdays, its agent and the client already on its books.
pub struct Salon {
    pub tenant: Uuid,
    pub url: String,
    pub jwt: JwtSession,
    pub messaging: Messaging,
    pub agent: Uuid,
    pub ada: Uuid,
    token: String,
    http: reqwest::Client,
    inbox: UnboundedReceiver<Message>,
    haircut: String,
    stylists: Vec<(String, String)>,
}

/// What the client chose: the stylist and the open time.
pub struct Choice {
    pub stylist: String,
    pub slot: Value,
}

#[derive(Serialize)]
struct Summary {
    service: &'static str,
    stylist: String,
    starts_at: String,
}

impl Salon {
    /// A new salon in its own tenant, served on a free local port.
    pub async fn open() -> Fallible<Self> {
        dotenvy::dotenv().ok();
        let pool = PgPool::connect(&std::env::var("DATABASE_URL")?).await?;
        megh::migrate(&pool).await?;
        let tenant: Uuid = sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Salon') RETURNING id").fetch_one(&pool).await?;
        let agent = UserRepo::new(&pool).machine(tenant, &format!("agent-{tenant}"), vec!["*".into()]).await?;
        let jwt = JwtSession::new(b"identity-example-secret-32-bytes", Duration::from_secs(900), pool.clone());
        let broker = Broker::new().with_authorizer(megh::messaging::follows).with_history_provider(History::new(pool.clone()));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        tokio::spawn(serve(listener, app(pool.clone(), broker.clone(), jwt.clone(), &url)));
        let messaging = Messaging::new(pool, broker, tenant).with_files_at(&format!("{url}/api/v1"));
        let (heard, inbox) = unbounded_channel();
        messaging.on(Party::Member(agent.id), move |message| heard.send(message).unwrap_or(()));
        let token = jwt.issue(&agent, &[])?.jwt;
        let empty = Self { tenant, url, jwt, messaging, agent: agent.id, ada: Uuid::nil(), token, http: reqwest::Client::new(), inbox, haircut: String::new(), stylists: Vec::new() };
        empty.furnished().await
    }

    /// The service, two stylists, a chair, weekday hours and Ada, made through the API as the agent.
    async fn furnished(self) -> Fallible<Self> {
        let stylist = |name: &str| json!({"first_name": name, "last_name": "Stylist", "name": name, "role": "stylist"});
        let mia = self.id("providers", stylist("Mia")).await?;
        let sam = self.id("providers", stylist("Sam")).await?;
        self.id("resources", json!({"name": "Chair 1", "role": "chair"})).await?;
        let weekdays = "DTSTART;TZID=UTC:20260105T000000\nRRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR";
        self.id("schedules", json!({"name": "Open", "recurrence": weekdays, "starts_at": "09:00:00", "ends_at": "17:00:00"})).await?;
        let haircut = self.id("services", json!({"name": "Haircut", "steps": [{"duration": "PT30M", "provider1": "stylist"}], "price": 40000000, "currency": "USD"})).await?;
        let ada = self.id("clients", json!({"first_name": "Ada", "last_name": "Lovelace", "name": "Ada Lovelace", "email": "ada@example.com"})).await?;
        Ok(Self { ada: ada.parse()?, haircut, stylists: vec![("Mia".into(), mia), ("Sam".into(), sam)], ..self })
    }

    async fn id(&self, path: &str, body: Value) -> Fallible<String> {
        Ok(self.api("POST", path, body).await?["id"].as_str().ok_or("no id")?.to_string())
    }

    /// The tenant's API, called as the agent.
    pub async fn api(&self, method: &str, path: &str, body: Value) -> Fallible<Value> {
        let url = format!("{}/api/v1/tenants/{}/{path}", self.url, self.tenant);
        let request = self.http.request(method.parse()?, url).header(AUTHORIZATION, format!("Bearer {}", self.token));
        let request = match method {
            "GET" => request,
            _ => request.json(&body),
        };
        let response = request.send().await?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        match status.is_success() {
            true => Ok(body),
            false => Err(format!("{method} {path}: {status} {body}").into()),
        }
    }

    /// The next message a client sent the agent.
    pub async fn heard(&mut self) -> Fallible<Message> {
        let message = tokio::time::timeout(Duration::from_secs(5), self.inbox.recv()).await?.ok_or("the agent stopped hearing")?;
        println!("  agent heard  {}", message.body);
        Ok(message)
    }

    pub async fn say(&self, surface: Surface, client: Uuid) -> Fallible<()> {
        self.messaging.send(surface, Party::Member(self.agent), Party::Client(client)).await?;
        Ok(())
    }

    /// The live client with this email, if the salon has one.
    pub async fn known(&self, email: &str) -> Fallible<Option<Uuid>> {
        let page = self.api("GET", &format!("clients?filter=email%20%3D%20%22{}%22", email.replace('@', "%40")), Value::Null).await?;
        Ok(page["items"].as_array().into_iter().flatten().filter_map(|client| client["id"].as_str()?.parse().ok()).next())
    }

    /// Steps 2 to 5: a stylist for the haircut, a day, then an open time that day.
    pub async fn choose(&mut self, browser: &mut Browser) -> Fallible<Choice> {
        let stylists = self.stylists.iter().map(|(name, id)| (name.as_str(), id.as_str())).collect::<Vec<_>>();
        self.say(Surface::new().text("A haircut - with whom?").choice("stylist", &stylists).button("Send", "submit"), browser.client).await?;
        let picked = browser.next().await?;
        browser.press(&picked, "submit", json!({"stylist": self.stylists[0].1})).await?;
        let stylist = action(&self.heard().await?)["stylist"].as_str().ok_or("no stylist")?.to_string();
        self.say(Surface::new().text("Which day?").calendar("date").button("Show times", "times"), browser.client).await?;
        let day = browser.next().await?;
        browser.press(&day, "times", json!({"date": next_weekday().to_string()})).await?;
        let date: NaiveDate = action(&self.heard().await?)["date"].as_str().ok_or("no date")?.parse()?;
        let slots = self.open_times(&stylist, date, browser.client).await?;
        let times = slots.iter().map(|slot| (slot["starts_at"].as_str().unwrap_or_default(), slot["starts_at"].as_str().unwrap_or_default())).collect::<Vec<_>>();
        self.say(Surface::times(&format!("Open times on {date}"), &times), browser.client).await?;
        let offered = browser.next().await?;
        browser.press(&offered, "submit", json!({"slot": times[0].1})).await?;
        let at = action(&self.heard().await?)["slot"].clone();
        Ok(Choice { stylist, slot: slots.into_iter().find(|slot| slot["starts_at"] == at).ok_or("no such time")? })
    }

    async fn open_times(&self, stylist: &str, date: NaiveDate, client: Uuid) -> Fallible<Vec<Value>> {
        let party = json!({"client": {"id": client}, "service_id": self.haircut, "providers": [{"id": stylist}]});
        let day = json!({"from": format!("{date}T00:00:00Z"), "to": format!("{}T00:00:00Z", date.succ_opt().ok_or("no next day")?), "parties": [party], "page_size": 3});
        let page = self.api("POST", "slots:search", day).await?;
        Ok(page["items"].as_array().into_iter().flatten().map(|slot| slot["parties"][0].clone()).collect())
    }

    /// The salon's own check that the client is who they say: a code it "sent", typed back.
    pub async fn verify(&mut self, browser: &mut Browser) -> Fallible<()> {
        self.say(Surface::new().text("We sent you a code.").input("Code", "code").button("Verify", "verify"), browser.client).await?;
        let asked = browser.next().await?;
        browser.press(&asked, "verify", json!({"code": code()})).await?;
        let typed = action(&self.heard().await?)["code"].clone();
        (typed == json!(code())).then_some(()).ok_or_else(|| "wrong code".into())
    }

    /// The guest turned out to be `client`: its chat moves there, and the browser, told it moved, becomes the client.
    pub async fn identified(&mut self, browser: Browser, client: Uuid) -> Fallible<Browser> {
        self.messaging.identify(browser.client, client).await?;
        let mut browser = browser;
        let moved = browser.next().await?;
        (moved["signal"]["moved"] == json!(client)).then_some(()).ok_or("not told it moved")?;
        browser.opened().await
    }

    /// Steps 9 and 10: the appointment shown with a Book button; booked for `client` when pressed.
    pub async fn book(&mut self, browser: &mut Browser, client: Uuid, choice: &Choice) -> Fallible<Value> {
        let stylist = self.stylists.iter().find(|(_, id)| *id == choice.stylist).map(|(name, _)| name.clone()).unwrap_or_default();
        let summary = Summary { service: "Haircut", stylist, starts_at: choice.slot["starts_at"].as_str().unwrap_or_default().into() };
        self.say(Surface::new().card(|card| card.title("Your appointment").fields(&summary).button("Book", "book")), client).await?;
        let card = browser.next().await?;
        browser.press(&card, "book", json!({})).await?;
        self.heard().await?;
        let slot = &choice.slot;
        let appointment = json!({
            "client_id": client, "service_id": self.haircut, "resource_id": slot["resources"][0]["id"], "provider_ids": [choice.stylist],
            "starts_at": slot["starts_at"], "ends_at": slot["ends_at"], "status": "booked", "payment": {"payer_id": client, "method": "card"}
        });
        let booked = self.api("POST", "appointments", appointment).await?;
        self.say(Surface::new().text(&format!("Booked for {}. See you then!", summary.starts_at)), client).await?;
        browser.next().await?;
        Ok(booked)
    }

    /// How many messages the client has, as the database holds them.
    pub async fn messages_of(&self, client: Uuid) -> Fallible<i64> {
        let pool = PgPool::connect(&std::env::var("DATABASE_URL")?).await?;
        Ok(sqlx::query_scalar("SELECT count(*) FROM messages WHERE tenant_id = $1 AND client_id = $2").bind(self.tenant).bind(client).fetch_one(&pool).await?)
    }
}

/// The action's context, from a message the browser sent.
pub fn action(message: &Message) -> Value {
    message.body["action"]["context"].clone()
}

fn next_weekday() -> NaiveDate {
    let today = Utc::now().date_naive();
    today.iter_days().skip(1).find(|day| day.weekday().num_days_from_monday() < 5).unwrap_or(today)
}

/// The host app: the public guest route, the messaging and booking routes signed in by token, and the event stream.
fn app(pool: PgPool, broker: Broker, jwt: JwtSession, url: &str) -> Router {
    let (guests, _) = megh::messaging::guests(jwt.clone()).split_for_parts();
    let (api, _) = megh::messaging::router(5 << 20, &format!("{url}/api/v1")).merge(megh::booking::router()).split_for_parts();
    let signed = Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .nest_service("/events", events_router(Arc::new(broker.clone())))
        .layer(from_fn_with_state(jwt, jwt_session));
    Router::new()
        .nest("/api/v1", guests)
        .merge(signed)
        .layer(Extension(broker))
        .layer(SessionManagerLayer::new(MemoryStore::default()))
        .with_state(AppContext { pool })
}

async fn serve(listener: TcpListener, app: Router) {
    let app = MapRequestLayer::new(megh::booking::rewrite_aip_136).layer(app);
    axum::serve(listener, axum::ServiceExt::into_make_service(app)).await.unwrap_or(());
}

/// A browser on the salon's widget: its cookies or a bearer token, its client, and the event stream it follows.
pub struct Browser {
    pub client: Uuid,
    http: reqwest::Client,
    url: String,
    tenant: Uuid,
    member: Uuid,
    cookies: BTreeMap<String, String>,
    bearer: Option<String>,
    stream: Option<reqwest::Response>,
    buffer: String,
}

impl Browser {
    /// A first visit: the guest route makes this browser a guest.
    pub async fn visit(salon: &Salon) -> Fallible<Self> {
        let browser = Self { client: Uuid::nil(), http: reqwest::Client::new(), url: salon.url.clone(), tenant: salon.tenant, member: salon.agent, cookies: BTreeMap::new(), bearer: None, stream: None, buffer: String::new() };
        browser.opened().await
    }

    /// A browser the host already signed in as a known client, with a token it issued.
    pub async fn signed_in(salon: &Salon, client: Uuid, token: String) -> Fallible<Self> {
        let browser = Self { client, http: reqwest::Client::new(), url: salon.url.clone(), tenant: salon.tenant, member: salon.agent, cookies: BTreeMap::new(), bearer: Some(token), stream: None, buffer: String::new() };
        browser.following().await
    }

    /// Calls the guest route (again): the client this browser is now, and its event stream.
    pub async fn opened(mut self) -> Fallible<Self> {
        let response = self.post(&format!("tenants/{}/guests", self.tenant), json!({})).await?;
        self.client = response["client_id"].as_str().ok_or("no client")?.parse()?;
        println!("  browser is   client {}", self.client);
        self.following().await
    }

    async fn following(mut self) -> Fallible<Self> {
        let topic = format!("tenants/{}/clients/{}/members/{}", self.tenant, self.client, self.member);
        let request = self.signed(self.http.get(format!("{}/events/stream?topic={topic}", self.url)));
        self.stream = Some(request.send().await?.error_for_status()?);
        self.buffer.clear();
        Ok(self)
    }

    fn signed(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let jar = self.cookies.iter().map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>().join("; ");
        let request = request.header(COOKIE, jar);
        match &self.bearer {
            Some(token) => request.header(AUTHORIZATION, format!("Bearer {token}")),
            None => request,
        }
    }

    async fn post(&mut self, path: &str, body: Value) -> Fallible<Value> {
        let response = self.signed(self.http.post(format!("{}/api/v1/{path}", self.url)).json(&body)).send().await?.error_for_status()?;
        let set = response.headers().get_all(SET_COOKIE).iter().filter_map(|value| value.to_str().ok()?.split(';').next()?.split_once('=').map(|(name, value)| (name.to_string(), value.to_string())));
        self.cookies.extend(set.collect::<Vec<_>>());
        Ok(response.json().await.unwrap_or(Value::Null))
    }

    /// What the user types.
    pub async fn say(&mut self, text: &str) -> Fallible<()> {
        println!("  browser says {text}");
        self.post(&format!("tenants/{}/members/{}/messages", self.tenant, self.member), json!({"body": {"text": text}})).await.map(|_| ())
    }

    /// What the renderer sends when a button of `message` is pressed with the inputs filled as `context`.
    pub async fn press(&mut self, message: &Value, name: &str, context: Value) -> Fallible<()> {
        let surface = message["body"][0]["createSurface"]["surfaceId"].clone();
        let pressed = json!({"version": "1.0", "action": {"name": name, "surfaceId": surface, "context": context}});
        self.post(&format!("tenants/{}/members/{}/messages", self.tenant, self.member), json!({"body": pressed})).await.map(|_| ())
    }

    /// The next delivery from the agent on the stream: a message, or an event such as `moved`.
    pub async fn next(&mut self) -> Fallible<Value> {
        match self.take() {
            Some(delivery) => {
                println!("  browser got  {}", short(&delivery));
                Ok(delivery)
            }
            None => {
                self.read().await?;
                Box::pin(self.next()).await
            }
        }
    }

    /// The first whole event in the buffer that the agent sent, dropping those before it.
    fn take(&mut self) -> Option<Value> {
        let end = self.buffer.find("\n\n")?;
        let event = self.buffer.drain(..end + 2).collect::<String>();
        from_agent(&event).or_else(|| self.take())
    }

    async fn read(&mut self) -> Fallible<()> {
        let stream = self.stream.as_mut().ok_or("not following")?;
        let chunk = tokio::time::timeout(Duration::from_secs(5), stream.chunk()).await??.ok_or("the stream ended")?;
        self.buffer.push_str(&String::from_utf8_lossy(&chunk));
        Ok(())
    }
}

/// The delivery an SSE event carries when the agent sent it.
fn from_agent(event: &str) -> Option<Value> {
    let data = event.lines().filter_map(|line| line.strip_prefix("data:")).map(str::trim).collect::<String>();
    serde_json::from_str::<Value>(&data).ok().filter(|delivery| delivery["from"].get("member").is_some())
}

fn short(delivery: &Value) -> String {
    let text = delivery["body"][1]["updateComponents"]["components"].as_array().into_iter().flatten().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>();
    match delivery["kind"].as_str() {
        Some("event") => format!("event {}", delivery["signal"]),
        _ => text.join(" | "),
    }
}
