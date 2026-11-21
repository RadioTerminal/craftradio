//! Integration tests for the audio core: one binary, modules beside it.

// This allocator counts heap calls inside `assert_no_alloc`.
// The tests use it to prove that the render path does not allocate.
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

mod decode;
mod fixtures;
mod mixer;
