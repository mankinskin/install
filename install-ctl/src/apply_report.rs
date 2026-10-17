use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub enum ActionStatus { Written, Unchanged, Skipped, Failed }
pub struct ApplyAction { pub id: String, pub target: PathBuf, pub status: ActionStatus, pub detail: String }
#[derive(Default)]
pub struct ApplyReport { pub actions: Vec<ApplyAction> }
impl ApplyReport {
    pub fn push(&mut self, action: ApplyAction) { self.actions.push(action); }
    pub fn has_failures(&self) -> bool { self.actions.iter().any(|a| a.status == ActionStatus::Failed) }
    pub fn render(&self) {
        println!("result:");
        for action in &self.actions {
            let label = match action.status { ActionStatus::Written => "written", ActionStatus::Unchanged => "unchanged", ActionStatus::Skipped => "skipped", ActionStatus::Failed => "failed" };
            println!("{label}: {} target={} detail={}", action.id, action.target.display(), action.detail);
        }
    }
}
