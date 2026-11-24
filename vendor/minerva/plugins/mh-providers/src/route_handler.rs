//! Record selected-route admission at Rig's handler boundary, before transport.
use super::{selection::SelectionControl, selection_hook::StepState, ProviderService};
use mh_sessions::{DispatchAdmission, RouteDispatch};
use rig_core::{
    effect::{family, EffectKind, HandlerDescriptor},
    error::{ErrorKind, ErrorReport},
    operation::Completion,
    serve::{adapters::ModelAdapter, Dispatch, Reply, Serve},
};
use std::sync::Arc;

pub(crate) struct RouteHandler {
    pub inner: ModelAdapter<Completion>,
    pub label: String,
    pub source: ProviderService,
    pub step: StepState,
    pub control: Arc<dyn SelectionControl>,
    pub run_id: String,
    pub history: super::rig_history::HistoryHook,
    pub cancellation: super::tools::Cancellation,
}
impl RouteHandler {
    async fn admit(&self, kind: &EffectKind, dispatch: &Dispatch) -> Result<(), String> {
        if self.cancellation.is_cancelled() {
            return Err("Run cancelled".into());
        }
        let step =
            self.step.lock().map_err(|e| e.to_string())?.clone().ok_or("No route was prepared for this dispatch")?;
        if self.label != step.route.label || self.label != step.selection.model {
            return Err("Prepared route does not match the invoked handler".into());
        }
        let EffectKind::Completion { request, .. } = kind else {
            return Err("Wrong effect family for route admission".into());
        };
        if request.model.as_deref() != Some(step.route.model.as_str())
            || request.options.reasoning != step.plan.typed
            || request.additional_params != step.plan.additional_params
            || request.max_tokens != Some(step.output_cap)
        {
            return Err("The dispatched request differs from its frozen route selection".into());
        }
        self.source.validate_route(&step.route).await?;
        if self.cancellation.is_cancelled() {
            return Err("Run cancelled".into());
        }
        super::selection_hook::validate_request(request, &step)?;
        self.history.selection_boundary()?;
        let record = RouteDispatch {
            run_id: self.run_id.clone(),
            effect_id: dispatch.id().as_u64().to_string(),
            turn: step.turn,
            selection: step.selection,
        };
        let changed = match self.control.record_dispatch(record.clone()).await? {
            DispatchAdmission::Duplicate => return Err("This model dispatch was already admitted".into()),
            DispatchAdmission::Recorded { changed } => changed,
        };
        if self.cancellation.is_cancelled() {
            return Err("Run cancelled".into());
        }
        self.source.validate_route(&step.route).await?;
        if self.cancellation.is_cancelled() {
            return Err("Run cancelled".into());
        }
        if changed {
            self.history.selection_notice(&record)?;
        }
        Ok(())
    }
}
impl Serve for RouteHandler {
    type Family = family::Completion;
    fn descriptor(&self) -> HandlerDescriptor {
        self.inner.descriptor()
    }
    fn check(&self, kind: &EffectKind) -> Vec<rig_core::completion::UnsupportedOption> {
        self.inner.check(kind)
    }
    async fn serve(&self, kind: EffectKind, dispatch: Dispatch) -> Reply {
        if let Err(error) = self.admit(&kind, &dispatch).await {
            return Reply::Outcome(Err(ErrorReport::new(ErrorKind::Denied, error)));
        }
        self.inner.serve(kind, dispatch).await
    }
}
