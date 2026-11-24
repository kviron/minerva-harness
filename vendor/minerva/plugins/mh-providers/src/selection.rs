//! The coordinator owns durable intent; providers only consume its typed boundary.
use mh_sessions::{DispatchAdmission, RouteDispatch, RouteSelection};
use std::{future::Future, pin::Pin};

pub type SelectionFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

pub trait SelectionControl: Send + Sync {
    fn selection(&self) -> SelectionFuture<'_, RouteSelection>;
    fn record_dispatch(&self, dispatch: RouteDispatch) -> SelectionFuture<'_, DispatchAdmission>;
}
