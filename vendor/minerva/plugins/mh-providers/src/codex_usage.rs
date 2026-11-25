use super::{Credential, Service};
use rig_core::client::signed_in::CredentialSource;
impl Service {
    pub(crate) async fn subscription_usage(&self) -> Result<Vec<crate::usage::UsageWindow>, &'static str> {
        let generation = self.auth_generation();
        let credential = Credential {
            auth: self.auth.clone(),
            generation: self.generation.clone(),
            expected: generation,
            state: self.state.clone(),
        };
        let work = async {
            let context = credential.auth_context(&self.http).await.map_err(|_| "auth")?;
            let payload = crate::usage::get(
                "https://chatgpt.com/backend-api/wham/usage",
                context.api_key.expose(),
                context.account_id.as_deref(),
            )
            .await?;
            if generation != self.auth_generation() {
                return Err("auth");
            }
            crate::usage::parse_codex(&payload)
        };
        tokio::time::timeout(crate::usage::REQUEST_TIMEOUT, work).await.map_err(|_| "timeout")?
    }
}
