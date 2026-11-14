# Projects have zero or one orchestrator

## Decision

- Each project (a tmux session) has zero or one orchestrator. Most agents are ordinary workers. The role describes responsibility for coordinating the project's work.
- The background naming agent may infer leadership while naming an agent. The active agent reports promotions and demotions through the CLI when the user's assignment or coordination context makes them clear; the shipped skill teaches this behavior.
- Promotion transfers leadership. Explicit reports take precedence over inference already in flight. A previous holder stepping down cannot demote its successor.
- The role belongs to a running agent in its project. Renaming and daemon restarts preserve it; a replacement process cannot inherit it. An exit or demotion leaves a vacancy with no automatic successor election. A move carries leadership into a vacant project and requires an explicit decision if the destination has a leader.
- Orchestrators lead their project in the lists, carry a role badge alongside their live state, and can be filtered independently of state and text, within the existing workspace scope.

## Why

Leadership follows the user's intent and the work's context. Parent links alone cannot distinguish a project lead from a helper using a reviewer. Most projects need no designated leader, and concurrent inference must not undo a deliberate handoff.

## Rejected

- **Every root or parent is an orchestrator.** Independent workers have no parent, and helpers may delegate.
- **Multiple leaders per project.** The user wants one clear point of responsibility at most.
- **Continuous background elections.** Naming provides initial inference; the active agent has the conversation needed to report later changes.
