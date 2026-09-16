mod call_target;
#[cfg(test)]
mod capture_alias_tests;
mod capture_identity;
mod continuation;
mod frame;
mod machine_frame;
mod message;
#[cfg(test)]
mod message_tests;
mod model;
mod validator;
mod value;
mod value_capture;
mod value_compare;
mod value_membership;
mod value_restore;

pub(crate) use frame::RestoreContext;
pub(crate) use validator::SnapshotValidator;
