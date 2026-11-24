pub mod conversation;
pub mod run_supervisor;
pub mod runs;
mod selection_control;
use cordis::{Context, Plugin};
pub use mh_plugin_manager::{context, engine, harness, plugins, providers, tools};
pub use mh_sessions::{sessions, transcript};
use plugins::PluginManager;
pub use run_supervisor::AgentLoopService;
use sessions::SessionService;
use std::{
    convert::Infallible,
    sync::{Arc, Weak},
    time::Duration,
};
fn error(value: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(value.to_string())
}
pub struct AgentLoopPlugin {
    pub plugins: Weak<PluginManager>,
    pub shutdown_timeout: Duration,
}
impl Plugin for AgentLoopPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = std::io::Error;
    fn prepare(&self, _: ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Self::ApplyError> {
        let sessions = ctx.try_service::<SessionService>().map_err(error)?;
        ctx.try_service::<crate::tools::ToolRegistry>().map_err(error)?;
        ctx.try_service::<crate::providers::ProviderService>().map_err(error)?;
        let service = Arc::new(AgentLoopService::new(sessions, self.plugins.clone(), self.shutdown_timeout));
        let owned = service.clone();
        let _cleanup = ctx
            .effect(move || async move {
                owned.shutdown().await;
                Ok::<_, std::io::Error>(())
            })
            .map_err(error)?;
        let _publication = ctx.provide(service).map_err(error)?;
        let _marker = ctx.provide(Arc::new(mh_plugin_manager::AgentLoopMarker)).map_err(error)?;
        Ok(())
    }
}
