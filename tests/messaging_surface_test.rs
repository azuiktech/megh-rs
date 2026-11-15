//! Canned surfaces: a fluent builder that produces a2ui 1.0 messages.

use std::collections::HashSet;

use megh::messaging::{Scale, Schema, Surface};
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

#[derive(serde::Serialize)]
struct Booking {
    service: &'static str,
    provider: &'static str,
    time: &'static str,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
enum Tier {
    Basic,
    Gold,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
struct Town {
    city: String,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
struct Intake {
    name: String,
    age: u32,
    tier: Tier,
    born: chrono::NaiveDate,
    agreed: bool,
    town: Town,
    secret: String,
}

fn bound_to(all: &[Value]) -> Vec<Value> {
    all.iter().filter(|c| c["value"].get("@path").is_some()).map(|c| c["value"]["@path"].clone()).collect()
}

#[test]
fn title_and_subtitle_lead_the_card_and_both_are_optional() {
    let all = components(Surface::new().card(|c| c.title("Booking").subtitle("Tomorrow").text("body")));
    let column = of(&all, "Column").into_iter().find(|c| c["id"] != "root").unwrap();
    let shown = children(column).iter().map(|id| by_id(&all, id).clone()).collect::<Vec<_>>();
    assert!(shown[0]["text"].as_str().unwrap().contains("Booking"));
    assert_ne!(shown[0]["variant"], "caption");
    assert_eq!((shown[1]["text"].clone(), shown[1]["variant"].clone()), (json!("Tomorrow"), json!("caption")));
    assert_eq!(shown[2]["text"], "body");
    assert_eq!(of(&components(Surface::new().card(|c| c.text("body"))), "Text").len(), 1);
}

#[test]
fn fields_show_each_property_of_a_value_in_its_order() {
    let booking = Booking { service: "Haircut", provider: "Dr. Rao", time: "10:30" };
    let all = components(Surface::new().fields(&booking).row(|r| r.button("Yes", "approve").button("No", "reject")));
    let rows = children(root(&all)).iter().map(|id| by_id(&all, id).clone()).filter(|c| c["component"] == "Row").collect::<Vec<_>>();
    let shown = rows.iter().filter(|r| children(r).len() == 2 && by_id(&all, &r["children"][0])["component"] == "Text");
    assert_eq!(shown.map(|r| texts(&all, &r["children"])).collect::<Vec<_>>(), [["service", "Haircut"], ["provider", "Dr. Rao"], ["time", "10:30"]]);
}

#[test]
fn inputs_follow_the_schema_of_a_type_in_its_order() {
    let all = components(Surface::new().inputs(Schema::of::<Intake>()));
    assert_eq!(bound_to(&all), [json!("/name"), json!("/age"), json!("/tier"), json!("/born"), json!("/agreed"), json!("/town/city"), json!("/secret")]);
    let at = |path: &str| all.iter().find(|c| c["value"]["@path"] == path).unwrap().clone();
    assert_eq!((at("/name")["component"].clone(), at("/name")["label"].clone()), (json!("TextField"), json!("name")));
    assert_eq!((at("/age")["component"].clone(), at("/age")["variant"].clone()), (json!("TextField"), json!("number")));
    assert_eq!(at("/tier")["component"], "ChoicePicker");
    assert_eq!(at("/tier")["options"].as_array().unwrap().iter().map(|o| o["value"].clone()).collect::<Vec<_>>(), [json!("Basic"), json!("Gold")]);
    assert_eq!(at("/born")["component"], "DateTimeInput");
    assert_eq!(at("/agreed")["component"], "CheckBox");
    assert_eq!(at("/town/city")["component"], "TextField");
}

#[test]
fn the_caller_drops_fields_a_form_should_not_show() {
    let all = components(Surface::new().inputs(Schema::of::<Intake>().without(&["secret", "town"])));
    assert_eq!(bound_to(&all), [json!("/name"), json!("/age"), json!("/tier"), json!("/born"), json!("/agreed")]);
}

#[test]
fn any_json_schema_makes_inputs() {
    let schema = json!({"type": "object", "properties": {"email": {"type": "string"}, "count": {"type": "integer"}}});
    let all = components(Surface::new().inputs(Schema::from(schema)));
    assert_eq!(bound_to(&all), [json!("/email"), json!("/count")]);
}

#[test]
fn filled_data_is_the_data_model_the_inputs_show() {
    let schema = json!({"type": "object", "properties": {"email": {"type": "string"}}});
    let messages = Surface::new().inputs(Schema::from(schema)).filled(&json!({"email": "ada@example.com"})).messages("s1");
    assert_eq!(messages[0]["createSurface"]["dataModel"], json!({"email": "ada@example.com"}));
}

#[test]
fn a_button_carries_every_input_of_its_surface_in_its_context() {
    let surface = Surface::new().card(|c| c.inputs(Schema::of::<Intake>().without(&["secret"]))).row(|r| r.button("Send", "submit"));
    let all = components(surface);
    let context = of(&all, "Button")[0]["action"]["event"]["context"].clone();
    let keys = ["name", "age", "tier", "born", "agreed", "town"];
    assert_eq!(context, Value::Object(keys.iter().map(|k| (k.to_string(), json!({"@path": format!("/{k}")}))).collect()));
}

#[test]
fn labels_are_strings_shown_with_the_message() {
    let all = components(Surface::new().text("Hi").labels(&["booking", "urgent"]));
    let first = by_id(&all, &root(&all)["children"][0]);
    assert_eq!(first["component"], "Row");
    assert_eq!(texts(&all, &first["children"]), ["booking", "urgent"]);
    assert_eq!(texts(&all, &json!([root(&all)["children"][1].clone()])), ["Hi"]);
}

#[test]
fn intake_is_the_client_schema_without_server_fields() {
    let paths = bound_to(&components(Surface::intake()));
    assert!(paths.contains(&json!("/first_name")) && paths.contains(&json!("/email")));
    assert!(!paths.contains(&json!("/tenant_id")) && !paths.iter().any(|p| p.as_str().unwrap().starts_with("/attributes")));
}

#[test]
fn every_component_id_is_an_identifier() {
    let surface = Surface::new().row(|r| r.button("Yes", "y")).card(|c| c.column(|c| c.text("t"))).carousel(|s| s.card(|c| c.text("x")));
    let identifier = |id: &str| id.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') && id.chars().all(|c| c.is_alphanumeric() || c == '_');
    let all = components(surface);
    let bad = all.iter().map(|c| c["id"].as_str().unwrap().to_string()).filter(|id| !identifier(id)).collect::<Vec<_>>();
    assert!(bad.is_empty(), "{bad:?}");
}
