//! Messenger-style templates and their limits.

use serde_json::{Value, json};

use super::Surface;
use super::component::{self as ui, Group};
use crate::billing::invoice::Invoice;
use crate::billing::payment::Payment;
use crate::messaging::MessagingError;

const QUICK_REPLIES: usize = 13;
const QUICK_REPLY_TITLE: usize = 20;
const BUTTONS: usize = 3;
const CARDS: usize = 10;
const CARD_TEXT: usize = 80;

#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub title: String,
    pub payload: String,
}

impl Reply {
    pub fn new(title: impl Into<String>, payload: impl Into<String>) -> Self {
        Self { title: title.into(), payload: payload.into() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    pub title: String,
    pub payload: String,
}

impl Button {
    pub fn new(title: impl Into<String>, payload: impl Into<String>) -> Self {
        Self { title: title.into(), payload: payload.into() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub title: String,
    pub subtitle: Option<String>,
    pub buttons: Vec<Button>,
}

impl Card {
    pub fn new(title: impl Into<String>) -> Self {
        Self { title: title.into(), subtitle: None, buttons: Vec::new() }
    }

    pub fn subtitle(self, subtitle: impl Into<String>) -> Self {
        Self { subtitle: Some(subtitle.into()), ..self }
    }

    pub fn button(self, button: Button) -> Self {
        Self { buttons: self.buttons.into_iter().chain([button]).collect(), ..self }
    }
}

fn within(fits: bool, what: &str) -> Result<(), MessagingError> {
    fits.then_some(()).ok_or_else(|| MessagingError::InvalidSurface(format!("{what} is over the Messenger limit")))
}

fn within_chars(text: &str, max: usize, what: &str) -> Result<(), MessagingError> {
    within(text.chars().count() <= max, what)
}

fn answers(prefix: &str, key: &str, options: &[(&str, &str)]) -> Vec<Group> {
    options
        .iter()
        .enumerate()
        .map(|(n, (title, payload))| ui::submit(&format!("{prefix}-{n}"), title, json!({key: payload})))
        .collect()
}

impl Surface {
    pub fn quick_replies(replies: Vec<Reply>) -> Result<Self, MessagingError> {
        within(replies.len() <= QUICK_REPLIES, "the number of quick replies")?;
        replies.iter().try_for_each(|r| within_chars(&r.title, QUICK_REPLY_TITLE, "a quick reply title"))?;
        let options: Vec<(&str, &str)> = replies.iter().map(|r| (r.title.as_str(), r.payload.as_str())).collect();
        Ok(ui::surface("", vec![ui::container("Row", "replies", answers("reply", "reply", &options))], json!({})))
    }

    pub fn buttons(text: impl AsRef<str>, buttons: Vec<Button>) -> Result<Self, MessagingError> {
        within(buttons.len() <= BUTTONS, "the number of buttons")?;
        let groups = ui::container("Row", "buttons", button_groups("button", &buttons));
        Ok(ui::surface(text.as_ref(), vec![groups], json!({})))
    }

    pub fn carousel(cards: Vec<Card>) -> Result<Self, MessagingError> {
        within(cards.len() <= CARDS, "the number of cards")?;
        cards.iter().try_for_each(check_card)?;
        let groups: Vec<Group> = cards.iter().enumerate().map(|(n, card)| card_group(n, card)).collect();
        Ok(ui::surface("", vec![ui::container("Row", "cards", groups)], json!({})))
    }

    pub fn receipt(invoice: &Invoice, payment: &Payment) -> Result<Self, MessagingError> {
        let (invoice, payment) = (&invoice.data, &payment.data);
        let money = |micros| {
            crate::money::from_micros(i64::from(micros), invoice.currency.as_ref())
                .map(|money| money.to_string())
                .map_err(|error| MessagingError::InvalidSurface(format!("receipt amount: {error:?}")))
        };
        let lines = [
            format!("Invoice {}", invoice.number.as_ref().map_or("", AsRef::as_ref)),
            format!("Date: {}", invoice.date),
            format!("Total: {}", money(invoice.total)?),
            format!("Paid: {} by {:?} on {}", money(payment.amount)?, payment.method, payment.date),
            format!("Balance due: {}", money(invoice.balance_due)?),
        ];
        let rows: Vec<Group> = lines.iter().enumerate().map(|(n, line)| ui::text(&format!("line-{n}"), line)).collect();
        Ok(ui::surface("Receipt", vec![ui::container("Column", "lines", rows)], json!({})))
    }
}

fn check_card(card: &Card) -> Result<(), MessagingError> {
    within_chars(&card.title, CARD_TEXT, "a card title")?;
    card.subtitle.iter().try_for_each(|subtitle| within_chars(subtitle, CARD_TEXT, "a card subtitle"))?;
    within(card.buttons.len() <= BUTTONS, "the number of buttons on a card")
}

fn button_groups(prefix: &str, buttons: &[Button]) -> Vec<Group> {
    let options: Vec<(&str, &str)> = buttons.iter().map(|b| (b.title.as_str(), b.payload.as_str())).collect();
    answers(prefix, "button", &options)
}

fn card_group(n: usize, card: &Card) -> Group {
    let texts = [Some(&card.title), card.subtitle.as_ref()].into_iter().flatten().enumerate();
    let texts = texts.map(|(m, text)| ui::text(&format!("card-{n}-text-{m}"), text));
    let buttons = button_groups(&format!("card-{n}-button"), &card.buttons);
    let body = ui::container("Column", &format!("card-{n}-body"), texts.chain(buttons).collect());
    let card_head: Value = json!({"id": format!("card-{n}"), "component": "Card", "child": format!("card-{n}-body")});
    std::iter::once(card_head).chain(body).collect()
}
