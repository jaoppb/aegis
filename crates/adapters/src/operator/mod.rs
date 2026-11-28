//! What the `aegis` command line shows an operator.
//!
//! The text and the verdict of `validate`, `status`, `diff` and `sync` are built from the desired
//! state of a checkout and the answers of the controller. Nothing here does IO except
//! [`load_checkout`], so every command is tested on values.

mod checkout;
mod diff;
mod page;
mod stacks;
mod status;
mod sync;
mod validate;

pub use checkout::{CheckoutError, load_checkout};
pub use diff::diff;
pub use page::{Finding, Rendered};
pub use status::status;
pub use sync::sync;
pub use validate::validate;
