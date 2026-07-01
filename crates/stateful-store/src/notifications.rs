use super::*;

impl Store {
    pub fn pending_notifications(
        &self,
        target_agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
    ) -> StoreResult<Vec<NotificationRecord>> {
        self.expire_stale()?;
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> StoreResult<Vec<NotificationRecord>> {
            let notifications = self.pending_notifications_after_in_transaction(
                target_agent_id.as_ref(),
                workspace_id.as_ref(),
                0,
            )?;

            for notification in &notifications {
                self.conn.execute(
                    "UPDATE notifications
                     SET status = 'delivered'
                     WHERE notification_id = ?1 AND status = 'pending'",
                    params![&notification.notification_id],
                )?;
            }

            self.conn.execute_batch("COMMIT")?;
            Ok(notifications)
        })();

        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }

        result
    }

    pub fn pending_notifications_after(
        &self,
        target_agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        after_sequence: u64,
    ) -> StoreResult<Vec<NotificationRecord>> {
        self.expire_stale()?;
        self.pending_notifications_after_in_transaction(
            target_agent_id.as_ref(),
            workspace_id.as_ref(),
            after_sequence,
        )
    }

    pub fn mark_notifications_delivered_through(
        &self,
        target_agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        sequence: u64,
    ) -> StoreResult<()> {
        if sequence == 0 {
            return Ok(());
        }
        let sequence = i64::try_from(sequence).unwrap_or(i64::MAX);
        self.conn.execute(
            "UPDATE notifications
             SET status = 'delivered'
             WHERE target_agent_id = ?1
                AND workspace_id = ?2
                AND status = 'pending'
                AND sequence <= ?3",
            params![target_agent_id.as_ref(), workspace_id.as_ref(), sequence],
        )?;
        Ok(())
    }

    fn pending_notifications_after_in_transaction(
        &self,
        target_agent_id: &str,
        workspace_id: &str,
        after_sequence: u64,
    ) -> StoreResult<Vec<NotificationRecord>> {
        let after_sequence = i64::try_from(after_sequence).unwrap_or(i64::MAX);
        let mut statement = self.conn.prepare(
            "SELECT
                notification_id,
                sequence,
                target_agent_id,
                workspace_id,
                kind,
                payload_json,
                status,
                created_at,
                expires_at
             FROM notifications
             WHERE target_agent_id = ?1
                AND workspace_id = ?2
                AND status = 'pending'
                AND sequence > ?3
             ORDER BY sequence ASC, rowid ASC",
        )?;
        let rows = statement.query_map(
            params![target_agent_id, workspace_id, after_sequence],
            notification_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub(crate) fn append_notification(
        &self,
        target_agent_id: &str,
        workspace_id: &str,
        kind: &str,
        payload: serde_json::Value,
    ) -> StoreResult<()> {
        let sequence = self.conn.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1
             FROM notifications
             WHERE target_agent_id = ?1 AND workspace_id = ?2",
            params![target_agent_id, workspace_id],
            |row| row.get::<_, u64>(0),
        )?;
        let now = now_timestamp();
        let expires_at = timestamp_after(&now, CLAIMABLE_RESERVATION_TTL_SECONDS)?;
        self.conn.execute(
            "INSERT INTO notifications (
                notification_id,
                sequence,
                target_agent_id,
                workspace_id,
                kind,
                payload_json,
                status,
                created_at,
                expires_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8)",
            params![
                Uuid::new_v4().to_string(),
                sequence,
                target_agent_id,
                workspace_id,
                kind,
                payload.to_string(),
                now,
                expires_at,
            ],
        )?;

        Ok(())
    }
}
