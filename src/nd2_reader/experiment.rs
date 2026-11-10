use crate::error::{Error, Result};

use super::clx::{ClxMap, ClxValue};

pub(super) const MAX_EXP_DEPTH: usize = 64;

/// Highest loop-type id the SDK defines; in-range ids the reader does not handle fail loud.
const MAX_LOOP_TYPE: i64 = 10;

/// Caps every loop count, so a crafted count cannot force an uncatchable allocation abort.
const MAX_LOOP_COUNT: usize = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Axis {
    T,
    Z,
    P,
}

/// Reference loop type ids (`ELxExperimentLoopType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopType {
    Time,   // 1
    XYPos,  // 2
    ZStack, // 4
    Spect,  // 6
    Custom, // 7
    NETime, // 8
}

/// Keeps `nesting` for the reference's replace-at-same-level dedup.
struct BuiltLoop {
    ltype: LoopType,
    count: usize,
    nesting: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Loop {
    pub(super) axis: Axis,
    pub(super) count: usize,
}

/// `SpectLoop` is skipped; unsupported loops are an error.
pub(super) fn load_experiment(exp: &ClxMap) -> Result<Vec<Loop>> {
    let mut built: Vec<BuiltLoop> = Vec::new();
    load_experiment_rec(exp, 0, &mut built, 0)?;

    let mut loops: Vec<Loop> = Vec::new();
    let mut seen = [false; 3];
    for b in &built {
        let axis = match b.ltype {
            LoopType::Time | LoopType::NETime => Axis::T,
            LoopType::ZStack => Axis::Z,
            LoopType::XYPos => Axis::P,
            LoopType::Custom => {
                return Err(Error::Unsupported(
                    "ND2 CustomLoop is not supported".to_string(),
                ))
            }
            LoopType::Spect => continue, // never appended, but be defensive
        };
        let slot = match axis {
            Axis::T => 0,
            Axis::Z => 1,
            Axis::P => 2,
        };
        if seen[slot] {
            return Err(Error::Unsupported(format!(
                "ND2 has multiple {axis:?} loops, which is not supported"
            )));
        }
        seen[slot] = true;
        loops.push(Loop { axis, count: b.count });
    }
    Ok(loops)
}

/// Walks one record, appending its loop unless it is a SpectLoop, then descends into `ppNextLevelEx`.
fn load_experiment_rec(
    exp: &ClxMap,
    level: i64,
    dest: &mut Vec<BuiltLoop>,
    depth: usize,
) -> Result<()> {
    if depth > MAX_EXP_DEPTH {
        return Err(Error::Parse("ND2 experiment tree nested too deeply".to_string()));
    }

    let (ltype, count) = match single_experiment_loop(exp)? {
        Some(x) => x,
        None => return Ok(()),
    };
    if count == 0 {
        return Ok(());
    }

    let effective_level = if ltype == LoopType::Spect {
        level - 1
    } else {
        if dest.is_empty() || dest.last().unwrap().nesting < level {
            dest.push(BuiltLoop { ltype, count, nesting: level });
        } else {
            let prev = dest.last_mut().unwrap();
            if prev.nesting == level && prev.ltype == ltype && prev.count < count {
                prev.count = count;
            }
        }
        level
    };

    if let Some(next) = exp.get("ppNextLevelEx") {
        for child in exp_children(next) {
            load_experiment_rec(child, effective_level + 1, dest, depth + 1)?;
        }
    }
    Ok(())
}

/// Loop type and count of one record: `None` when it carries no loop (no `uLoopPars`, type 0, or an id above
/// MAX_LOOP_TYPE).
/// An in-range but unhandled SDK type is an Unsupported error: the reference raises instead of dropping the subtree.
/// Counts over MAX_LOOP_COUNT are Unsupported too.
fn single_experiment_loop(exp: &ClxMap) -> Result<Option<(LoopType, usize)>> {
    let e_type = exp.get_i64("eType").unwrap_or(0);
    let lp = match exp.get("uLoopPars").and_then(as_loop_params) {
        Some(m) => m,
        None => return Ok(None),
    };
    if e_type == 0 || e_type > MAX_LOOP_TYPE {
        return Ok(None);
    }

    let (ltype, count) = match e_type {
        1 => {
            let count = lp.get_usize("uiCount").unwrap_or(0);
            if count == 0 {
                return Ok(None);
            }
            (LoopType::Time, count)
        }
        2 => (LoopType::XYPos, xy_point_count(exp, lp)),
        4 => (LoopType::ZStack, lp.get_usize("uiCount").unwrap_or(0)),
        6 => {
            let mut count = lp.get_usize("uiCount").unwrap_or(0);
            if let Some(planes) = lp.get_level("pPlanes") {
                if let Some(c) = planes.get_usize("uiCount") {
                    count = c;
                }
            }
            (LoopType::Spect, count)
        }
        7 => (LoopType::Custom, lp.get_usize("uiCount").unwrap_or(0)),
        8 => (LoopType::NETime, lp.get_usize("uiCount").unwrap_or(0)),
        other => {
            return Err(Error::Unsupported(format!(
                "ND2 experiment loop type {other} is not supported"
            )))
        }
    };

    if count > MAX_LOOP_COUNT {
        return Err(Error::Unsupported(format!(
            "ND2 loop count {count} exceeds cap {MAX_LOOP_COUNT}"
        )));
    }
    Ok(Some((ltype, count)))
}

/// Unwraps the single-item indexed list that older files use for `uLoopPars`.
pub(super) fn as_loop_params(v: &ClxValue) -> Option<&ClxMap> {
    match v {
        ClxValue::Level(m) => Some(m),
        ClxValue::List(l) if l.len() == 1 => l[0].as_level(),
        _ => None,
    }
}

fn xy_point_count(exp: &ClxMap, lp: &ClxMap) -> usize {
    let points: Vec<()> = match lp.get("Points") {
        Some(ClxValue::List(l)) => l.iter().map(|_| ()).collect(),
        Some(ClxValue::Level(m)) => m.entries.iter().map(|_| ()).collect(),
        _ => Vec::new(),
    };
    let n_points = points.len();
    if n_points == 0 {
        return lp.get_usize("uiCount").unwrap_or(0);
    }
    match exp.get("pItemValid") {
        Some(ClxValue::List(valid)) if !valid.is_empty() => valid
            .iter()
            .take(n_points)
            .filter(|v| v.is_truthy())
            .count(),
        _ => n_points,
    }
}

pub(super) fn exp_children(v: &ClxValue) -> Vec<&ClxMap> {
    match v {
        ClxValue::List(l) => l.iter().filter_map(ClxValue::as_level).collect(),
        ClxValue::Level(m) => m.entries.iter().filter_map(|(_, x)| x.as_level()).collect(),
        _ => Vec::new(),
    }
}
