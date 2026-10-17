//! Interactive artifact selector.  This module deliberately only returns a
//! selection: installation remains in the scripted command's apply path.

use std::io::stdout;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Terminal,
};

use crate::{
    plan::{category_token, exclusion_reason, InstallOptions, InstallPlan},
    review_plan::{build_review_plan, GuidanceInput, ReviewPlan},
    registry::{EntryCategory, RegistryEntryV2, INSTALL_CATEGORIES},
    guidance::destination::DestinationPaths,
};

enum Screen {
    Categories,
    Artifacts,
    Guidance,
    Review,
    RootEdit,
}

pub struct SelectionState {
    inventory: Vec<RegistryEntryV2>,
    root: std::path::PathBuf,
    options: InstallOptions,
    categories: [bool; 6],
    selected: Vec<bool>,
}

impl SelectionState {
    pub fn new(inventory: &[RegistryEntryV2], root: std::path::PathBuf, options: InstallOptions) -> Self {
        Self { inventory: inventory.to_vec(), root, options, categories: [false; 6], selected: vec![false; inventory.len()] }
    }
    fn category_index(category: EntryCategory) -> usize {
        INSTALL_CATEGORIES.iter().position(|c| *c == category).unwrap()
    }
    pub fn toggle_category(&mut self, category: EntryCategory) -> bool {
        let index = Self::category_index(category);
        if !self.inventory.iter().any(|entry| entry.category == category) { return false; }
        self.categories[index] = !self.categories[index]; true
    }
    pub fn toggle_all_categories(&mut self) {
        let all = INSTALL_CATEGORIES.iter().filter(|category| self.inventory.iter().any(|entry| entry.category == **category)).all(|category| self.categories[Self::category_index(*category)]);
        for category in INSTALL_CATEGORIES { if self.inventory.iter().any(|entry| entry.category == *category) { self.categories[Self::category_index(*category)] = !all; } }
    }
    pub fn enter_artifacts(&mut self) {
        self.selected.fill(false);
        for index in self.visible_indices() {
            self.selected[index] = true;
        }
    }
    pub fn visible_indices(&self) -> Vec<usize> { self.inventory.iter().enumerate().filter(|(_, entry)| self.categories[Self::category_index(entry.category)]).map(|(index, _)| index).collect() }
    pub fn toggle_visible(&mut self, offset: usize) -> Result<(), String> { let index = *self.visible_indices().get(offset).ok_or_else(|| "no visible artifact at offset".to_string())?; self.selected[index] = !self.selected[index]; Ok(()) }
    pub fn toggle_all_visible(&mut self) { let visible = self.visible_indices(); let all = visible.iter().all(|index| self.selected[*index]); for index in visible { self.selected[index] = !all; } }
    pub fn prepare_review(&self) -> Result<InstallPlan, String> {
        let visible = self.visible_indices();
        let all = visible.len() == self.inventory.len() && visible.iter().all(|index| self.selected[*index]);
        let tokens = if all { vec!["all".into()] } else { self.inventory.iter().enumerate().filter(|(index, _)| self.selected[*index]).map(|(_, entry)| entry.id.clone()).collect() };
        InstallPlan::from_inventory(&self.inventory, &tokens, self.root.clone(), self.options)
    }
    pub fn cancel(&mut self) { self.categories = [false; 6]; self.selected.fill(false); }
    fn set_root(&mut self, root: std::path::PathBuf) { self.root = root; }
    pub fn prepare_composed_review(&self, inputs: &[GuidanceInput], paths: &DestinationPaths) -> Result<ReviewPlan, String> {
        let selected_inputs: Vec<GuidanceInput> = inputs.iter().cloned().collect();
        let any_artifact = self.selected.iter().any(|selected| *selected);
        let install = if any_artifact {
            self.prepare_review()?
        } else {
            InstallPlan { selected: vec![], root: self.root.clone(), options: self.options, excluded: vec![] }
        };
        if install.selected.is_empty() && selected_inputs.is_empty() {
            return Err("selection matched no artifacts".into());
        }
        build_review_plan(install, &selected_inputs, paths)
    }
}

