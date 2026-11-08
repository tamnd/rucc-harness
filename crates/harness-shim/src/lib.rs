//! The compiler shim of the rucc harnesses, as a library.
//!
//! A build calls its C compiler thousands of times: once per unit, and hundreds of times more
//! while the Makefiles ask the compiler what it can do. A harness points all of those at a shim
//! of its own, which runs a real compiler and appends one line of JSON to `compile.jsonl` for
//! every call. The shim adds nothing that changes code. It may add `-frucc-trace=<file>` when the
//! compiler is rucc, and it may change which compiler runs under bring-up, which marks the call
//! as delegated so that the run can never be graded.
//!
//! The parts:
//!
//! - [`args`] reads a command line: inputs, outputs, language, and whether the call is a probe.
//! - [`record`] is one line of `compile.jsonl`, and reads a log back.
//! - [`run`] runs a decided call and fills in its record.
//! - [`bringup`] holds the rules every harness shares for delegated calls.
//! - [`digest`], [`usage`] and [`env`] are the hashes, the resource usage and the recorded
//!   environment.
//!
//! A harness's own binary decides which compiler a call goes to. This crate never does.

pub mod args;
pub mod bringup;
pub mod digest;
pub mod env;
pub mod record;
pub mod run;
pub mod usage;
