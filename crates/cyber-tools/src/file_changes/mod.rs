//! Bounded host-owned file observation and durable observational hook dispatch.
mod native;
mod owner;
mod root;
mod scan;
mod state;
pub(crate) use owner::Observers;
