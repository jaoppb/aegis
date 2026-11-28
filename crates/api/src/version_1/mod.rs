//! Version 1 of the controller-agent API, served under the `/v1` path prefix.

mod enroll;
mod health;
mod nodes;
mod reconcile;
mod status;
mod sync;

pub use enroll::{ENROLLMENTS_PATH, EnrollRequest, EnrollResponse, SidecarVolume};
pub use health::{HEALTH_PATH, Health, Role, VERSION};
pub use nodes::{NODE_GONE, NODE_PATH, node_path};
pub use reconcile::{
    HOST_STATUS_PATH, MAX_REPLY_BYTES, RECONCILIATIONS_PATH, ReconcileRequest, StackState,
    StackStatus, StatusReport, host_status_path,
};
pub use status::{
    Agent, Confirmation, ControllerStatus, Counted, FLEET_PATH, HostStatus, InvalidCommit,
    RefusalReason, Refused, TargetStatus,
};
pub use sync::{HOST_SYNCS_PATH, HostSync, SYNCS_PATH, SyncRequest, SyncResponse, host_syncs_path};
