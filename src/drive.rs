use crate::{autoscan::create_payload, Atrain, Result, Error};
use bernard::SyncKind;
use futures::prelude::*;
use tracing::{info, warn};
use reqwest::Client;
use serde_json::json;
const CONCURRENCY: usize = 5;

impl Atrain {
    async fn sync_drive(&self, drive_id: &str) -> Result<()> {
        match self.bernard.sync_drive(drive_id).await {
            // Do not send a payload to Autoscan on a full scan
            Ok(SyncKind::Full) => (),
            Ok(SyncKind::Partial(changes)) => {
                let changed_paths = changes.paths().await?;
                let payload = create_payload(changed_paths);

                if !payload.is_empty() {
                    info!(%drive_id, "Sending payload to Autoscan: {:?}", payload);
                    let result = self.autoscan.send_payload(drive_id, &payload).await;
                    info!(%drive_id, "Result of send_payload: {:?}", result);
                    result?;
                }
            }
            Err(err) => {
                // Can ignore a Partial Change List as it should recover eventually.
                if !err.is_partial_change_list() {
                    self.send_telegram_notification(&format!("Drive ID: {} Synchronization error: {}", drive_id, err)).await?;
                    return Err(err.into());
                }

                warn!(
                    %drive_id,
                    error = %err,
                    "Encountered a Partial Change List error. Drive ID: {}, Error details: {}. This may be due to network issues or Google Drive API limitations.",
                    drive_id,
                    err
                );
                // 发送部分变更列表错误的通知
                self.send_telegram_notification(&format!("Drive ID: {} Encountered some errors in the change list: {}", drive_id, err)).await?;
            }
        }

        Ok(())
    }

    /// 发送 Telegram 通知的方法
    async fn send_telegram_notification(&self, message: &str) -> Result<()> {
        // 如果 Telegram 配置为空，直接返回
        let telegram_config = match &self.telegram_config {
            Some(config) => config,
            None => return Ok(()),
        };

          // 如果 bot_token 或 chat_id 为空，也直接返回
          if telegram_config.bot_token.is_empty() || telegram_config.chat_id.is_empty() {
            return Ok(());
        }

        let client = Client::new();
        let url = format!("https://api.telegram.org/bot{}/sendMessage", telegram_config.bot_token);

        let response = client.post(&url)
            .json(&json!({
                "chat_id": telegram_config.chat_id,
                "text": message,
            }))
            .send()
            .await
            .map_err(|e| Error::Telegram(format!("Failed to send Telegram message: {}", e)))?;

        if !response.status().is_success() {
            return Err(Error::Telegram(format!("Telegram API error: {}", response.status())));
        }

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self))]
    pub async fn sync(&self) -> Result<()> {
        // also fetch changes here and create+send response to Autoscan for each individual Drive.
        // https://stackoverflow.com/questions/51044467
        stream::iter(&self.drives)
            .map(|drive_id| self.sync_drive(drive_id))
            .buffer_unordered(CONCURRENCY)
            .try_collect()
            .await
    }

    pub async fn close(self) {
        self.bernard.close().await
    }
}
