//! One [`MaintenanceStep`] per whitelisted action.
//!
//! [`step_for`] reads the action's typed params once and returns the step;
//! the plan loop then runs it without knowing which action it is.

use async_trait::async_trait;

use super::params::*;
use super::*;

/// Identity of the plan step being run (for outcomes and ops_log rows).
pub(super) struct StepInput<'a> {
    pub index: usize,
    pub name: &'static str,
    pub target: Option<&'a str>,
    pub reason: Option<&'a str>,
}

/// One executable maintenance action.
#[async_trait]
pub(super) trait MaintenanceStep: Send + Sync {
    async fn run(
        &self,
        context: &MaintenanceExecutionContext<'_>,
        input: &StepInput<'_>,
    ) -> Result<StepResult>;
}

/// Build the step for a validated plan item; bad params are a step error.
pub(super) fn step_for(item: &MaintenancePlanItem) -> Result<Box<dyn MaintenanceStep>> {
    let params = &item.params;
    Ok(match item.action {
        MaintenanceAction::Noop => Box::new(Noop),
        MaintenanceAction::Refile => Box::new(Refile(RefileParams::parse(params))),
        MaintenanceAction::Pin => Box::new(Pin(true)),
        MaintenanceAction::Unpin => Box::new(Pin(false)),
        MaintenanceAction::SetBoost => Box::new(SetBoost(SetBoostParams::parse(params))),
        MaintenanceAction::SetTags => Box::new(SetTags(SetTagsParams::parse(params))),
        MaintenanceAction::Archive => Box::new(Status("archived")),
        MaintenanceAction::Unarchive => Box::new(Status("active")),
        MaintenanceAction::RebuildIndex => Box::new(RebuildIndex),
        MaintenanceAction::RebuildGraph => {
            Box::new(RebuildGraph(RebuildGraphParams::parse(params)?))
        }
        MaintenanceAction::ReindexFts => Box::new(ReindexFts),
        MaintenanceAction::Reembed => Box::new(Reembed),
        MaintenanceAction::ReembedAll => Box::new(ReembedAll(ReembedAllParams::parse(params)?)),
        MaintenanceAction::CompileSource => {
            Box::new(CompileSource(CompileSourceParams::parse(params)))
        }
        MaintenanceAction::Consolidate => Box::new(Consolidate(ConsolidateParams::parse(params))),
        MaintenanceAction::RefreshStaleWiki => {
            Box::new(RefreshStale(RefreshStaleParams::parse(params)?))
        }
        MaintenanceAction::FileAnswer => Box::new(FileAnswer(FileAnswerParams::parse(params))),
        MaintenanceAction::MergeExactDup => {
            Box::new(MergeExactDup(MergeExactDupParams::parse(params)))
        }
        MaintenanceAction::DropTombstones => {
            Box::new(DropTombstones(DropTombstonesParams::parse(params)?))
        }
        MaintenanceAction::Vacuum | MaintenanceAction::Checkpoint => Box::new(Vacuum),
    })
}

struct Noop;

#[async_trait]
impl MaintenanceStep for Noop {
    async fn run(
        &self,
        context: &MaintenanceExecutionContext<'_>,
        input: &StepInput<'_>,
    ) -> Result<StepResult> {
        Ok(StepResult {
            outcome: ok_outcome(
                input.index,
                input.name,
                input.target,
                context.dry_run,
                "noop",
                Some(json!({"reason": input.reason})),
            ),
            docs_delta: 0,
        })
    }
}

/// Declares a step whose body runs synchronously on the store.
macro_rules! sync_step {
    ($step:ty, |$this:ident, $ctx:ident, $input:ident| $body:expr) => {
        #[async_trait]
        impl MaintenanceStep for $step {
            async fn run(
                &self,
                $ctx: &MaintenanceExecutionContext<'_>,
                $input: &StepInput<'_>,
            ) -> Result<StepResult> {
                let $this = self;
                blocking_section(|| $body)
            }
        }
    };
}

struct Refile(RefileParams);
sync_step!(Refile, |s, c, i| apply_refile(
    c.store, i.index, i.name, i.target, &s.0, c.dry_run
));

struct Pin(bool);
sync_step!(Pin, |s, c, i| apply_pin(
    c.store, i.index, i.name, i.target, s.0, c.dry_run
));

struct SetBoost(SetBoostParams);
sync_step!(SetBoost, |s, c, i| apply_set_boost(
    c.store, i.index, i.name, i.target, &s.0, c.dry_run
));

struct SetTags(SetTagsParams);
sync_step!(SetTags, |s, c, i| apply_set_tags(
    c.store, i.index, i.name, i.target, &s.0, c.dry_run
));

