//! Routing scenarios driven through [`super::sim`], which calls the same `step` as
//! `run/drive.rs`. Each test that moved here from `cli/tests/factory/` keeps its name, so
//! `git log -S<name>` finds the end-to-end version it replaced. One file per area.

mod gates;
mod guards;
mod merge;
mod review_loop;
mod routing;
