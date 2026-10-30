//! OAuth with any site, through `rmcp`'s OAuth client: the site's authorization server is found from its metadata
//! (RFC 9728, 8414), megh registers as a client (RFC 7591) and signs in with PKCE. The credentials a connection keeps
//! are rmcp's, which it refreshes.

use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationRequest, AuthorizationSession, CredentialStore, InMemoryCredentialStore,
    OAuthClientConfig, StateStore, StoredCredentials,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::connections::ConnectionError;
use super::encryption::Encryptor;

/// The sign-in page to send the browser to, and the client megh registered as.
pub struct Started {
    pub authorize_url: String,
    pub client_id: String,
}

/// Finds the authorization server of the site at `url`, registers with it for `redirect_uri` and builds the sign-in
/// URL; the PKCE state is kept in `states`.
pub async fn start(url: &str, redirect_uri: &str, states: impl StateStore + 'static) -> Result<Started, ConnectionError> {
    let mut manager = discovered(url).await?;
    manager.set_state_store(states);
    let request = AuthorizationRequest::new(redirect_uri).with_client_name("megh").with_application_type("web");
    let session = AuthorizationSession::new(manager, request).await.map_err(|(_, error)| failed(error))?;
    let (client_id, _) = session.auth_manager.get_credentials().await.map_err(failed)?;
    Ok(Started { authorize_url: session.auth_url, client_id })
}

/// Exchanges the callback's `code` for the site's credentials, checking `csrf` against `states`.
pub async fn finish(url: &str, client_id: &str, redirect_uri: &str, (code, csrf): (&str, &str), states: impl StateStore + 'static) -> Result<StoredCredentials, ConnectionError> {
    let mut manager = discovered(url).await?;
    manager.configure_client(OAuthClientConfig::new(client_id, redirect_uri)).map_err(failed)?;
    manager.set_state_store(states);
    let credentials = InMemoryCredentialStore::new();
    manager.set_credential_store(credentials.clone());
    manager.exchange_code_for_token(code, csrf).await.map_err(failed)?;
    credentials.load().await.map_err(failed)?.ok_or(ConnectionError::OAuth("the site gave no token".into()))
}

/// The access token of the site connection in `store`, refreshed at the site when it expires; its credentials are saved.
pub async fn access_token(url: &str, store: StoredGrant) -> Result<String, ConnectionError> {
    let mut manager = AuthorizationManager::new(url).await.map_err(failed)?;
    manager.set_credential_store(store);
    manager.initialize_from_store().await.map_err(failed)?;
    manager.get_access_token().await.map_err(failed)
}

async fn discovered(url: &str) -> Result<AuthorizationManager, ConnectionError> {
    let mut manager = AuthorizationManager::new(url).await.map_err(failed)?;
    let no_oauth = ConnectionError::Invalid("this site does not offer OAuth");
    let resolution = manager.resolve_metadata().await.ok().filter(|resolution| resolution.source.is_discovered()).ok_or(no_oauth)?;
    manager.set_metadata(resolution.metadata);
    Ok(manager)
}

fn failed(error: AuthError) -> ConnectionError {
    ConnectionError::OAuth(error.to_string())
}

/// rmcp's credentials of a site connection, kept encrypted in its row; a save also keeps the access token current.
#[derive(Clone)]
pub struct StoredGrant {
    pub pool: PgPool,
    pub encryptor: Option<Encryptor>,
    pub id: Uuid,
}

impl StoredGrant {
    fn seal(&self, plaintext: &str) -> Result<String, AuthError> {
        match &self.encryptor {
            Some(encryptor) => encryptor.encrypt(plaintext).map_err(|_| store_error("encryption failed")),
            None => Ok(plaintext.to_string()),
        }
    }

    fn open(&self, stored: &str) -> Result<String, AuthError> {
        match &self.encryptor {
            Some(encryptor) => encryptor.decrypt(stored).map_err(|_| store_error("decryption failed")),
            None => Ok(stored.to_string()),
        }
    }
}

fn store_error(reason: impl Into<String>) -> AuthError {
    AuthError::CredentialStoreError(reason.into())
}

#[async_trait::async_trait]
impl CredentialStore for StoredGrant {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let stored: Option<Option<String>> = sqlx::query_scalar("SELECT oauth FROM connected_accounts WHERE id = $1")
            .bind(self.id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| store_error(error.to_string()))?;
        let credentials = stored.flatten().map(|sealed| self.open(&sealed)).transpose()?;
        credentials.map(|json| serde_json::from_str(&json).map_err(|error| store_error(error.to_string()))).transpose()
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let json = serde_json::to_string(&credentials).map_err(|error| store_error(error.to_string()))?;
        let access = credentials.token_response.as_ref().map(|token| oauth2::TokenResponse::access_token(token).secret().clone());
        let access = access.map(|token| self.seal(&token)).transpose()?;
        sqlx::query("UPDATE connected_accounts SET oauth = $2, access_token = COALESCE($3, access_token), updated_at = NOW() WHERE id = $1")
            .bind(self.id)
            .bind(self.seal(&json)?)
            .bind(access)
            .execute(&self.pool)
            .await
            .map_err(|error| store_error(error.to_string()))?;
        Ok(())
    }

    async fn clear(&self) -> Result<(), AuthError> {
        sqlx::query("UPDATE connected_accounts SET oauth = NULL, updated_at = NOW() WHERE id = $1")
            .bind(self.id)
            .execute(&self.pool)
            .await
            .map_err(|error| store_error(error.to_string()))?;
        Ok(())
    }
}
