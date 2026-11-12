//! Incoming traversal with explicit lexical trait frame transitions.
use super::*;
const MAX_STATES: usize = 65536;
pub(in crate::code_index) struct Options<'a> {
    pub limit: usize,
    pub depth_limit: usize,
    pub include_paths: bool,
    pub work_limit: usize,
    pub deduplicate_symbols: bool,
    pub output_symbols: Option<&'a HashSet<&'a str>>,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct State<'a> {
    symbol: &'a str,
    context: Option<&'a str>,
}
pub(in crate::code_index) struct Walk {
    pub items: Vec<serde_json::Value>,
    pub truncated: bool,
    pub depth_limited: bool,
    pub metadata_incomplete: bool,
    pub diagnostics: serde_json::Value,
}
#[derive(Default)]
struct Checks {
    work: usize,
    work_limit: usize,
    pruned: usize,
    incomplete: bool,
    work_limited: bool,
    state_limited: bool,
}
fn spend(checks: &mut Checks) -> bool {
    if checks.work >= checks.work_limit {
        checks.work_limited = true;
        return false;
    }
    checks.work += 1;
    true
}
fn transitions<'a>(
    edge: &Incoming<'a>,
    target: State<'a>,
    known: &HashSet<&str>,
    projects: &HashMap<&str, Option<&str>>,
    checks: &mut Checks,
) -> Vec<Option<&'a str>> {
    if !spend(checks) {
        return vec![];
    }
    let evidence = match edge.evidence {
        Evidence::Reference(e) => e.context_evidence.as_ref(),
        _ => None,
    };
    let Some(evidence) = evidence else {
        if target.context.is_some() {
            checks.incomplete = true;
            checks.pruned += 1;
            return vec![];
        }
        return vec![None];
    };
    let body = evidence.kind == "php_captured_trait_consumers";
    let entry = evidence.kind == "php_captured_trait_entry";
    if !(body || entry) {
        checks.incomplete = true;
        return vec![];
    }
    checks.incomplete |= !evidence.enumeration_complete || evidence.truncated;
    let mut contexts = BTreeSet::new();
    for row in &evidence.contexts {
        if !spend(checks) {
            break;
        }
        let mut matches_target = false;
        for id in &row.candidates {
            if !spend(checks) {
                break;
            }
            if id == target.symbol {
                matches_target = true;
                break;
            }
        }
        if !matches_target {
            continue;
        }
        if !known.contains(row.owner.as_str())
            || projects.get(row.owner.as_str()) != projects.get(edge.source)
            || row.target_owner.as_deref().is_some_and(|id| {
                !known.contains(id) || projects.get(id) != projects.get(target.symbol)
            })
        {
            checks.incomplete = true;
            checks.pruned += 1;
            continue;
        }
        if target.context.is_some() && target.context != row.target_owner.as_deref() {
            checks.pruned += 1;
            continue;
        }
        if entry && row.target_owner.as_deref() != Some(row.owner.as_str()) {
            checks.incomplete = true;
            checks.pruned += 1;
            continue;
        }
        contexts.insert(if body { Some(row.owner.as_str()) } else { None });
    }
    contexts.into_iter().collect()
}
fn state_path<'a>(
    mut state: State<'a>,
    predecessors: &BTreeMap<State<'a>, (State<'a>, Incoming<'a>)>,
    roots: &BTreeSet<State<'a>>,
    depth_limit: usize,
) -> serde_json::Value {
    let mut steps = Vec::new();
    while !roots.contains(&state) && steps.len() < depth_limit {
        let Some((target, edge)) = predecessors.get(&state) else {
            break;
        };
        steps.push(serde_json::json!({"source_symbol_id":state.symbol,"target_symbol_id":target.symbol,"source_context":state.context,"target_context":target.context,"relation":edge.evidence.location().0,"evidence":edge.evidence.value()}));
        state = *target;
    }
    serde_json::json!({"steps":steps,"seed_symbol_id":state.symbol,"seed_context":state.context,"complete_to_seed":roots.contains(&state),"captured_trait_frames_matched":true,"runtime_context_correlation_proven":false})
}
pub(in crate::code_index) fn walk<'a>(
    selected: &[&'a Symbol],
    incoming: &BTreeMap<&'a str, Vec<Incoming<'a>>>,
    known: &HashSet<&str>,
    projects: &HashMap<&str, Option<&str>>,
    options: Options<'_>,
) -> Walk {
    let Options {
        limit,
        depth_limit,
        include_paths,
        work_limit,
        output_symbols,
        deduplicate_symbols,
    } = options;
    let mut visited: BTreeSet<_> = selected
        .iter()
        .map(|s| State {
            symbol: s.id.as_str(),
            context: None,
        })
        .collect();
    let roots = visited.clone();
    let mut predecessors = BTreeMap::new();
    let mut frontier = visited.clone();
    let mut reported: BTreeSet<_> = selected.iter().map(|s| s.id.as_str()).collect();
    let root_symbols = reported.clone();
    let mut items = Vec::new();
    let mut truncated = false;
    let mut checks = Checks {
        work_limit,
        ..Default::default()
    };
    for depth in 1..=depth_limit {
        let mut next = BTreeSet::new();
        for target in &frontier {
            for edge in incoming.get(target.symbol).into_iter().flatten() {
                for context in transitions(edge, *target, known, projects, &mut checks) {
                    let state = State {
                        symbol: edge.source,
                        context,
                    };
                    if visited.contains(&state) {
                        continue;
                    }
                    let new_symbol = (!deduplicate_symbols || !reported.contains(edge.source))
                        && !root_symbols.contains(edge.source)
                        && output_symbols.is_none_or(|ids| ids.contains(edge.source));
                    if new_symbol && items.len() >= limit {
                        truncated = true;
                        continue;
                    }
                    if visited.len() >= MAX_STATES {
                        checks.state_limited = true;
                        truncated = true;
                        continue;
                    }
                    visited.insert(state);
                    next.insert(state);
                    if include_paths {
                        predecessors.insert(state, (*target, *edge));
                    }
                    if new_symbol {
                        reported.insert(edge.source);
                        let mut item = serde_json::json!({"symbol_id":edge.source,"depth":depth,"via_kind":edge.evidence.location().0,"via_target":target.symbol,"via":edge.evidence.value()});
                        if context.is_some() || target.context.is_some() {
                            item["trait_context"] = serde_json::json!({"source_owner":context,"target_owner":target.context,"runtime_context_proven":false});
                        }
                        if include_paths {
                            item["state_path"] =
                                state_path(state, &predecessors, &roots, depth_limit);
                        }
                        items.push(item);
                    }
                }
                if checks.work_limited {
                    break;
                }
            }
            if checks.work_limited {
                break;
            }
        }
        if checks.work_limited {
            truncated = true;
            frontier = next;
            break;
        }
        if next.is_empty() {
            frontier.clear();
            break;
        }
        frontier = next;
    }
    let mut depth_limited = false;
    'peek: for target in &frontier {
        for edge in incoming.get(target.symbol).into_iter().flatten() {
            for context in transitions(edge, *target, known, projects, &mut checks) {
                if !visited.contains(&State {
                    symbol: edge.source,
                    context,
                }) {
                    depth_limited = true;
                    break 'peek;
                }
            }
            if checks.work_limited {
                break 'peek;
            }
        }
    }
    truncated |= checks.work_limited;
    Walk {
        items,
        truncated,
        depth_limited,
        metadata_incomplete: checks.incomplete,
        diagnostics: serde_json::json!({"matching":"captured_lexical_trait_frames","applied":true,"context_correlation_proven":false,"runtime_contexts_complete":false,"states_visited":visited.len(),"transitions_pruned":checks.pruned,"metadata_incomplete":checks.incomplete,"work_steps":checks.work,"work_limit":work_limit,"state_limit":MAX_STATES,"work_limited":checks.work_limited,"state_limited":checks.state_limited,"symbol_witnesses_deduplicated":deduplicate_symbols}),
    }
}
