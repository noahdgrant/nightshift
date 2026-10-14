//! The pure core of `ns watch`: its decisions over plain data, with the scheduler, the forge and
//! the children left to the shell, as `run::core` does for `ns run`
//! (docs/design/factory-core/v1.md). Each submodule is a stub that phase 2 of #562 fills; they
//! are declared here now so no phase-2 change edits this file. `run::core::purity` checks that
//! none of them does I/O.

pub mod fill;
pub mod red;
pub mod settle;
pub mod split;
pub mod update;
