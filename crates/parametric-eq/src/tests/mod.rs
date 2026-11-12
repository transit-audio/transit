//! The crate's evidence (ADR 0054 decision 19, the slice 1 items), one module per topic.

// Index loops over parallel frequency and response tables read more plainly in these sweeps.
#![allow(clippy::needless_range_loop)]

mod block_size;
mod clicks;
mod closed_form;
mod continuity;
mod feasibility;
mod flat_tilt;
mod full_sweep;
mod gain_match;
mod glide;
mod hostile;
mod matching;
mod mid_side;
mod pass_through;
mod proc;
mod rbj;
mod response;
mod shelves;
mod sweep;
mod util;
