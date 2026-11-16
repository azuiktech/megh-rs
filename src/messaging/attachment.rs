//! Attachments: files a client and a member send each other, kept in Postgres and referenced as a model's file data.

use serde::{Deserialize, Serialize};

/// A file itself: its name, media type and bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct File {
    pub name: String,
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

/// A file as a model takes it (`fileData`): where it is, its media type and its name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "axum", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct FileData {
    pub file_uri: String,
    pub mime_type: String,
    pub display_name: String,
}

#[cfg(all(feature = "axum", feature = "postgres"))]
pub(super) use stored::routes;

#[cfg(all(feature = "axum", feature = "postgres"))]
mod stored {
    use std::collections::HashMap;

    use axum::extract::{Path, Query, State};
    use axum::http::header::{CONTENT_DISPOSITION, CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
    use axum::response::{IntoResponse, Response};
    use axum::{Extension, Json};
    use axum_params::{Params, UploadFile};
    use futures_util::{StreamExt, TryStreamExt, stream};
    use serde::Deserialize;
    use tokio::io::AsyncReadExt;
    use tower_http::limit::RequestBodyLimitLayer;
    use utoipa_axum::router::OpenApiRouter;
    use utoipa_axum::routes;
    use uuid::Uuid;

    use super::{File, FileData};
    use crate::auth::AccessToken;
    use crate::auth::rest::AppContext;
    use crate::event::Broker;
    use crate::messaging::message::{Messaging, MessagingError, Party, pair};

    #[derive(sqlx::FromRow)]
    struct Stored {
        name: String,
        mime_type: String,
        bytes: Vec<u8>,
    }

    impl From<Stored> for File {
        fn from(stored: Stored) -> Self {
            Self { name: stored.name, mime_type: stored.mime_type, bytes: stored.bytes }
        }
    }

    impl Messaging {
        /// Keeps the file between one side of a client and a member and the other; answers it as file data.
        pub async fn attach(&self, file: File, from: Party, to: Party) -> Result<FileData, MessagingError> {
            let (client, member, _) = pair(from, to)?;
            let id = Uuid::now_v7();
            sqlx::query("INSERT INTO attachments (tenant_id, id, client_id, member_id, name, mime_type, bytes) VALUES ($1, $2, $3, $4, $5, $6, $7)")
                .bind(self.tenant_id)
                .bind(id)
                .bind(client)
                .bind(member)
                .bind(&file.name)
                .bind(&file.mime_type)
                .bind(&file.bytes)
                .execute(&self.pool)
                .await?;
            Ok(FileData { file_uri: format!("{}/tenants/{}/attachments/{id}", self.files_at, self.tenant_id), mime_type: file.mime_type, display_name: file.name })
        }

        /// The file the data refers to, within this tenant.
        pub async fn file(&self, data: &FileData) -> Result<File, MessagingError> {
            let id = data.file_uri.rsplit('/').next().and_then(|id| id.parse::<Uuid>().ok()).ok_or(MessagingError::NotFound)?;
            let stored: Option<Stored> = sqlx::query_as("SELECT name, mime_type, bytes FROM attachments WHERE tenant_id = $1 AND id = $2")
                .bind(self.tenant_id)
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
            stored.map(File::from).ok_or(MessagingError::NotFound)
        }
    }

    /// Where the routes are mounted, for the `fileUri` an upload answers.
    #[derive(Clone)]
    struct FilesAt(String);

    /// The upload route, refusing a body over `cap` bytes, and the download route.
    pub fn routes(cap: usize, base: &str) -> OpenApiRouter<AppContext> {
        let uploads = OpenApiRouter::new().routes(routes!(upload)).layer(RequestBodyLimitLayer::new(cap));
        uploads.routes(routes!(download)).layer(Extension(FilesAt(base.trim_end_matches('/').to_string())))
    }

    #[derive(Deserialize)]
    struct Uploaded {
        files: Vec<UploadFile>,
    }

    async fn read(upload: UploadFile) -> Result<File, MessagingError> {
        let mut bytes = Vec::new();
        upload.open().await?.read_to_end(&mut bytes).await?;
        Ok(File { name: upload.name, mime_type: upload.content_type, bytes })
    }

    /// Keeps the files the signed-in client attaches for the member; answers them as file data.
    #[utoipa::path(
        post,
        path = "/tenants/{tenant_id}/members/{member_id}/attachments",
        tag = "messages",
        params(("tenant_id" = Uuid, Path), ("member_id" = Uuid, Path)),
        request_body(content_type = "multipart/form-data", description = "`files[]`: the files"),
        responses((status = 200, body = [FileData]), (status = 413, description = "Over the size cap")),
    )]
    async fn upload(
        State(ctx): State<AppContext>,
        Extension(broker): Extension<Broker>,
        Extension(token): Extension<AccessToken>,
        Extension(FilesAt(base)): Extension<FilesAt>,
        Path((tenant_id, member_id)): Path<(Uuid, Uuid)>,
        Params(uploaded, _kept): Params<Uploaded>,
    ) -> Result<Json<Vec<FileData>>, MessagingError> {
        let messaging = Messaging::new(ctx.pool, broker, tenant_id).with_files_at(&base);
        let (from, to) = (Party::Client(token.sub), Party::Member(member_id));
        let files = stream::iter(uploaded.files).then(read).and_then(|file| messaging.attach(file, from, to)).try_collect().await?;
        Ok(Json(files))
    }

    /// The file, to the signed-in client of its pair: shown in place, or saved under its name with `?download`.
    #[utoipa::path(
        get,
        path = "/tenants/{tenant_id}/attachments/{id}",
        tag = "messages",
        params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path), ("download" = Option<String>, Query)),
        responses((status = 200, description = "The file's bytes"), (status = 404)),
    )]
    async fn download(
        State(ctx): State<AppContext>,
        Extension(token): Extension<AccessToken>,
        Path((tenant_id, id)): Path<(Uuid, Uuid)>,
        Query(query): Query<HashMap<String, String>>,
    ) -> Result<Response, MessagingError> {
        let stored: Option<Stored> = sqlx::query_as("SELECT name, mime_type, bytes FROM attachments WHERE tenant_id = $1 AND id = $2 AND client_id = $3")
            .bind(tenant_id)
            .bind(id)
            .bind(token.sub)
            .fetch_optional(&ctx.pool)
            .await?;
        let file = stored.ok_or(MessagingError::NotFound)?;
        let disposition = match query.contains_key("download") {
            true => format!("attachment; filename=\"{}\"", header_safe(&file.name)),
            false => "inline".to_string(),
        };
        let headers = [(CONTENT_TYPE, file.mime_type), (CONTENT_DISPOSITION, disposition), (CONTENT_SECURITY_POLICY, "sandbox".to_string()), (X_CONTENT_TYPE_OPTIONS, "nosniff".to_string())];
        Ok((headers, file.bytes).into_response())
    }

    /// The name as a header carries it: printable ASCII, with anything else and quotes as `_`.
    fn header_safe(name: &str) -> String {
        name.chars().map(|c| match c {
            '"' => '_',
            ' ' => c,
            c if c.is_ascii_graphic() => c,
            _ => '_',
        }).collect()
    }
}
