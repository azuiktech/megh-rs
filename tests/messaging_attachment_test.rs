//! Attachments (PRD 11.2): `Messaging::attach` and `file` on Postgres, and the browser's upload and download routes.

#[macro_use]
mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, Response, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::AccessToken;
use megh::auth::rest::AppContext;
use megh::messaging::{File, FileData, Messaging, MessagingError, Party};
use megh::{Broker, Grant, authorizer};
use megh::Db;
use tower::ServiceExt;
use uuid::Uuid;

const BASE: &str = "https://app.example.com/api/v1";

async fn tenant(pool: &Db, name: &str) -> Uuid {
    Db::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id").bind(name).fetch_one(pool).await.unwrap()
}

fn photo() -> File {
    File { name: "a.png".into(), mime_type: "image/png".into(), bytes: vec![137, 80, 78, 71, 1, 2, 3] }
}

dual!(an_attached_file_is_referenced_as_file_data_and_read_back);
async fn an_attached_file_is_referenced_as_file_data_and_read_back(pool: Db) {
    let salon = tenant(&pool, "Salon").await;
    let messaging = Messaging::new(pool.clone(), Broker::new(), salon).with_files_at(BASE);

    let data = messaging.attach(photo(), Party::Member(Uuid::now_v7()), Party::Client(Uuid::now_v7())).await.unwrap();

    assert!(data.file_uri.starts_with(&format!("{BASE}/tenants/{salon}/attachments/")));
    assert_eq!((data.mime_type.as_str(), data.display_name.as_str()), ("image/png", "a.png"));
    assert_eq!(messaging.file(&data).await.unwrap(), photo());
}

dual!(files_go_between_a_client_and_a_member_and_stay_in_their_tenant);
async fn files_go_between_a_client_and_a_member_and_stay_in_their_tenant(pool: Db) {
    let messaging = Messaging::new(pool.clone(), Broker::new(), tenant(&pool, "Salon").await).with_files_at(BASE);
    let other = Messaging::new(pool.clone(), Broker::new(), tenant(&pool, "Spa").await).with_files_at(BASE);

    let two_members = messaging.attach(photo(), Party::Member(Uuid::now_v7()), Party::Member(Uuid::now_v7())).await;
    let data = messaging.attach(photo(), Party::Client(Uuid::now_v7()), Party::Member(Uuid::now_v7())).await.unwrap();

    assert!(matches!(two_members, Err(MessagingError::NotAPair)));
    assert!(other.file(&data).await.is_err());
}

fn app(pool: Db, tenant: Uuid, client: Uuid, cap: usize) -> Router {
    let (api, _) = megh::messaging::router(cap, BASE).split_for_parts();
    let grants = [format!("tenants:read:{tenant}"), "members:*".into(), "messages:*".into(), "events:*".into(), "attachments:*".into()].map(Grant::new).to_vec();
    let token = AccessToken { sub: client, iss: "megh".into(), aud: vec![tenant.to_string()], exp: u64::MAX, iat: 0, client_id: "megh".into(), scope: String::new() };
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants))
        .layer(Extension(token))
        .layer(Extension(Broker::new()))
        .with_state(AppContext { pool })
}

fn multipart(files: &[File]) -> (String, Vec<u8>) {
    let boundary = "megh-boundary";
    let part = |file: &File| {
        let head = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"files[]\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n", file.name, file.mime_type);
        [head.into_bytes(), file.bytes.clone(), b"\r\n".to_vec()].concat()
    };
    let body = [files.iter().flat_map(part).collect::<Vec<_>>(), format!("--{boundary}--\r\n").into_bytes()].concat();
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn upload(app: &Router, uri: &str, files: &[File]) -> Response<Body> {
    let (kind, body) = multipart(files);
    let request = Request::builder().method("POST").uri(uri).header("content-type", kind).header("content-length", body.len());
    app.clone().oneshot(request.body(Body::from(body)).unwrap()).await.unwrap()
}

async fn get(app: &Router, uri: &str) -> Response<Body> {
    app.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap()
}

fn header(response: &Response<Body>, name: &str) -> String {
    response.headers()[name].to_str().unwrap().to_string()
}

dual!(the_browser_uploads_files_and_downloads_them_inline_or_as_a_download);
async fn the_browser_uploads_files_and_downloads_them_inline_or_as_a_download(pool: Db) {
    let (salon, agent, ann) = (tenant(&pool, "Salon").await, Uuid::now_v7(), Uuid::now_v7());
    let app = app(pool.clone(), salon, ann, 1 << 20);
    let notes = File { name: "notes.pdf".into(), mime_type: "application/pdf".into(), bytes: b"%PDF-1.7".to_vec() };

    let uploaded = upload(&app, &format!("/api/v1/tenants/{salon}/members/{agent}/attachments"), &[photo(), notes]).await;

    assert_eq!(uploaded.status(), StatusCode::OK);
    let files: Vec<FileData> = serde_json::from_slice(&to_bytes(uploaded.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(files.iter().map(|f| (f.display_name.as_str(), f.mime_type.as_str())).collect::<Vec<_>>(), [("a.png", "image/png"), ("notes.pdf", "application/pdf")]);
    let path = files[0].file_uri.strip_prefix("https://app.example.com").unwrap().to_string();
    let inline = get(&app, &path).await;
    assert_eq!((inline.status(), header(&inline, "content-type"), header(&inline, "content-disposition")), (StatusCode::OK, "image/png".into(), "inline".into()));
    assert_eq!(to_bytes(inline.into_body(), usize::MAX).await.unwrap().to_vec(), photo().bytes);
    let download = get(&app, &format!("{path}?download")).await;
    assert_eq!(header(&download, "content-disposition"), "attachment; filename=\"a.png\"");
}

dual!(only_the_pair_s_client_reads_a_file);
async fn only_the_pair_s_client_reads_a_file(pool: Db) {
    let salon = tenant(&pool, "Salon").await;
    let messaging = Messaging::new(pool.clone(), Broker::new(), salon).with_files_at(BASE);
    let ann = Uuid::now_v7();
    let data = messaging.attach(photo(), Party::Member(Uuid::now_v7()), Party::Client(ann)).await.unwrap();
    let path = data.file_uri.strip_prefix("https://app.example.com").unwrap().to_string();

    let theirs = get(&app(pool.clone(), salon, ann, 1 << 20), &path).await.status();
    let stranger = get(&app(pool.clone(), salon, Uuid::now_v7(), 1 << 20), &path).await.status();

    assert_eq!((theirs, stranger), (StatusCode::OK, StatusCode::NOT_FOUND));
}

dual!(an_upload_over_the_cap_is_refused);
async fn an_upload_over_the_cap_is_refused(pool: Db) {
    let (salon, agent) = (tenant(&pool, "Salon").await, Uuid::now_v7());
    let app = app(pool.clone(), salon, Uuid::now_v7(), 64);
    let big = File { name: "big.bin".into(), mime_type: "application/octet-stream".into(), bytes: vec![0; 1024] };

    let refused = upload(&app, &format!("/api/v1/tenants/{salon}/members/{agent}/attachments"), &[big]).await;

    assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let stored: i64 = Db::query_scalar("SELECT count(*) FROM attachments").fetch_one(&pool).await.unwrap();
    assert_eq!(stored, 0);
}