struct Status(&'static str);
sync_step!(Status, |s, c, i| apply_status(
    c.store, i.index, i.name, i.target, s.0, c.dry_run
));

struct RebuildIndex;
sync_step!(RebuildIndex, |_s, c, i| apply_rebuild_index(
    c.store, i.index, i.name, c.dry_run
));

struct RebuildGraph(RebuildGraphParams);
sync_step!(RebuildGraph, |s, c, i| apply_rebuild_graph(
    c.store, i.index, i.name, &s.0, c.config, c.dry_run
));

struct MergeExactDup(MergeExactDupParams);
sync_step!(MergeExactDup, |s, c, i| apply_merge_exact_dup(
    c.store,
    i.index,
    i.name,
    i.target,
    &s.0,
    c.dry_run,
    c.remaining_docs
));

struct DropTombstones(DropTombstonesParams);
sync_step!(DropTombstones, |s, c, i| apply_drop_tombstones(
    c.store, i.index, i.name, &s.0, c.config, c.dry_run
));

struct Vacuum;
sync_step!(Vacuum, |_s, c, i| apply_vacuum(
    c.store, i.index, i.name, c.dry_run
));

struct ReindexFts;

#[async_trait]
impl MaintenanceStep for ReindexFts {
    async fn run(
        &self,
        c: &MaintenanceExecutionContext<'_>,
        i: &StepInput<'_>,
    ) -> Result<StepResult> {
        apply_reindex_fts(c.store, i.index, i.name, c.config, c.dry_run).await
    }
}

struct Reembed;

#[async_trait]
impl MaintenanceStep for Reembed {
    async fn run(
        &self,
        c: &MaintenanceExecutionContext<'_>,
        i: &StepInput<'_>,
    ) -> Result<StepResult> {
        apply_reembed_one(
            c.store, c.embedder, c.config, i.index, i.name, i.target, c.dry_run,
        )
        .await
    }
}

struct ReembedAll(ReembedAllParams);

#[async_trait]
impl MaintenanceStep for ReembedAll {
    async fn run(
        &self,
        c: &MaintenanceExecutionContext<'_>,
        i: &StepInput<'_>,
    ) -> Result<StepResult> {
        apply_reembed_all(
            c.store, c.embedder, c.config, i.index, i.name, &self.0, c.dry_run,
        )
        .await
    }
}

struct CompileSource(CompileSourceParams);

#[async_trait]
impl MaintenanceStep for CompileSource {
    async fn run(
        &self,
        c: &MaintenanceExecutionContext<'_>,
        i: &StepInput<'_>,
    ) -> Result<StepResult> {
        apply_compile_source(c, i.index, i.name, i.target, &self.0).await
    }
}

struct Consolidate(ConsolidateParams);

#[async_trait]
impl MaintenanceStep for Consolidate {
    async fn run(
        &self,
        c: &MaintenanceExecutionContext<'_>,
        i: &StepInput<'_>,
    ) -> Result<StepResult> {
        apply_consolidate(c, i.index, i.name, i.target, &self.0).await
    }
}

struct RefreshStale(RefreshStaleParams);

#[async_trait]
impl MaintenanceStep for RefreshStale {
    async fn run(
        &self,
        c: &MaintenanceExecutionContext<'_>,
        i: &StepInput<'_>,
    ) -> Result<StepResult> {
        apply_refresh_stale(c, i.index, i.name, &self.0).await
    }
}

struct FileAnswer(FileAnswerParams);

#[async_trait]
impl MaintenanceStep for FileAnswer {
    async fn run(
        &self,
        c: &MaintenanceExecutionContext<'_>,
        i: &StepInput<'_>,
    ) -> Result<StepResult> {
        apply_file_answer(c, i.index, i.name, &self.0).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every whitelisted action builds a step from the wire fixture, and the
    /// typed params read the lenient forms exactly as before.
    #[test]
    fn wire_fixture_params_parse_into_typed_steps() {
        let items: Vec<MaintenancePlanItem> =
            serde_json::from_str(crate::maintain::plan::tests::WIRE_PLAN_FIXTURE).unwrap();
        for item in &items {
            assert!(step_for(item).is_ok(), "{}", item.action);
        }
        let p = |a: MaintenanceAction| &items.iter().find(|i| i.action == a).unwrap().params;

        let refile = RefileParams::parse(p(MaintenanceAction::Refile));
        assert_eq!(refile.wing.as_deref(), Some("projects"));
        assert_eq!(refile.room.as_deref(), Some(""));
        assert_eq!(refile.title.as_deref(), Some("T"));
        assert_eq!(refile.layer, None);

        let tags = SetTagsParams::parse(p(MaintenanceAction::SetTags));
        assert_eq!(tags.tags, ["a", "b", "#c"]);
        assert_eq!(tags.mode, "add");

        let graph = RebuildGraphParams::parse(p(MaintenanceAction::RebuildGraph)).unwrap();
        assert_eq!(
            graph,
            RebuildGraphParams {
                dirty_only: false,
                max_docs: Some(5)
            }
        );
        let reembed = ReembedAllParams::parse(p(MaintenanceAction::ReembedAll)).unwrap();
        assert_eq!(reembed.max_docs, Some(12));
        let refresh = RefreshStaleParams::parse(p(MaintenanceAction::RefreshStaleWiki)).unwrap();
        assert!(refresh.dry_run);

        let consolidate = ConsolidateParams::parse(p(MaintenanceAction::Consolidate));
        assert_eq!(consolidate.content.as_deref(), Some("B"));
        let answer = FileAnswerParams::parse(p(MaintenanceAction::FileAnswer));
        assert_eq!(answer.body.as_deref(), Some("A"));
        assert_eq!(answer.citations.map(|c| c.len()), Some(1));

        let merge = MergeExactDupParams::parse(p(MaintenanceAction::MergeExactDup));
        assert_eq!(merge.source_ids, ["keep", "drop"]);
        assert!(merge.allow_raw_delete && merge.delete);
        let compile = CompileSourceParams::parse(p(MaintenanceAction::CompileSource));
        assert_eq!(compile.source_id_or_uri.as_deref(), Some("file:///x.md"));
        assert_eq!(
            SetBoostParams::parse(p(MaintenanceAction::SetBoost)).boost,
            Some(1.5)
        );
    }

    #[test]
    fn bad_numeric_params_are_step_errors() {
        let item = MaintenancePlanItem {
            action: MaintenanceAction::DropTombstones,
            reason: None,
            target_id: None,
            params: json!({"max_docs": -1}),
        };
        let err = step_for(&item).err().unwrap().to_string();
        assert!(err.contains("max_docs"), "{err}");
    }
}
