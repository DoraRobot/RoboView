//! RoboView core: rendering, scene graph, IO, and display-type traits.
//!
//! This crate is the GUI-free core layer (CONSTITUTION §2.4.1): it must compile
//! without any GUI dependency, so it can be used headlessly for off-screen
//! rendering, data conversion, and testing.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod displays;
pub mod io;
pub mod render;
pub mod scene;
