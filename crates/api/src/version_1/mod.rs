//! Version 1 of the controller-agent API, served under the `/v1` path prefix.

mod enroll;
mod health;
mod reconcile;
mod status;

pub use enroll::{ENROLL_PATH, EnrollRequest, EnrollResponse, SidecarVolume};
pub use health::{HEALTH_PATH, Health, Role, VERSION};
pub use reconcile::{
    RECONCILE_PATH, ReconcileRequest, STATUS_PATH, StackState, StackStatus, StatusReport,
};
pub use status::{Agent, Confirmation, ControllerStatus, HostStatus, InvalidCommit, TargetStatus};
