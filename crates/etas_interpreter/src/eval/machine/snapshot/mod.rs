mod call_target;
mod continuation;
mod frame;
mod machine_frame;
mod model;
mod validator;
mod value;
mod value_capture;
mod value_clone;
mod value_compare;
mod value_membership;
mod value_restore;

pub(crate) use frame::RestoreContext;
pub(crate) use validator::SnapshotValidator;
