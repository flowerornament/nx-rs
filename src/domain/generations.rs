use std::cmp::Reverse;
use std::collections::BTreeSet;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GenerationKind {
    DarwinSystem,
    HomeManager,
}

impl GenerationKind {
    pub const ALL: [Self; 2] = [Self::DarwinSystem, Self::HomeManager];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::DarwinSystem => "darwin",
            Self::HomeManager => "home-manager",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct GenerationId(u64);

impl GenerationId {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GenerationRecord {
    pub kind: GenerationKind,
    pub id: GenerationId,
    pub created_at: Option<String>,
    pub current: bool,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiskUsageSnapshot {
    pub filesystem: String,
    pub size: String,
    pub used: String,
    pub available: String,
    pub capacity: String,
    pub mounted_on: String,
    #[serde(skip)]
    pub available_bytes: u64,
}

impl GenerationRecord {
    #[cfg(test)]
    #[must_use]
    pub fn new(kind: GenerationKind, id: u64) -> Self {
        Self {
            kind,
            id: GenerationId::new(id),
            created_at: None,
            current: false,
            label: None,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn current(mut self) -> Self {
        self.current = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetentionPolicy {
    pub keep_newest: usize,
    pub kinds: BTreeSet<GenerationKind>,
    pub run_gc: bool,
}

impl RetentionPolicy {
    #[must_use]
    pub fn new(
        keep_newest: usize,
        kinds: impl IntoIterator<Item = GenerationKind>,
        run_gc: bool,
    ) -> Self {
        Self {
            keep_newest,
            kinds: kinds.into_iter().collect(),
            run_gc,
        }
    }

    #[must_use]
    pub fn all(keep_newest: usize, run_gc: bool) -> Self {
        Self::new(keep_newest, GenerationKind::ALL, run_gc)
    }

    #[must_use]
    pub fn single(keep_newest: usize, kind: GenerationKind, run_gc: bool) -> Self {
        Self::new(keep_newest, [kind], run_gc)
    }

    #[must_use]
    pub fn includes(&self, kind: GenerationKind) -> bool {
        self.kinds.contains(&kind)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetentionAction {
    KeepCurrent,
    KeepByRetentionWindow,
    Prune,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetentionDecision {
    pub generation: GenerationRecord,
    pub action: RetentionAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecutionPlan {
    pub darwin_remove: Vec<GenerationId>,
    pub home_manager_remove: Vec<GenerationId>,
    pub run_gc: bool,
}

impl ExecutionPlan {
    #[must_use]
    pub fn prune_ids(&self, kind: GenerationKind) -> &[GenerationId] {
        match kind {
            GenerationKind::DarwinSystem => &self.darwin_remove,
            GenerationKind::HomeManager => &self.home_manager_remove,
        }
    }

    #[must_use]
    pub fn has_prunable_generations(&self) -> bool {
        !self.darwin_remove.is_empty() || !self.home_manager_remove.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrunePlan {
    pub policy: RetentionPolicy,
    pub decisions: Vec<RetentionDecision>,
    pub execution: ExecutionPlan,
}

impl PrunePlan {
    #[cfg(test)]
    #[must_use]
    pub fn decisions_for(&self, kind: GenerationKind) -> Vec<&RetentionDecision> {
        self.decisions
            .iter()
            .filter(|decision| decision.generation.kind == kind)
            .collect()
    }
}

#[must_use]
pub fn plan_prune(generations: &[GenerationRecord], policy: RetentionPolicy) -> PrunePlan {
    let decisions = GenerationKind::ALL
        .into_iter()
        .flat_map(|kind| plan_kind(generations, &policy, kind))
        .collect::<Vec<_>>();
    let execution = build_execution_plan(&decisions, policy.run_gc);

    PrunePlan {
        policy,
        decisions,
        execution,
    }
}

fn plan_kind(
    generations: &[GenerationRecord],
    policy: &RetentionPolicy,
    kind: GenerationKind,
) -> Vec<RetentionDecision> {
    let mut family: Vec<_> = generations
        .iter()
        .filter(|generation| generation.kind == kind)
        .cloned()
        .collect();
    family.sort_by_key(|generation| Reverse(generation.id));

    if !policy.includes(kind) {
        return family
            .into_iter()
            .map(|generation| RetentionDecision {
                action: RetentionAction::KeepByRetentionWindow,
                generation,
            })
            .collect();
    }

    family
        .into_iter()
        .enumerate()
        .map(|(index, generation)| RetentionDecision {
            action: if generation.current {
                RetentionAction::KeepCurrent
            } else if index < policy.keep_newest {
                RetentionAction::KeepByRetentionWindow
            } else {
                RetentionAction::Prune
            },
            generation,
        })
        .collect()
}

fn build_execution_plan(decisions: &[RetentionDecision], run_gc: bool) -> ExecutionPlan {
    let prune_ids = |kind| {
        decisions
            .iter()
            .filter(|decision| {
                decision.generation.kind == kind && decision.action == RetentionAction::Prune
            })
            .map(|decision| decision.generation.id)
            .collect()
    };

    ExecutionPlan {
        darwin_remove: prune_ids(GenerationKind::DarwinSystem),
        home_manager_remove: prune_ids(GenerationKind::HomeManager),
        run_gc,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn darwin(id: u64) -> GenerationRecord {
        GenerationRecord::new(GenerationKind::DarwinSystem, id)
    }

    fn home_manager(id: u64) -> GenerationRecord {
        GenerationRecord::new(GenerationKind::HomeManager, id)
    }

    #[test]
    fn planner_keeps_newest_generations_per_family() {
        let plan = plan_prune(
            &[
                darwin(12),
                darwin(11),
                darwin(10),
                home_manager(8),
                home_manager(7),
            ],
            RetentionPolicy::all(2, true),
        );

        let darwin_actions: Vec<_> = plan
            .decisions_for(GenerationKind::DarwinSystem)
            .into_iter()
            .map(|decision| decision.action)
            .collect();
        let hm_actions: Vec<_> = plan
            .decisions_for(GenerationKind::HomeManager)
            .into_iter()
            .map(|decision| decision.action)
            .collect();

        assert_eq!(
            darwin_actions,
            vec![
                RetentionAction::KeepByRetentionWindow,
                RetentionAction::KeepByRetentionWindow,
                RetentionAction::Prune,
            ]
        );
        assert_eq!(
            hm_actions,
            vec![
                RetentionAction::KeepByRetentionWindow,
                RetentionAction::KeepByRetentionWindow,
            ]
        );
        assert_eq!(plan.execution.darwin_remove, vec![GenerationId::new(10)]);
        assert!(plan.execution.home_manager_remove.is_empty());
        assert!(plan.execution.run_gc);
    }

    #[test]
    fn planner_preserves_current_generation_outside_retention_window() {
        let plan = plan_prune(
            &[darwin(30), darwin(29), darwin(28).current(), darwin(27)],
            RetentionPolicy::single(1, GenerationKind::DarwinSystem, false),
        );

        let actions: Vec<_> = plan
            .decisions_for(GenerationKind::DarwinSystem)
            .into_iter()
            .map(|decision| (decision.generation.id.get(), decision.action))
            .collect();

        assert_eq!(
            actions,
            vec![
                (30, RetentionAction::KeepByRetentionWindow),
                (29, RetentionAction::Prune),
                (28, RetentionAction::KeepCurrent),
                (27, RetentionAction::Prune),
            ]
        );
        assert_eq!(
            plan.execution.darwin_remove,
            vec![GenerationId::new(29), GenerationId::new(27)]
        );
        assert!(!plan.execution.run_gc);
    }

    #[test]
    fn planner_respects_kind_filtering() {
        let plan = plan_prune(
            &[darwin(5), darwin(4), home_manager(9), home_manager(8)],
            RetentionPolicy::single(1, GenerationKind::HomeManager, true),
        );

        let darwin_actions: Vec<_> = plan
            .decisions_for(GenerationKind::DarwinSystem)
            .into_iter()
            .map(|decision| decision.action)
            .collect();
        let hm_actions: Vec<_> = plan
            .decisions_for(GenerationKind::HomeManager)
            .into_iter()
            .map(|decision| decision.action)
            .collect();

        assert_eq!(
            darwin_actions,
            vec![
                RetentionAction::KeepByRetentionWindow,
                RetentionAction::KeepByRetentionWindow,
            ]
        );
        assert_eq!(
            hm_actions,
            vec![
                RetentionAction::KeepByRetentionWindow,
                RetentionAction::Prune,
            ]
        );
        assert!(plan.execution.darwin_remove.is_empty());
        assert_eq!(
            plan.execution.home_manager_remove,
            vec![GenerationId::new(8)]
        );
    }

    #[test]
    fn planner_sorts_newest_first_before_deciding() {
        let plan = plan_prune(
            &[home_manager(3), home_manager(9), home_manager(5)],
            RetentionPolicy::single(1, GenerationKind::HomeManager, true),
        );

        let decisions: Vec<_> = plan
            .decisions_for(GenerationKind::HomeManager)
            .into_iter()
            .map(|decision| decision.generation.id.get())
            .collect();

        assert_eq!(decisions, vec![9, 5, 3]);
        assert_eq!(
            plan.execution.home_manager_remove,
            vec![GenerationId::new(5), GenerationId::new(3)]
        );
    }

    #[test]
    fn execution_plan_reports_when_nothing_is_prunable() {
        let plan = plan_prune(
            &[darwin(2).current(), home_manager(1)],
            RetentionPolicy::all(5, true),
        );

        assert!(!plan.execution.has_prunable_generations());
        assert!(plan.execution.run_gc);
    }
}
