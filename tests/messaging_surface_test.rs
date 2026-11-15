//! Canned surfaces: a fluent builder that produces a2ui 1.0 messages.

use std::collections::HashSet;

use megh::messaging::{Scale, Surface};
use serde_json::{Value, json};

fn components(surface: Surface) -> Vec<Value> {
    surface.messages("s1")[1]["updateComponents"]["components"].as_array().unwrap().clone()
}

fn of<'a>(all: &'a [Value], component: &str) -> Vec<&'a Value> {
    all.iter().filter(|c| c["component"] == component).collect()
}

fn by_id<'a>(all: &'a [Value], id: &Value) -> &'a Value {
    all.iter().find(|c| &c["id"] == id).unwrap()
}

fn texts(all: &[Value], ids: &Value) -> Vec<String> {
    ids.as_array().unwrap().iter().map(|id| by_id(all, id)["text"].as_str().unwrap().to_string()).collect()
}

fn children(component: &Value) -> Vec<Value> {
    component["children"].as_array().unwrap().clone()
}

fn root(all: &[Value]) -> &Value {
    by_id(all, &json!("root"))
}

#[test]
fn the_messages_are_a2ui_one_point_zero_create_then_update() {
    let messages = Surface::new().text("Hello").messages("s1");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["version"], "1.0");
    assert_eq!(messages[0]["createSurface"]["surfaceId"], "s1");
    assert_eq!(messages[0]["createSurface"]["sendDataModel"], true);
    assert_eq!(messages[1]["version"], "1.0");
    assert_eq!(messages[1]["updateComponents"]["surfaceId"], "s1");
}

#[test]
fn the_top_level_is_a_column_named_root() {
    let all = components(Surface::new().text("Hello"));
    assert_eq!(root(&all)["component"], "Column");
    assert_eq!(texts(&all, &root(&all)["children"]), ["Hello"]);
}

#[test]
fn plain_text_is_an_a2ui_text() {
    let all = components(Surface::new().text("Hello").text("again"));
    assert_eq!(texts(&all, &root(&all)["children"]), ["Hello", "again"]);
    assert_eq!(of(&all, "Text").len(), 2);
}

#[test]
fn a_button_sends_its_event_and_shows_its_title() {
    let all = components(Surface::new().button("Yes", "approve"));
    let button = of(&all, "Button")[0];
    assert_eq!(button["action"]["event"]["name"], "approve");
    assert_eq!(by_id(&all, &button["child"])["text"], "Yes");
    assert_eq!(root(&all)["children"], json!([button["id"]]));
}

#[test]
fn one_button_alone_works() {
    let all = components(Surface::new().text("Add to calendar?").button("Yes", "approve"));
    assert_eq!(of(&all, "Button").len(), 1);
    assert_eq!(children(root(&all)).len(), 2);
}

#[test]
fn a_field_is_a_row_of_its_name_and_its_value() {
    let all = components(Surface::new().field("Service", "Haircut"));
    let row = by_id(&all, &root(&all)["children"][0]);
    assert_eq!(row["component"], "Row");
    assert_eq!(texts(&all, &row["children"]), ["Service", "Haircut"]);
}

#[test]
fn a_row_holds_the_components_built_in_it() {
    let all = components(Surface::new().row(|r| r.button("Yes", "approve").button("No", "reject")));
    let row = by_id(&all, &root(&all)["children"][0]);
    assert_eq!(row["component"], "Row");
    let events = children(row).iter().map(|id| by_id(&all, id)["action"]["event"]["name"].clone()).collect::<Vec<_>>();
    assert_eq!(events, [json!("approve"), json!("reject")]);
}

#[test]
fn a_column_nests_inside_the_top_level() {
    let all = components(Surface::new().column(|c| c.text("Haircut").text("Dr. Rao")));
    let column = by_id(&all, &root(&all)["children"][0]);
    assert_eq!(column["component"], "Column");
    assert_eq!(texts(&all, &column["children"]), ["Haircut", "Dr. Rao"]);
}

#[test]
fn each_card_is_a_card_with_its_own_content() {
    let all = components(
        Surface::new()
            .card(|c| c.text("Haircut, 10:30").button("Pick", "pick:1"))
            .card(|c| c.text("Haircut, 11:30").button("Pick", "pick:2")),
    );
    let cards = of(&all, "Card");
    assert_eq!(cards.len(), 2);
    assert_eq!(children(root(&all)), cards.iter().map(|c| c["id"].clone()).collect::<Vec<_>>());
    let inside = by_id(&all, &cards[1]["child"]);
    assert_eq!(by_id(&all, &inside["children"][0])["text"], "Haircut, 11:30");
    assert_eq!(by_id(&all, &inside["children"][1])["action"]["event"]["name"], "pick:2");
}