struct RestoreTerminal;
impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
}

/// Run the real terminal event/render loop. `None` is a successful
/// cancellation and, importantly, is returned before the caller can apply.
pub fn run_with_source(
    inventory: &[RegistryEntryV2], guidance_inputs: &[GuidanceInput], destination_paths: &DestinationPaths,
    target_root: std::path::PathBuf,
    options: InstallOptions,
    source: Option<&str>,
) -> Result<Option<ReviewPlan>, String> {
    enable_raw_mode().map_err(|e| e.to_string())?;
    // Establish the guard before the second terminal operation: entering raw
    // mode succeeded even if switching to the alternate screen does not.
    let _restore = RestoreTerminal;
    execute!(stdout(), EnterAlternateScreen).map_err(|e| e.to_string())?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout())).map_err(|e| e.to_string())?;
    let categories = INSTALL_CATEGORIES;
    let mut state = SelectionState::new(inventory, target_root, options);
    let mut reviewed_plan: Option<ReviewPlan> = None;
    let mut error: Option<String> = None;
    let mut selected_guidance = vec![false; guidance_inputs.len()];
    let mut screen = Screen::Categories;
    let mut cursor = 0usize;
    let mut review_scroll = 0usize;
    let mut root_text = String::new();
    let mut list_state = ListState::default();
    let mut review_state = ListState::default();

    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            let title = match screen {
                Screen::Categories => "Artifact categories — Space toggle, Enter continue, Esc cancel",
                Screen::Artifacts => "Artifacts — Space toggle, Enter review, Esc back",
                Screen::Guidance => "Guidance profiles — Space toggle, Enter review, Esc back",
                Screen::Review => "Review — Enter/y install, n/q/Esc cancel",
                Screen::RootEdit => "Binary installation root — type path, Enter save, Esc cancel",
            };
            let rows: Vec<ListItem> = match screen {
                Screen::Categories => {
                    let all_selected = categories.iter().filter(|category| inventory.iter().any(|entry| entry.category == **category)).all(|category| state.categories[SelectionState::category_index(*category)]);
                    let mut rows = vec![row(0, cursor, all_selected, "All registered artifacts".into())];
                    rows.extend(categories.iter().enumerate().map(|(i, category)| {
                        let token = category_token(*category);
                        let count = inventory.iter().filter(|a| a.category == *category).count();
                        let label = match category { EntryCategory::Cli => "CLI", EntryCategory::Mcp => "MCP", EntryCategory::Service => "Service", EntryCategory::VscodeExtension => "VS Code extension", EntryCategory::Hook => "Hook", EntryCategory::Misc => "Misc" };
                        let selected = inventory.iter().filter(|entry| entry.category == *category && state.selected[inventory.iter().position(|candidate| candidate.id == entry.id).unwrap()]).count();
                        row(i + 1, cursor, state.categories[i], format!("{label} [{token}] ({selected}/{count}){}", if count == 0 { " — no registered artifacts" } else { "" }))
                    }));
                    rows
                }
                Screen::Artifacts => state.visible_indices().into_iter().enumerate().map(|(offset, i)| {
                    let entry = &inventory[i];
                    let suffix = exclusion_reason(entry).map(|r| format!(" — {r}")).unwrap_or_default();
                    row(offset, cursor, state.selected[i], format!("{} [{}]{suffix}", entry.id, category_token(entry.category)))
                }).collect(),
                Screen::Guidance => guidance_inputs.iter().enumerate().map(|(i, input)| row(i, cursor, selected_guidance[i], input.profile_path.display().to_string())).collect(),
                Screen::RootEdit => vec![ListItem::new(root_text.clone())],
                Screen::Review => {
                    let Some(review) = reviewed_plan.as_ref() else { return; };
                    let plan = &review.install;
                    let mut lines = if let Some(source) = source {
                        wrap_review_row("source: ", source, area.width)
                    } else {
                        vec![ListItem::new(format!(
                            "cargo options: force={} offline={} locked={}", plan.options.force, plan.options.offline, plan.options.locked
                        ))]
                    };
                    lines.push(ListItem::new(format!(
                        "target installation root: {}",
                        plan.root.display()
                    )));
                    lines.extend(plan.selected.iter().map(|a| {
                        ListItem::new(format!("install: {} [{}]", a.id, a.category))
                    }));
                    lines.extend(plan.excluded.iter().map(|e| ListItem::new(format!("excluded: {} — {}", e.id, e.reason))));
                    for guidance in &review.guidance {
                        lines.push(ListItem::new(format!("guidance profile: {}", guidance.plan.profile_id)));
                        lines.extend(wrap_review_row("source: ", &guidance.source_root.display().to_string(), area.width));
                        lines.extend(wrap_review_row("scope: ", &guidance.plan.destination_scope, area.width));
                        lines.extend(wrap_review_row("destination: ", &guidance.plan.destination_root, area.width));
                        for artifact in &guidance.plan.artifacts {
                            lines.extend(wrap_review_row("write: ", &artifact.final_path, area.width));
                        }
                        lines.extend(guidance.plan.diagnostics.iter().map(|diagnostic| ListItem::new(format!("  diagnostic: {}", diagnostic.message))));
                    }
                    if let Some(error) = &error { lines.push(ListItem::new(format!("error: {error}"))); }
                    lines
                }
            };
            let chunks = Layout::default().direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(1)]).split(area);
            let title = if let Some(error) = &error { format!("{title} — error: {error}") } else { title.to_string() };
            frame.render_widget(Paragraph::new(title).block(Block::default().borders(Borders::ALL)), chunks[0]);
            if matches!(screen, Screen::Categories | Screen::Artifacts | Screen::Guidance) {
                list_state.select(Some(cursor));
                frame.render_stateful_widget(List::new(rows).block(Block::default().borders(Borders::ALL)), chunks[1], &mut list_state);
            } else {
                review_scroll = review_scroll.min(rows.len().saturating_sub(1));
                review_state.select(Some(review_scroll));
                frame.render_stateful_widget(List::new(rows).block(Block::default().borders(Borders::ALL)), chunks[1], &mut review_state);
            }
        }).map_err(|e| e.to_string())?;

        let Event::Key(key) = event::read().map_err(|e| e.to_string())? else { continue };
        if key.kind != KeyEventKind::Press { continue; }
        match (key.code, &screen) {
            (KeyCode::Esc | KeyCode::Char('q'), Screen::Categories | Screen::Review) => { state.cancel(); return Ok(None) },
            (KeyCode::Esc, Screen::Artifacts) => { screen = Screen::Categories; cursor = 0; }
            (KeyCode::Esc, Screen::Guidance) => { screen = Screen::Artifacts; cursor = 0; }
            (KeyCode::Up, Screen::Categories) if cursor > 0 => cursor -= 1,
            (KeyCode::Down, Screen::Categories) if cursor < categories.len() => cursor += 1,
            (KeyCode::Char(' '), Screen::Categories) if cursor == 0 => state.toggle_all_categories(),
            (KeyCode::Char(' '), Screen::Categories) => { state.toggle_category(categories[cursor - 1]); },
            (KeyCode::Char('a'), Screen::Categories) => state.toggle_all_categories(),
            (KeyCode::Enter, Screen::Categories) => {
                state.enter_artifacts();
                screen = Screen::Artifacts; cursor = 0;
            }
            (KeyCode::Up, Screen::Artifacts) if cursor > 0 => cursor -= 1,
            (KeyCode::Down, Screen::Artifacts) if cursor + 1 < state.visible_indices().len() => cursor += 1,
            (KeyCode::Char(' '), Screen::Artifacts) => { if let Err(e) = state.toggle_visible(cursor) { error = Some(e); } },
            (KeyCode::Char('a'), Screen::Artifacts) => state.toggle_all_visible(),
            (KeyCode::Enter, Screen::Artifacts) if !guidance_inputs.is_empty() => { screen = Screen::Guidance; cursor = 0; }
            (KeyCode::Enter, Screen::Artifacts) => prepare_composed_review(&state, guidance_inputs, &selected_guidance, destination_paths, &mut reviewed_plan, &mut error, &mut screen),
            (KeyCode::Up, Screen::Guidance) if cursor > 0 => cursor -= 1,
            (KeyCode::Down, Screen::Guidance) if cursor + 1 < guidance_inputs.len() => cursor += 1,
            (KeyCode::Char(' '), Screen::Guidance) if !guidance_inputs.is_empty() => selected_guidance[cursor] = !selected_guidance[cursor],
            (KeyCode::Char('a'), Screen::Guidance) => { let all = selected_guidance.iter().all(|v| *v); selected_guidance.fill(!all); },
            (KeyCode::Enter, Screen::Guidance) => prepare_composed_review(&state, guidance_inputs, &selected_guidance, destination_paths, &mut reviewed_plan, &mut error, &mut screen),
            (KeyCode::Enter | KeyCode::Char('y'), Screen::Review) => {
                let review = reviewed_plan.take().ok_or_else(|| "selection matched no artifacts".to_string())?;
                if review.is_blocking() { error = Some("review has blocking diagnostics".into()); reviewed_plan = Some(review); continue; }
                return Ok(Some(review));
            }
            (KeyCode::Up, Screen::Review) => { review_scroll = review_scroll.saturating_sub(1); }
            (KeyCode::Down, Screen::Review) => { review_scroll = review_scroll.saturating_add(1); }
            (KeyCode::Char('r'), Screen::Review) => { root_text = reviewed_plan.as_ref().map(|plan| plan.install.root.display().to_string()).unwrap_or_default(); screen = Screen::RootEdit; }
            (KeyCode::Esc, Screen::RootEdit) => { screen = Screen::Review; }
            (KeyCode::Backspace, Screen::RootEdit) => { root_text.pop(); }
            (KeyCode::Char(c), Screen::RootEdit) => root_text.push(c),
            (KeyCode::Enter, Screen::RootEdit) => {
                let candidate = root_text.trim();
                let path = std::path::PathBuf::from(candidate);
                if candidate.is_empty() || candidate.contains('\0') || (path.exists() && !path.is_dir()) { error = Some("binary root must be a nonempty directory path".into()); }
                else { state.set_root(path); prepare_composed_review(&state, guidance_inputs, &selected_guidance, destination_paths, &mut reviewed_plan, &mut error, &mut screen); }
            }
            (KeyCode::Char('n'), Screen::Review) => return Ok(None),
            _ => {}
        }

        fn prepare_composed_review(state: &SelectionState, inputs: &[GuidanceInput], selected: &[bool], paths: &DestinationPaths, review: &mut Option<ReviewPlan>, error: &mut Option<String>, screen: &mut Screen) {
            let guidance: Vec<GuidanceInput> = inputs.iter().zip(selected).filter(|(_, checked)| **checked).map(|(input, _)| GuidanceInput { source_root: input.source_root.clone(), profile_path: input.profile_path.clone(), select: input.select.clone(), target_root: input.target_root.clone(), scope_override: input.scope_override, explicit_override: input.explicit_override.clone() }).collect();
            match state.prepare_composed_review(&guidance, paths) { Ok(plan) => { *review = Some(plan); *error = None; *screen = Screen::Review; }, Err(message) => *error = Some(message) }
        }
    }
}

fn row(index: usize, cursor: usize, checked: bool, label: String) -> ListItem<'static> {
    let marker = if checked { "x" } else { " " };
    let style = if index == cursor { Style::default().fg(Color::Yellow) } else { Style::default() };
    ListItem::new(format!("[{marker}] {label}")).style(style)
}

fn wrap_review_row(label: &str, value: &str, width: u16) -> Vec<ListItem<'static>> {
    let width = usize::from(width.saturating_sub(4)).max(1);
    let text = format!("{label}{value}");
    text.chars()
        .collect::<Vec<_>>()
        .chunks(width)
        .map(|chunk| ListItem::new(chunk.iter().collect::<String>()))
        .collect()
}
