//! What every process of Patient Partridge that handles untrusted or secret data shares: the
//! operating system sandbox it locks itself into, the ceiling on its memory, the closing of
//! descriptors it did not ask for, and the clean protocol output.
//!
//! The PDF engine (`partridge-engine`) and the signer (`partridge-signer`) both use it. The code
//! was moved out of the engine without a change of behaviour.

#[cfg(unix)]
pub mod fds;
pub mod limits;
pub mod sandbox;
pub mod stdio;
