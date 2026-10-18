use sqlx::types::Json;

use super::{Queue, Result, Target, Waitlist, WaitlistEntry, WaitlistEntryId, WaitlistEntryRow};

impl Waitlist for Queue {
    async fn join(&self, entry: WaitlistEntry) -> Result<WaitlistEntryRow> {
        Ok(WaitlistEntryRow::new(WaitlistEntryId(uuid::Uuid::new_v4()), entry).insert(&self.pool).await?)
    }

    async fn waiting(&self, target: &Target) -> Result<Vec<WaitlistEntryRow>> {
        let query = "SELECT * FROM waitlist_entries WHERE target = $1 AND deleted_at IS NULL ORDER BY requested_at, created_at";
        Ok(sqlx::query_as(query).bind(Json(target)).fetch_all(&self.pool).await?)
    }
}
