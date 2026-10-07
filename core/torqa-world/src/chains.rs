//! Joining the pieces of ways end to end (#116, #117). The map splits a way where its tags
//! change and cuts it at every tile border, but a street or a railway is one line and is laid
//! out as one: a bridge's deck runs between its real ends, not down to the ground where a tile
//! cut it, and a railway keeps its grades across the cut.

use std::collections::HashMap;

/// Ends of pieces this close meet: the parts of a way cut at a tile border end within a few
/// decimetres of each other, each tile having rounded the way's points to its own pixels.
const JOIN_M: f64 = 0.5;

/// The pieces `lines` (metres east/north) joined where exactly two of them end at one place
/// (not at junctions, where more do) and `joinable` says those two go together: each chain as
/// its pieces in order, with whether each runs reversed. Every piece of two points or more is
/// in exactly one chain.
pub(crate) fn chains(
    lines: &[&[(f64, f64)]],
    joinable: &dyn Fn(usize, usize) -> bool,
) -> Vec<Vec<(usize, bool)>> {
    #[allow(clippy::cast_possible_truncation)] // local metres stay far below 2^63 cells
    let key = |(east, north): (f64, f64)| {
        (
            (east / JOIN_M).floor() as i64,
            (north / JOIN_M).floor() as i64,
        )
    };
    let end_of = |piece: usize, last: bool| {
        let line = lines[piece];
        if last { line[line.len() - 1] } else { line[0] }
    };
    // The pieces' ends by cell: the piece, and whether it is its last point.
    let mut ends: HashMap<(i64, i64), Vec<(usize, bool)>> = HashMap::new();
    for (piece, line) in lines.iter().enumerate() {
        if line.len() >= 2 {
            for last in [false, true] {
                ends.entry(key(end_of(piece, last)))
                    .or_default()
                    .push((piece, last));
            }
        }
    }
    let meeting = |at: (f64, f64)| -> Vec<(usize, bool)> {
        let (ke, kn) = key(at);
        (ke - 1..=ke + 1)
            .flat_map(|x| (kn - 1..=kn + 1).map(move |y| (x, y)))
            .flat_map(|cell| ends.get(&cell).into_iter().flatten().copied())
            .filter(|&(piece, last)| {
                let end = end_of(piece, last);
                (end.0 - at.0).hypot(end.1 - at.1) <= JOIN_M
            })
            .collect()
    };
    let mut used = vec![false; lines.len()];
    let mut chains = Vec::new();
    for start in 0..lines.len() {
        if used[start] || lines[start].len() < 2 {
            continue;
        }
        used[start] = true;
        let mut chain = vec![(start, false)];
        // Grow the chain at its end, then at its start.
        for forward in [true, false] {
            loop {
                let (piece, reversed) = if forward {
                    chain[chain.len() - 1]
                } else {
                    chain[0]
                };
                // The chain's free end on this side: the piece's last point as the chain runs
                // forward through it, or its first.
                let at = end_of(piece, forward != reversed);
                let here = meeting(at);
                if here.len() != 2 {
                    break;
                }
                let Some(&(other, other_last)) =
                    here.iter().find(|&&(candidate, _)| candidate != piece)
                else {
                    break;
                };
                if used[other] || !joinable(piece, other) {
                    break;
                }
                used[other] = true;
                // Running on from `at`, a piece meeting it with its last point runs reversed;
                // leading up to `at`, one meeting it with its first point does.
                if forward {
                    chain.push((other, other_last));
                } else {
                    chain.insert(0, (other, !other_last));
                }
            }
        }
        chains.push(chain);
    }
    chains
}
