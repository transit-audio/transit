//! The "Render path" row of the diagnostics report (G9 of the tail audit): which path the last
//! block took (strips serial, on the render workers, through the task graph, or the strips and
//! the bus tail through the task graph) and why the bus tail graph is on or off. The callback
//! stores one code per block in a relaxed atomic, only when it changes (allocation-free); the
//! text is built off the audio thread from the reader's snapshot.

use crate::StripFaults;

/// The last block rendered its strips serially on the audio thread.
pub(crate) const PATH_SERIAL: u32 = 1;
/// The last block rendered its strips on the render workers (no sidechain key).
pub(crate) const PATH_WORKERS: u32 = 2;
/// The last block rendered its strips through the task graph (a plan with sidechain keys).
pub(crate) const PATH_GRAPH: u32 = 3;
/// The last block rendered its strips and the bus tail through the task graph.
pub(crate) const PATH_TAIL: u32 = 4;
/// The last block brought strips home from the render workers and rendered the rest serially.
pub(crate) const PATH_RECOVERY: u32 = 5;

/// The tail graph is on but the block did not take it: the strips are forced serial.
pub(crate) const TAIL_FORCED_SERIAL: u32 = 1;
/// The tail graph is on but the plan has more strips and auxes than the pool has cells.
pub(crate) const TAIL_BEYOND_CELLS: u32 = 2;
/// The tail graph is on but the plan has fewer than four strips and live auxes.
pub(crate) const TAIL_TOO_FEW: u32 = 3;

/// One code: the path in the low byte, the reason the tail graph was not taken above it.
pub(crate) fn encode(path: u32, tail_reason: u32) -> u32 {
    path | tail_reason << 8
}

/// Why a block with the tail switch on did not take the tail graph (0: no reason this rule names,
/// for example a render off the audio thread).
pub(crate) fn tail_reason(forced_serial: bool, beyond_cells: bool, strips_and_auxes: usize) -> u32 {
    if forced_serial {
        TAIL_FORCED_SERIAL
    } else if beyond_cells {
        TAIL_BEYOND_CELLS
    } else if strips_and_auxes < crate::PARALLEL_STRIPS {
        TAIL_TOO_FEW
    } else {
        0
    }
}

/// The report's "Render path" value, for example `strips on the render workers; bus tail graph
/// off (default)`. Allocates; off the audio thread.
#[must_use]
pub fn render_path_text(f: &StripFaults) -> String {
    let path = f.render_path & 0xff;
    let strips = match path {
        PATH_SERIAL => "strips serial on the audio thread",
        PATH_WORKERS => "strips on the render workers",
        PATH_GRAPH => "strips through the task graph (sidechain keys)",
        PATH_TAIL => "strips and bus tail through the task graph",
        PATH_RECOVERY => "strips coming home from the render workers",
        _ => "no block rendered yet",
    };
    let why = f.tail_why.text();
    let tail = if path == PATH_TAIL {
        format!("bus tail graph on ({why})")
    } else if f.tail_on {
        let reason = match f.render_path >> 8 {
            TAIL_FORCED_SERIAL => ", not taken: the strips are forced serial",
            TAIL_BEYOND_CELLS => ", not taken: more strips and auxes than the render workers' cells",
            TAIL_TOO_FEW => ", not taken: fewer than four strips and live auxes",
            _ => ", not taken by the last block",
        };
        format!("bus tail graph on ({why}){reason}")
    } else {
        format!("bus tail graph off ({why})")
    };
    format!("{strips}; {tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TailWhy;

    fn text(path: u32, reason: u32, tail_on: bool, tail_why: TailWhy) -> String {
        render_path_text(&StripFaults { render_path: encode(path, reason), tail_on, tail_why, ..StripFaults::default() })
    }

    /// G9: the row names the last block's path and the tail switch with its reason, injected.
    #[test]
    fn the_render_path_row_names_the_path_and_why_the_tail_is_on_or_off() {
        assert_eq!(text(0, 0, false, TailWhy::Default), "no block rendered yet; bus tail graph off (default)");
        assert_eq!(text(PATH_WORKERS, 0, false, TailWhy::Default), "strips on the render workers; bus tail graph off (default)");
        assert_eq!(text(PATH_SERIAL, 0, false, TailWhy::Off), "strips serial on the audio thread; bus tail graph off (TRANSIT_RENDER_TAIL=0)");
        assert_eq!(
            text(PATH_GRAPH, 0, false, TailWhy::GraphOff),
            "strips through the task graph (sidechain keys); bus tail graph off (TRANSIT_RENDER_GRAPH=0)"
        );
        assert_eq!(text(PATH_TAIL, 0, true, TailWhy::On), "strips and bus tail through the task graph; bus tail graph on (TRANSIT_RENDER_TAIL=1)");
        assert_eq!(
            text(PATH_WORKERS, TAIL_TOO_FEW, true, TailWhy::On),
            "strips on the render workers; bus tail graph on (TRANSIT_RENDER_TAIL=1), not taken: fewer than four strips and live auxes"
        );
        assert_eq!(
            text(PATH_SERIAL, TAIL_BEYOND_CELLS, true, TailWhy::Default),
            "strips serial on the audio thread; bus tail graph on (default), not taken: more strips and auxes than the render workers' cells"
        );
        assert_eq!(
            text(PATH_SERIAL, TAIL_FORCED_SERIAL, true, TailWhy::Forced),
            "strips serial on the audio thread; bus tail graph on (set for a test or a measurement), not taken: the strips are forced serial"
        );
        assert_eq!(
            text(PATH_RECOVERY, 0, true, TailWhy::Unrecognized),
            "strips coming home from the render workers; bus tail graph on (TRANSIT_RENDER_TAIL not recognized; default), not taken by the last block"
        );
    }

    /// The reason rule, in its order: forced serial, then the cells, then the count.
    #[test]
    fn the_tail_reason_follows_the_eligibility_rule_in_order() {
        assert_eq!(tail_reason(true, true, 0), TAIL_FORCED_SERIAL);
        assert_eq!(tail_reason(false, true, 0), TAIL_BEYOND_CELLS);
        assert_eq!(tail_reason(false, false, 3), TAIL_TOO_FEW);
        assert_eq!(tail_reason(false, false, 4), 0);
        assert_eq!(encode(PATH_TAIL, TAIL_TOO_FEW) & 0xff, PATH_TAIL);
        assert_eq!(encode(PATH_TAIL, TAIL_TOO_FEW) >> 8, TAIL_TOO_FEW);
    }
}
