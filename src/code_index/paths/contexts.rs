//! Conditional lexical frames for forward/back/undirected simple symbol paths.
use super::*;
pub(super) struct Choice<'a> {
    pub current: Option<&'a str>,
    pub next: Option<&'a str>,
    pub ambiguous: bool,
}
pub(super) struct Frames<'a, 'm> {
    symbols: &'m HashMap<&'a str, &'a Symbol>,
    excluded: &'m HashSet<&'a str>,
    trait_methods: HashSet<&'a str>,
    allow_ambiguous: bool,
    pub pruned: usize,
    pub incomplete: bool,
    pub exhausted: bool,
}
impl<'a, 'm> Frames<'a, 'm> {
    pub fn new(
        symbols: &'m HashMap<&'a str, &'a Symbol>,
        excluded: &'m HashSet<&'a str>,
        allow_ambiguous: bool,
    ) -> Self {
        let traits: HashSet<_> = symbols
            .values()
            .filter(|s| s.kind == "trait_declaration")
            .map(|s| (s.document_id.as_str(), s.qualified_name.as_str()))
            .collect();
        let trait_methods = symbols
            .values()
            .filter(|s| {
                s.kind == "method_declaration"
                    && s.qualified_name
                        .rsplit_once("::")
                        .is_some_and(|(owner, _)| traits.contains(&(s.document_id.as_str(), owner)))
            })
            .map(|s| s.id.as_str())
            .collect();
        Self {
            symbols,
            excluded,
            trait_methods,
            allow_ambiguous,
            pruned: 0,
            incomplete: false,
            exhausted: false,
        }
    }
    pub fn missing(&mut self, edge: &CallEdge, work: &mut usize, budget: usize) -> bool {
        if edge.context_evidence.is_some() || self.trait_methods.is_empty() {
            return false;
        }
        if edge
            .reference
            .caller
            .as_deref()
            .is_some_and(|id| self.trait_methods.contains(id))
            && (edge.reference.expression.contains("::")
                || edge.reference.expression.contains("->"))
        {
            return true;
        }
        for id in &edge.candidates {
            if *work == budget {
                self.exhausted = true;
                return false;
            }
            *work += 1;
            if self.trait_methods.contains(id.as_str()) {
                self.incomplete = true;
            }
        }
        false
    }
    pub fn missing_target(&mut self, edge: &CallEdge, target: &str) -> bool {
        if edge.context_evidence.is_none() && self.trait_methods.contains(target) {
            self.incomplete = true;
            self.pruned += 1;
            true
        } else {
            false
        }
    }
    fn consumer(&self, id: &str) -> bool {
        self.symbols.get(id).is_some_and(|s| {
            !self.excluded.contains(s.document_id.as_str())
                && matches!(s.kind.as_str(), "class_declaration" | "enum_declaration")
        })
    }
    pub fn choices(
        &mut self,
        step: &Step<'a>,
        current: Option<&'a str>,
        work: &mut usize,
        budget: usize,
    ) -> Vec<Choice<'a>> {
        let meta = match step.link.evidence {
            impact::Evidence::Reference(e) => e.context_evidence.as_ref(),
            _ => None,
        };
        let Some(meta) = meta else {
            let missing_body = matches!(step.link.evidence,impact::Evidence::Reference(e) if self.trait_methods.contains(step.link.source) && (e.reference.expression.contains("::") || e.reference.expression.contains("->")));
            if self.trait_methods.contains(step.link.target) || missing_body {
                self.incomplete = true;
                self.pruned += 1;
                return vec![];
            }
            return vec![Choice {
                current,
                next: None,
                ambiguous: step.link.ambiguous,
            }];
        };
        let body = meta.kind == "php_captured_trait_consumers";
        let entry = meta.kind == "php_captured_trait_entry";
        if !(body || entry) {
            self.incomplete = true;
            return vec![];
        }
        self.incomplete |= !meta.enumeration_complete || meta.truncated;
        let mut choices = std::collections::BTreeMap::new();
        for row in &meta.contexts {
            if *work == budget {
                self.exhausted = true;
                break;
            }
            *work += 1;
            if !row.candidates.iter().any(|id| id == step.link.target) {
                continue;
            }
            if !self.consumer(&row.owner)
                || row
                    .target_owner
                    .as_deref()
                    .is_some_and(|id| !self.consumer(id))
                || entry && row.target_owner.as_deref() != Some(row.owner.as_str())
            {
                self.incomplete = true;
                self.pruned += 1;
                continue;
            }
            let ambiguous = row.candidates.len() != 1 || row.resolution == "ambiguous";
            if ambiguous && !self.allow_ambiguous {
                continue;
            }
            let source = body.then_some(row.owner.as_str());
            let target = row.target_owner.as_deref();
            let (required, next) = if step.reversed {
                (target, source)
            } else {
                (source, target)
            };
            if current.is_some() && required != current {
                self.pruned += 1;
                continue;
            }
            let selected = current.or(required);
            choices
                .entry((selected, next))
                .and_modify(|v| *v |= ambiguous)
                .or_insert(ambiguous);
        }
        choices
            .into_iter()
            .map(|((current, next), ambiguous)| Choice {
                current,
                next,
                ambiguous,
            })
            .collect()
    }
}