#[test]
fn a_carousel_is_a_horizontal_list_of_cards() {
    let slots = ["10:30", "11:30", "12:30"];
    let all = components(Surface::new().carousel(|s| slots.iter().fold(s, |s, slot| s.card(|c| c.text(slot)))));
    let list = of(&all, "List")[0];
    assert_eq!(list["direction"], "horizontal");
    assert_eq!(children(list).iter().map(|id| by_id(&all, id)["component"].clone()).collect::<Vec<_>>(), vec![json!("Card"); 3]);
}

#[test]
fn input_choice_and_calendar_are_bound_to_the_data_model() {
    let all = components(Surface::new().input("Your name", "name").choice("slot", &[("10:30", "a"), ("11:30", "b")]).calendar("day"));
    assert_eq!(of(&all, "TextField")[0]["value"], json!({"@path": "/name"}));
    assert_eq!(of(&all, "TextField")[0]["label"], "Your name");
    let picker = of(&all, "ChoicePicker")[0];
    assert_eq!(picker["value"], json!({"@path": "/slot"}));
    assert_eq!(picker["options"].as_array().unwrap().iter().map(|o| o["value"].clone()).collect::<Vec<_>>(), [json!("a"), json!("b")]);
    let date = of(&all, "DateTimeInput")[0];
    assert_eq!(date["value"], json!({"@path": "/day"}));
    assert_eq!(date["enableDate"], true);
}

#[test]
fn approval_shows_the_question_the_fields_and_approve_and_reject() {
    let all = components(Surface::approval("Book it?", &[("Service", "Haircut"), ("Provider", "Dr. Rao")]));
    let events = of(&all, "Button").iter().map(|b| b["action"]["event"]["name"].clone()).collect::<Vec<_>>();
    assert_eq!(events, [json!("approve"), json!("reject")]);
    assert!(of(&all, "Text").iter().any(|t| t["text"] == "Book it?"));
    assert_eq!(of(&all, "Row").iter().filter(|r| by_id(&all, &r["children"][0])["text"] == "Service").count(), 1);
}

#[test]
fn login_and_logout_open_the_given_url() {
    let all = components(Surface::login("https://app.example.com/login"));
    assert_eq!(of(&all, "Button")[0]["action"]["functionCall"]["@call"], "openUrl");
    assert_eq!(of(&all, "Button")[0]["action"]["functionCall"]["args"]["url"], "https://app.example.com/login");
    let all = components(Surface::logout("https://app.example.com/logout"));
    assert_eq!(of(&all, "Button")[0]["action"]["functionCall"]["args"]["url"], "https://app.example.com/logout");
}

#[test]
fn selection_offers_the_options_and_counted_adds_a_number_field() {
    let plain = components(Surface::selection("Pick", &[("Cut", "1"), ("Dye", "2")], false));
    assert_eq!(of(&plain, "ChoicePicker")[0]["options"].as_array().unwrap().len(), 2);
    assert!(of(&plain, "TextField").is_empty());
    let counted = components(Surface::selection("Pick", &[("Cut", "1")], true));
    assert_eq!(of(&counted, "TextField")[0]["variant"], "number");
}

#[test]
fn times_date_range_and_intake_are_the_matching_inputs() {
    assert_eq!(of(&components(Surface::times("When?", &[("10:30", "a")])), "ChoicePicker").len(), 1);
    assert_eq!(of(&components(Surface::date("Day?")), "DateTimeInput").len(), 1);
    assert_eq!(of(&components(Surface::range("Days?")), "DateTimeInput").len(), 2);
    assert!(of(&components(Surface::intake()), "TextField").len() >= 3);
}

#[test]
fn feedback_is_stars_or_words_with_a_comment() {
    let stars = components(Surface::feedback(Scale::Stars(5)));
    let slider = of(&stars, "Slider")[0];
    assert_eq!((slider["min"].clone(), slider["max"].clone()), (json!(1), json!(5)));
    assert_eq!(of(&stars, "TextField").len(), 1);
    let words = components(Surface::feedback(Scale::Words(&["good", "ok", "poor"])));
    assert_eq!(of(&words, "ChoicePicker")[0]["options"].as_array().unwrap().len(), 3);
}

#[test]
fn ids_are_unique_and_every_reference_exists() {
    let surface = Surface::new()
        .field("a", "b")
        .row(|r| r.button("Yes", "y"))
        .card(|c| c.column(|c| c.text("t")))
        .carousel(|s| s.card(|c| c.text("x")));
    let all = components(surface);
    let ids = all.iter().map(|c| c["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(ids.len(), ids.iter().collect::<HashSet<_>>().len());
    let known = ids.iter().cloned().collect::<HashSet<_>>();
    let mut refs = all.iter().flat_map(|c| {
        let list = c["children"].as_array().cloned().unwrap_or_default();
        list.into_iter().chain(c["child"].as_str().map(|s| json!(s))).map(|v| v.as_str().unwrap().to_string()).collect::<Vec<_>>()
    });
    assert!(refs.all(|r| known.contains(&r)));
}
