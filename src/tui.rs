//! Read-only-first TUI for NeoSense.
//!
//! The TUI never writes fan state directly. The only control action currently
//! exposed is a confirmed `fan.auto` request through the supervisor IPC socket;
//! if the supervisor is unavailable, the interface stays read-only.

use crate::custom_profiles::{CustomProfile, write_private_file};
use crate::diagnostics::{CheckGroup, CheckStatus, DiagnosticReport, diagnose};
use crate::ipc::Client;
use crate::model::{SensorSample, now_ms};
use crate::nbfc_config;
use crate::policy::{FanRole, fan_role};
use crate::presets;
use crate::profiles::{ProfileAction, ProfileContext, ProfileOrigin, evaluate_all_with};
use crate::regulation::{RegulationMode, evaluate_profile};
use crate::runtime::{RuntimeView, fan_observations, host_info, read_view};
use crate::supervisor::{SupervisorProbe, SupervisorState};
use crate::warnings::{Severity, from_view as warnings_from_view};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Tabs, Wrap};
use serde_json::Value;
use std::io::{IsTerminal, stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Dashboard,
    Sensors,
    Fans,
    Profiles,
    System,
    Diagnostics,
    Presets,
}

impl Tab {
    const ALL: [Tab; 7] = [
        Tab::Dashboard,
        Tab::Sensors,
        Tab::Fans,
        Tab::Profiles,
        Tab::System,
        Tab::Diagnostics,
        Tab::Presets,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    fn title(self) -> &'static str {
        match self {
            Tab::Dashboard => "Dashboard",
            Tab::Sensors => "Sensors",
            Tab::Fans => "Fans",
            Tab::Profiles => "Profiles",
            Tab::System => "System",
            Tab::Diagnostics => "Diagnostics",
            Tab::Presets => "Presets",
        }
    }

    fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    fn previous(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

pub struct TuiApp {
    tab: Tab,
    view: RuntimeView,
    supervisor: SupervisorProbe,
    diagnostics: Option<DiagnosticReport>,
    socket: Option<PathBuf>,
    last_refresh: Instant,
    refresh_interval: Duration,
    message: Option<String>,
    show_help: bool,
    pending_auto: bool,
    profile_index: usize,
    pending_profile: Option<usize>,
    preset_index: usize,
    pending_preset: Option<usize>,
    pending_editor: Option<EditorRequest>,
    scroll: u16,
    should_quit: bool,
}

#[derive(Debug, Clone)]
enum EditorAction {
    Create,
    Update(String),
}

#[derive(Debug, Clone)]
struct EditorRequest {
    path: PathBuf,
    action: EditorAction,
}

impl TuiApp {
    pub fn new(socket: Option<PathBuf>) -> Self {
        let socket_path = socket
            .clone()
            .unwrap_or_else(crate::daemon::socket_path_from_env);
        let view = read_view(&socket_path);
        let supervisor = view.supervisor.clone();
        let mut app = Self {
            tab: Tab::Dashboard,
            view,
            supervisor,
            diagnostics: None,
            socket: Some(socket_path),
            last_refresh: Instant::now(),
            refresh_interval: Duration::from_secs(2),
            message: None,
            show_help: false,
            pending_auto: false,
            profile_index: 0,
            pending_profile: None,
            preset_index: 0,
            pending_preset: None,
            pending_editor: None,
            scroll: 0,
            should_quit: false,
        };
        app.refresh_diagnostics();
        app
    }

    #[cfg(test)]
    fn from_view(view: RuntimeView) -> Self {
        Self {
            tab: Tab::Dashboard,
            view,
            supervisor: crate::supervisor::probe(std::path::Path::new(
                "/nonexistent-neosense-test.sock",
            )),
            diagnostics: None,
            socket: None,
            last_refresh: Instant::now(),
            refresh_interval: Duration::from_secs(2),
            message: None,
            show_help: false,
            pending_auto: false,
            profile_index: 0,
            pending_profile: None,
            preset_index: 0,
            pending_preset: None,
            pending_editor: None,
            scroll: 0,
            should_quit: false,
        }
    }

    fn custom_profiles(&self) -> Vec<CustomProfile> {
        self.supervisor
            .status
            .as_ref()
            .and_then(|status| status.get("custom"))
            .and_then(|custom| custom.get("profiles"))
            .and_then(|profiles| serde_json::from_value(profiles.clone()).ok())
            .unwrap_or_default()
    }

    fn evaluations(&self) -> Vec<crate::profiles::ProfileEvaluation> {
        evaluate_all_with(
            &ProfileContext::from_view(&self.view),
            &self.custom_profiles(),
        )
    }

    fn refresh(&mut self) {
        let socket = self
            .socket
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("/nonexistent-neosense-test.sock"));
        self.view = read_view(socket);
        self.supervisor = self.view.supervisor.clone();
        if self.tab == Tab::Diagnostics {
            self.refresh_diagnostics();
        }
    }

    fn refresh_diagnostics(&mut self) {
        if self.tab == Tab::Diagnostics {
            self.diagnostics = Some(diagnose(&self.view));
        }
    }

    fn select_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.scroll = 0;
        self.refresh_diagnostics();
    }

    fn maybe_refresh(&mut self) {
        if self.last_refresh.elapsed() >= self.refresh_interval && !self.show_help {
            self.refresh();
            self.last_refresh = Instant::now();
        }
    }

    fn handle_key(&mut self, code: KeyCode) {
        if self.show_help {
            match code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => self.show_help = false,
                _ => {}
            }
            return;
        }
        if self.pending_auto {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm_auto();
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.pending_auto = false;
                    self.message = Some("Automatic request cancelled".to_string());
                }
                _ => {}
            }
            return;
        }
        if let Some(index) = self.pending_profile {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.confirm_profile(index),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.pending_profile = None;
                    self.message = Some("Profile request cancelled".to_string());
                }
                _ => {}
            }
            return;
        }
        if let Some(index) = self.pending_preset {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.confirm_preset(index),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.pending_preset = None;
                    self.message = Some("Preset request cancelled".to_string());
                }
                _ => {}
            }
            return;
        }

        self.message = None;
        match code {
            KeyCode::Esc | KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Tab | KeyCode::Right => {
                let tab = self.tab.next();
                self.select_tab(tab);
            }
            KeyCode::BackTab | KeyCode::Left => {
                let tab = self.tab.previous();
                self.select_tab(tab);
            }
            KeyCode::Char('r') => {
                self.refresh();
                self.last_refresh = Instant::now();
                self.message = Some("Refreshed".to_string());
            }
            KeyCode::Up if self.tab == Tab::Profiles => {
                self.profile_index = self.profile_index.saturating_sub(1);
            }
            KeyCode::Down if self.tab == Tab::Profiles => {
                let count = self.evaluations().len();
                self.profile_index = (self.profile_index + 1).min(count.saturating_sub(1));
            }
            KeyCode::Enter if self.tab == Tab::Profiles => {
                let evaluations = self.evaluations();
                if let Some(evaluation) = evaluations.get(self.profile_index) {
                    self.pending_profile = Some(self.profile_index);
                    let verb = if evaluation.profile.curve.is_some() {
                        "Simulate"
                    } else {
                        "Apply"
                    };
                    self.message = Some(format!(
                        "{verb} {} ({})? y = yes, n = no",
                        evaluation.profile.name,
                        evaluation.compatibility.label()
                    ));
                } else {
                    self.message = Some("Profile no longer available".to_string());
                }
            }
            KeyCode::Char('e') if self.tab == Tab::Profiles => self.begin_edit_selected(),
            KeyCode::Char('n') if self.tab == Tab::Profiles => self.begin_new_custom(),
            KeyCode::Up if self.tab == Tab::Presets => {
                self.preset_index = self.preset_index.saturating_sub(1);
            }
            KeyCode::Down if self.tab == Tab::Presets => {
                let count = presets::builtins().len();
                self.preset_index = (self.preset_index + 1).min(count.saturating_sub(1));
            }
            KeyCode::Enter if self.tab == Tab::Presets => {
                let presets = presets::builtins();
                if let Some(preset) = presets.get(self.preset_index) {
                    self.pending_preset = Some(self.preset_index);
                    self.message = Some(format!(
                        "Apply fixed preset {} (CPU {:.0}%, GPU {:.0}%)? y/n",
                        preset.name, preset.cpu_percent, preset.gpu_percent
                    ));
                }
            }
            KeyCode::Char('1') => self.select_tab(Tab::Dashboard),
            KeyCode::Char('2') => self.select_tab(Tab::Sensors),
            KeyCode::Char('3') => self.select_tab(Tab::Fans),
            KeyCode::Char('4') => self.select_tab(Tab::Profiles),
            KeyCode::Char('5') => self.select_tab(Tab::System),
            KeyCode::Char('6') => self.select_tab(Tab::Diagnostics),
            KeyCode::Char('7') => self.select_tab(Tab::Presets),
            KeyCode::PageUp if matches!(self.tab, Tab::System | Tab::Diagnostics) => {
                self.scroll = self.scroll.saturating_sub(5);
            }
            KeyCode::PageDown if matches!(self.tab, Tab::System | Tab::Diagnostics) => {
                self.scroll = self.scroll.saturating_add(5);
            }
            KeyCode::Home if matches!(self.tab, Tab::System | Tab::Diagnostics) => {
                self.scroll = 0;
            }
            KeyCode::Char('k') if matches!(self.tab, Tab::System | Tab::Diagnostics) => {
                self.scroll = self.scroll.saturating_sub(1);
            }
            KeyCode::Char('j') if matches!(self.tab, Tab::System | Tab::Diagnostics) => {
                self.scroll = self.scroll.saturating_add(1);
            }
            KeyCode::Char('a') => {
                if self.supervisor.state.is_connected() {
                    self.pending_auto = true;
                    self.message =
                        Some("Return to NBFC automatic mode? y = yes, n = no".to_string());
                } else {
                    self.message = Some("Supervisor unavailable: TUI is read-only".to_string());
                }
            }
            _ => {}
        }
    }

    fn editor_profile(&self, name: &str) -> Option<CustomProfile> {
        self.supervisor
            .status
            .as_ref()
            .and_then(|status| status.get("custom"))
            .and_then(|custom| custom.get("profiles"))
            .and_then(|profiles| {
                serde_json::from_value::<Vec<CustomProfile>>(profiles.clone()).ok()
            })
            .and_then(|profiles| profiles.into_iter().find(|profile| profile.name == name))
    }

    fn editor_path(&self, name: &str) -> PathBuf {
        crate::custom_profiles::editor_path(name)
    }

    fn begin_edit_selected(&mut self) {
        self.message = None;
        let evaluations = self.evaluations();
        let Some(evaluation) = evaluations.get(self.profile_index) else {
            self.message = Some("Profile no longer available".to_string());
            return;
        };
        if evaluation.profile.origin != ProfileOrigin::Custom {
            self.message =
                Some("Built-in curves are read-only; press n to create a custom curve".to_string());
            return;
        }
        if self.socket.is_none() || !self.supervisor.state.is_connected() {
            self.message = Some("Custom profiles are edited through the supervisor".to_string());
            return;
        }
        let name = evaluation.profile.name.clone();
        let Some(profile) = self.editor_profile(&name) else {
            self.message = Some("Custom definition is unavailable; press r to refresh".to_string());
            return;
        };
        let path = self.editor_path(&name);
        match serde_json::to_string_pretty(&profile)
            .map_err(|e| e.to_string())
            .and_then(|json| write_private_file(&path, &json).map_err(|e| e.to_string()))
        {
            Ok(()) => {
                self.message = Some(format!(
                    "Editing {name}; save and exit the editor to validate"
                ));
                self.pending_editor = Some(EditorRequest {
                    path,
                    action: EditorAction::Update(name),
                });
            }
            Err(error) => self.message = Some(format!("Cannot create editor file: {error}")),
        }
    }

    fn begin_new_custom(&mut self) {
        self.message = None;
        if self.socket.is_none() || !self.supervisor.state.is_connected() {
            self.message = Some("Custom profiles are created through the supervisor".to_string());
            return;
        }
        let existing: Vec<String> = self
            .custom_profiles()
            .into_iter()
            .map(|profile| profile.name)
            .collect();
        let mut name = "my-curve".to_string();
        let mut suffix = 2;
        while existing.contains(&name) {
            name = format!("my-curve-{suffix}");
            suffix += 1;
        }
        let profile = CustomProfile {
            name: name.clone(),
            title: "My curve".to_string(),
            description: "User-defined dynamic thermal curve.".to_string(),
            points: vec![
                crate::regulation::CurvePoint {
                    temperature_c: 40.0,
                    percent: 25.0,
                },
                crate::regulation::CurvePoint {
                    temperature_c: 60.0,
                    percent: 45.0,
                },
                crate::regulation::CurvePoint {
                    temperature_c: 80.0,
                    percent: 75.0,
                },
                crate::regulation::CurvePoint {
                    temperature_c: 90.0,
                    percent: 100.0,
                },
            ],
            min_percent: 20.0,
            max_percent: 100.0,
            hysteresis_c: 3.0,
            max_step_percent: 10.0,
            stale_after_ms: 3_000,
            fan_roles: vec![FanRole::Cpu, FanRole::Gpu],
        };
        let path = self.editor_path(&name);
        match serde_json::to_string_pretty(&profile)
            .map_err(|e| e.to_string())
            .and_then(|json| write_private_file(&path, &json).map_err(|e| e.to_string()))
        {
            Ok(()) => {
                self.message = Some(
                    "Edit the new curve, then save and exit to validate and create".to_string(),
                );
                self.pending_editor = Some(EditorRequest {
                    path,
                    action: EditorAction::Create,
                });
            }
            Err(error) => self.message = Some(format!("Cannot create editor file: {error}")),
        }
    }

    fn finish_editor(&mut self, request: EditorRequest) {
        let text = match std::fs::read_to_string(&request.path) {
            Ok(text) => text,
            Err(error) => {
                self.message = Some(format!("Cannot read edited profile: {error}"));
                return;
            }
        };
        let _ = std::fs::remove_file(&request.path);
        let profile: CustomProfile = match serde_json::from_str(&text) {
            Ok(profile) => profile,
            Err(error) => {
                self.message = Some(format!("Edited profile is not valid JSON: {error}"));
                return;
            }
        };
        if let Err(error) = profile.validate() {
            self.message = Some(format!("Edited profile is invalid: {error}"));
            return;
        }
        if let EditorAction::Update(old_name) = &request.action {
            if profile.name != *old_name {
                self.message = Some(
                    "Renaming in the editor is not supported; use `profile custom rename`"
                        .to_string(),
                );
                return;
            }
        }
        let Some(socket) = self.socket.as_ref() else {
            self.message = Some("No supervisor socket configured".to_string());
            return;
        };
        let method = match &request.action {
            EditorAction::Create => "custom.create",
            EditorAction::Update(_) => "custom.update",
        };
        match Client::connect(socket) {
            Ok(mut client) => match client.call(method, serde_json::json!({"profile": profile})) {
                Ok(_) => {
                    self.message = Some(format!(
                        "Custom curve {} through the supervisor",
                        if matches!(request.action, EditorAction::Create) {
                            "created"
                        } else {
                            "updated"
                        }
                    ));
                    self.refresh();
                }
                Err(error) => self.message = Some(error.message),
            },
            Err(error) => self.message = Some(error.message),
        }
    }

    fn confirm_profile(&mut self, index: usize) {
        self.pending_profile = None;
        let evaluations = self.evaluations();
        let Some(evaluation) = evaluations.get(index) else {
            self.message = Some("Profile no longer available".to_string());
            return;
        };
        let Some(socket) = self.socket.as_ref() else {
            self.message = Some("No supervisor socket configured".to_string());
            return;
        };
        if evaluation.profile.action == ProfileAction::SafeAuto {
            match Client::connect(socket) {
                Ok(mut client) => match client.call("fan.auto", serde_json::json!({})) {
                    Ok(value) => {
                        let verified = value
                            .get("verified")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        self.message = Some(if verified {
                            "NBFC automatic mode confirmed".to_string()
                        } else {
                            "Automatic request not confirmed".to_string()
                        });
                        self.refresh();
                    }
                    Err(error) => self.message = Some(error.message),
                },
                Err(error) => self.message = Some(error.message),
            }
        } else if evaluation.profile.curve.is_some() {
            match Client::connect(socket) {
                Ok(mut client) => match client.call(
                    "profile.select",
                    serde_json::json!({"name": evaluation.profile.name}),
                ) {
                    Ok(value) => {
                        let simulated = value
                            .get("hardware_applied")
                            .and_then(Value::as_bool)
                            .map(|applied| !applied)
                            .unwrap_or(false);
                        self.message = Some(if simulated {
                            format!(
                                "{} selected for simulation; no fan write performed",
                                evaluation.profile.name
                            )
                        } else {
                            "Profile selection did not confirm simulation-only mode".to_string()
                        });
                        self.refresh();
                    }
                    Err(error) => self.message = Some(error.message),
                },
                Err(error) => self.message = Some(error.message),
            }
        } else {
            let mut message = format!("Profile unavailable: {}", evaluation.compatibility.label());
            if !evaluation.reasons.is_empty() {
                message.push_str(" — ");
                message.push_str(&evaluation.reasons.join("; "));
            }
            self.message = Some(message);
        }
    }

    fn confirm_preset(&mut self, index: usize) {
        self.pending_preset = None;
        let Some(preset) = presets::builtins().get(index).cloned() else {
            self.message = Some("Preset no longer available".to_string());
            return;
        };
        let Some(socket) = self.socket.as_ref() else {
            self.message = Some("No supervisor socket configured".to_string());
            return;
        };
        match Client::connect(socket) {
            Ok(mut client) => match client.call(
                "preset.apply",
                serde_json::json!({"name": preset.name, "confirm": true}),
            ) {
                Ok(value) => {
                    let applied = value
                        .get("hardware_applied")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    self.message = Some(if applied {
                        format!("{} fixed preset applied and targets confirmed", preset.name)
                    } else {
                        format!("{} preset request was not hardware-applied", preset.name)
                    });
                    self.refresh();
                }
                Err(error) => self.message = Some(error.message),
            },
            Err(error) => self.message = Some(error.message),
        }
    }

    fn confirm_auto(&mut self) {
        self.pending_auto = false;
        let Some(socket) = self.socket.as_ref() else {
            self.message = Some("No supervisor socket configured".to_string());
            return;
        };
        match Client::connect(socket) {
            Ok(mut client) => match client.call("fan.auto", serde_json::json!({})) {
                Ok(value) => {
                    let verified = value
                        .get("verified")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    self.message = Some(if verified {
                        "NBFC automatic mode confirmed".to_string()
                    } else {
                        "Automatic request not confirmed".to_string()
                    });
                    self.refresh();
                }
                Err(error) => self.message = Some(error.message),
            },
            Err(error) => self.message = Some(error.message),
        }
    }

    fn render(&mut self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let [header, tabs, body, footer] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Length(2),
            Constraint::Min(6),
            Constraint::Length(3),
        ])
        .areas(area);

        self.render_header(frame, header);
        self.render_tabs(frame, tabs);
        match self.tab {
            Tab::Dashboard => self.render_dashboard(frame, body),
            Tab::Sensors => self.render_sensors(frame, body),
            Tab::Fans => self.render_fans(frame, body),
            Tab::Profiles => self.render_profiles(frame, body),
            Tab::System => self.render_system(frame, body),
            Tab::Diagnostics => self.render_diagnostics(frame, body),
            Tab::Presets => self.render_presets(frame, body),
        }
        self.render_footer(frame, footer);
        if self.show_help {
            self.render_help(frame, area);
        }
    }

    fn render_header(&self, frame: &mut ratatui::Frame, area: Rect) {
        let host = host_info();
        let compact = area.width < 60;
        let host_label = if compact {
            host.model.as_deref().unwrap_or("unknown model").to_string()
        } else {
            format!(
                "{} {}",
                host.vendor.as_deref().unwrap_or("Machine"),
                host.model.as_deref().unwrap_or("unknown")
            )
        };
        let nbfc = self
            .view
            .version
            .map(|v| format!("NBFC {v}"))
            .unwrap_or_else(|| "NBFC unavailable".to_string());
        let (supervisor_color, supervisor_label) = supervisor_style(self.supervisor.state);
        let supervisor_label = if compact {
            supervisor_short(self.supervisor.state)
        } else {
            supervisor_label
        };
        let line_one = Line::from(vec![
            Span::styled(
                format!(" NeoSense {} ", crate::VERSION),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" · {host_label}")),
        ]);
        let line_two = Line::from(vec![
            Span::raw(format!(" {nbfc}  ·  ")),
            Span::styled(
                format!(
                    "{}: {supervisor_label}",
                    if compact { "sup" } else { "supervisor" }
                ),
                Style::default().fg(supervisor_color),
            ),
        ]);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));
        frame.render_widget(Paragraph::new(vec![line_one, line_two]).block(block), area);
    }

    fn render_tabs(&self, frame: &mut ratatui::Frame, area: Rect) {
        if area.width < 60 {
            let line = Line::from(vec![
                Span::styled(
                    self.tab.title(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(
                    "  ·  {}/{}  ·  Tab{}",
                    self.tab.index() + 1,
                    Tab::ALL.len(),
                    if matches!(self.tab, Tab::System | Tab::Diagnostics) {
                        "  ·  j/k"
                    } else {
                        ""
                    }
                )),
            ]);
            frame.render_widget(Paragraph::new(line), area);
            return;
        }
        let titles: Vec<Line> = Tab::ALL.iter().map(|tab| Line::from(tab.title())).collect();
        let tabs = Tabs::new(titles)
            .select(self.tab.index())
            .highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .divider(" · ");
        frame.render_widget(tabs, area);
    }

    fn render_dashboard(&self, frame: &mut ratatui::Frame, area: Rect) {
        let [top, fans] =
            Layout::vertical([Constraint::Percentage(58), Constraint::Percentage(42)]).areas(area);
        let (cpu_area, gpu_area) = if area.width < 80 {
            let [cpu, gpu] =
                Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .areas(top);
            (cpu, gpu)
        } else {
            let [cpu, gpu] =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .areas(top);
            (cpu, gpu)
        };

        let now = crate::model::now_ms();
        let cpu = self.view.snapshot.cpu_package(now, 5_000);
        let max_core = self
            .view
            .snapshot
            .max_core_c()
            .map(|v| format!("{v:.1} °C"))
            .unwrap_or_else(|| "n/a".into());
        let cpu_text = format!(
            "{}  {}\n{}\n{}\n\nMax core  {}\n{}",
            cpu.map(|s| s.display_value())
                .unwrap_or_else(|| "unavailable".into()),
            cpu.map(|s| format!("{:?}", s.health.trust))
                .unwrap_or_else(|| "no data".into()),
            cpu.map(|s| s.label.as_deref().unwrap_or(s.chip.as_str()))
                .unwrap_or("unknown provenance"),
            cpu.map(|s| s.path.as_deref().unwrap_or("-")).unwrap_or("-"),
            max_core,
            "Sensors CPU : hwmon coretemp / k10temp / thermal x86_pkg_temp",
        );
        frame.render_widget(
            Paragraph::new(cpu_text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" CPU ")
                        .border_style(Style::default().fg(Color::Cyan)),
                )
                .wrap(Wrap { trim: true }),
            cpu_area,
        );

        let gpu_text = if self.view.snapshot.gpu_indexes().is_empty() {
            "No GPU sensor identified.\n\nNo value is fabricated.\nThe GPU fan may be unavailable."
                .to_string()
        } else {
            self.view
                .snapshot
                .gpu_indexes()
                .into_iter()
                .map(|index| {
                    let die = self.view.snapshot.gpu_die(index, now, 5_000);
                    format!(
                        "GPU {index}  {}\n{}\n{}\n",
                        die.map(|s| s.display_value())
                            .unwrap_or_else(|| "unavailable".into()),
                        die.map(|s| format!("{:?}", s.health.trust))
                            .unwrap_or_else(|| "no data".into()),
                        die.and_then(|s| s.label.as_deref())
                            .unwrap_or("unknown provenance")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        frame.render_widget(
            Paragraph::new(gpu_text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" GPU ")
                        .border_style(Style::default().fg(Color::Magenta)),
                )
                .wrap(Wrap { trim: true }),
            gpu_area,
        );

        let mut lines = Vec::new();
        for warning in warnings_from_view(&self.view, Some(&self.supervisor))
            .into_iter()
            .take(3)
        {
            let color = match warning.severity {
                Severity::Critical => Color::Red,
                Severity::Warning => Color::Yellow,
                Severity::Info => Color::Cyan,
                Severity::Success => Color::Green,
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "[{}] {}: {}",
                    warning.severity.label(),
                    warning.title,
                    warning.detail
                ),
                Style::default().fg(color),
            )));
        }
        if let Some(status) = &self.view.status {
            for fan in &status.fans {
                let mode = match fan.auto_mode {
                    Some(true) => "auto",
                    Some(false) => "manual",
                    None => "unknown",
                };
                let target = fan
                    .target_speed
                    .map(|v| format!("{v:.1}%"))
                    .unwrap_or_else(|| "n/a".into());
                let current = fan
                    .current_speed
                    .map(|v| format!("{v:.1}%"))
                    .unwrap_or_else(|| "n/a".into());
                let rpm = self
                    .view
                    .snapshot
                    .sensors
                    .iter()
                    .find(|s| matches!(s.role, crate::model::SensorRole::FanRpm(_)))
                    .map(|s| s.display_value())
                    .unwrap_or_else(|| "n/a".into());
                let source = self
                    .view
                    .sensor_config
                    .as_ref()
                    .and_then(|config| config.fans.iter().find(|f| f.index == fan.index))
                    .map(|f| f.sensors.join(","))
                    .unwrap_or_else(|| "unknown".into());
                let implausible = matches!(
                    (fan.current_speed, fan.target_speed),
                    (Some(cur), Some(tgt)) if (cur - tgt).abs() > 20.0
                );
                lines.push(Line::from(format!(
                    "{:<10} {:<7} target {:<6} EC {:<6}{} RPM {:<8} source {}",
                    fan.name,
                    mode,
                    target,
                    current,
                    if implausible { " (!)" } else { "" },
                    rpm,
                    source
                )));
            }
        } else {
            lines.push(Line::from("NBFC status unavailable."));
        }
        if let Some(error) = &self.view.backend_error {
            lines.push(Line::from(Span::styled(
                format!("Error: {error}"),
                Style::default().fg(Color::Red),
            )));
        }
        if let Some(status) = &self.view.status {
            for warning in status.warnings.iter().take(2) {
                lines.push(Line::from(Span::styled(
                    warning.clone(),
                    Style::default().fg(Color::Yellow),
                )));
            }
        }
        if let Some(message) = &self.message {
            lines.push(Line::from(Span::styled(
                message.clone(),
                Style::default().fg(Color::Cyan),
            )));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Fans / state ")
                        .border_style(Style::default().fg(Color::Yellow)),
                )
                .wrap(Wrap { trim: true }),
            fans,
        );
    }

    fn render_sensors(&self, frame: &mut ratatui::Frame, area: Rect) {
        if self.view.snapshot.sensors.is_empty() {
            frame.render_widget(
                Paragraph::new(
                    "No sensors detected. Check hwmon/thermal access and run `neosense diagnose`.",
                )
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Discovered sensors "),
                )
                .wrap(Wrap { trim: true }),
                area,
            );
            return;
        }
        let compact = area.width < 70;
        let rows = self.view.snapshot.sensors.iter().map(|sensor| {
            if compact {
                Row::new(vec![
                    sensor.role.to_string(),
                    sensor.display_value(),
                    sensor.health.trust.short_label().to_string(),
                ])
            } else {
                sensor_row(sensor)
            }
        });
        let (header, widths) = if compact {
            (
                Row::new(["Role", "Value", "Trust"]),
                vec![
                    Constraint::Length(16),
                    Constraint::Length(10),
                    Constraint::Min(10),
                ],
            )
        } else {
            (
                Row::new(["Role", "Value", "Trust", "Status", "Source", "Path"]),
                vec![
                    Constraint::Length(18),
                    Constraint::Length(12),
                    Constraint::Length(11),
                    Constraint::Length(10),
                    Constraint::Length(9),
                    Constraint::Min(20),
                ],
            )
        };
        let table = Table::new(rows, widths)
            .header(
                header.style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            )
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Discovered sensors "),
            );
        frame.render_widget(table, area);
    }

    fn render_fans(&self, frame: &mut ratatui::Frame, area: Rect) {
        let compact = area.width < 70;
        let mut rows = Vec::new();
        if let Some(status) = &self.view.status {
            for fan in &status.fans {
                let source = self
                    .view
                    .sensor_config
                    .as_ref()
                    .and_then(|config| config.fans.iter().find(|f| f.index == fan.index))
                    .map(|f| f.sensors.join(","))
                    .unwrap_or_else(|| "unknown".into());
                if compact {
                    rows.push(Row::new(vec![
                        format!("#{}", fan.index),
                        fan.name.clone(),
                        fan_mode_label(fan.auto_mode).to_string(),
                        opt_percent(fan.target_speed),
                        opt_percent(fan.current_speed),
                    ]));
                } else {
                    rows.push(Row::new(vec![
                        format!("#{}", fan.index),
                        fan.name.clone(),
                        fan_role(&fan.name).to_string(),
                        fan_mode_label(fan.auto_mode).to_string(),
                        opt_percent(fan.requested_speed),
                        opt_percent(fan.target_speed),
                        opt_percent(fan.current_speed),
                        source,
                    ]));
                }
            }
        }
        if rows.is_empty() {
            frame.render_widget(
                Paragraph::new("No NBFC fan data. Start nbfc_service and run `neosense diagnose`.")
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" NBFC fans — EC is not RPM "),
                    )
                    .wrap(Wrap { trim: true }),
                area,
            );
            return;
        }
        let (header, widths) = if compact {
            (
                Row::new(["#", "Name", "Mode", "Target", "EC"]),
                vec![
                    Constraint::Length(3),
                    Constraint::Length(12),
                    Constraint::Length(7),
                    Constraint::Length(8),
                    Constraint::Min(6),
                ],
            )
        } else {
            (
                Row::new([
                    "Index",
                    "Name",
                    "Role",
                    "Mode",
                    "Requested",
                    "Target",
                    "EC",
                    "Source",
                ]),
                vec![
                    Constraint::Length(6),
                    Constraint::Length(14),
                    Constraint::Length(8),
                    Constraint::Length(8),
                    Constraint::Length(9),
                    Constraint::Length(9),
                    Constraint::Length(9),
                    Constraint::Min(10),
                ],
            )
        };
        let table = Table::new(rows, widths)
            .header(
                header.style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            )
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" NBFC fans — EC is not RPM "),
            );
        frame.render_widget(table, area);
    }

    fn render_profiles(&self, frame: &mut ratatui::Frame, area: Rect) {
        let compact = area.width < 70;
        let short_compatibility = area.width < 100;
        let evaluations = self.evaluations();
        let selected = self.profile_index.min(evaluations.len().saturating_sub(1));
        let observations = fan_observations(&self.view);
        let rows = evaluations.iter().enumerate().map(|(index, evaluation)| {
            let style = if index == selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let target = evaluation
                .profile
                .curve
                .as_ref()
                .and_then(|curve| {
                    let observations: Vec<_> = observations
                        .iter()
                        .filter(|fan| {
                            evaluation
                                .profile
                                .fan_roles
                                .as_ref()
                                .is_none_or(|roles| roles.contains(&fan.role))
                        })
                        .cloned()
                        .collect();
                    evaluate_profile(
                        curve,
                        &self.view.snapshot,
                        &observations,
                        RegulationMode::Preview,
                        now_ms(),
                    )
                    .ok()
                })
                .and_then(|targets| targets.into_iter().find(|target| target.available))
                .and_then(|target| target.target_percent)
                .map(|percent| format!("{percent:.1}%"))
                .unwrap_or_else(|| "n/a".to_string());
            if compact {
                Row::new(vec![
                    if evaluation.profile.origin == ProfileOrigin::Custom {
                        format!("{}*", evaluation.profile.name)
                    } else {
                        evaluation.profile.name.to_string()
                    },
                    if short_compatibility {
                        evaluation.compatibility.short_label().to_string()
                    } else {
                        evaluation.compatibility.label().to_string()
                    },
                    target,
                ])
                .style(style)
            } else {
                Row::new(vec![
                    format!(
                        "{} ({})",
                        evaluation.profile.name,
                        match evaluation.profile.origin {
                            ProfileOrigin::Builtin => "built-in",
                            ProfileOrigin::Custom => "custom",
                        }
                    ),
                    evaluation.profile.title.to_string(),
                    if short_compatibility {
                        evaluation.compatibility.short_label().to_string()
                    } else {
                        evaluation.compatibility.label().to_string()
                    },
                    target,
                    evaluation.profile.reason.clone().unwrap_or_default(),
                ])
                .style(style)
            }
        });
        let (header, widths) = if compact {
            (
                Row::new(["Name", "Compat.", "Target"]),
                vec![
                    Constraint::Length(14),
                    Constraint::Length(12),
                    Constraint::Length(8),
                ],
            )
        } else {
            (
                Row::new(["Name", "Title", "Compatibility", "Target", "Notes"]),
                vec![
                    Constraint::Length(12),
                    Constraint::Length(18),
                    Constraint::Length(20),
                    Constraint::Length(8),
                    Constraint::Min(18),
                ],
            )
        };
        let table = Table::new(rows, widths)
            .header(
                header.style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            )
            .block(
                Block::default().borders(Borders::ALL).title(
                    if let Some(profile) = self
                        .supervisor
                        .status
                        .as_ref()
                        .and_then(|status| status.get("profile"))
                        .and_then(|profile| profile.get("selected"))
                        .and_then(Value::as_str)
                    {
                        format!(" Profiles — {profile} (simulated) ")
                    } else if area.width < 70 {
                        " Profiles — Enter to simulate ".to_string()
                    } else {
                        " Thermal profiles — Enter simulate, e edit custom, n new ".to_string()
                    },
                ),
            );
        frame.render_widget(table, area);
    }

    fn render_presets(&self, frame: &mut ratatui::Frame, area: Rect) {
        let presets = presets::builtins();
        let selected = self.preset_index.min(presets.len().saturating_sub(1));
        let compact = area.width < 70;
        let rows = presets.iter().enumerate().map(|(index, preset)| {
            let style = if index == selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            if compact {
                Row::new(vec![
                    preset.name.to_string(),
                    format!("{:.0}%", preset.cpu_percent),
                    format!("{:.0}%", preset.gpu_percent),
                ])
                .style(style)
            } else {
                Row::new(vec![
                    preset.name.to_string(),
                    preset.title.to_string(),
                    format!("{:.0}%", preset.cpu_percent),
                    format!("{:.0}%", preset.gpu_percent),
                ])
                .style(style)
            }
        });
        let (header, widths) = if compact {
            (
                Row::new(["Name", "CPU", "GPU"]),
                vec![
                    Constraint::Length(14),
                    Constraint::Length(8),
                    Constraint::Min(8),
                ],
            )
        } else {
            (
                Row::new(["Name", "Title", "CPU", "GPU"]),
                vec![
                    Constraint::Length(14),
                    Constraint::Length(22),
                    Constraint::Length(8),
                    Constraint::Min(8),
                ],
            )
        };
        let table = Table::new(rows, widths).header(
            header.style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        );
        frame.render_widget(
            table.block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Fixed presets — Up/Down, Enter, y/n "),
            ),
            area,
        );
    }

    fn render_system(&self, frame: &mut ratatui::Frame, area: Rect) {
        let mut lines = Vec::new();
        lines.push(Line::from(format!(
            "NBFC service: {} / {}",
            self.view
                .service
                .active_state
                .as_deref()
                .unwrap_or("unknown"),
            self.view
                .service
                .unit_file_state
                .as_deref()
                .unwrap_or("unknown")
        )));
        if let Some(error) = &self.view.service.error {
            lines.push(Line::from(Span::styled(
                format!("Service error: {error}"),
                Style::default().fg(Color::Red),
            )));
        }
        if let Some(error) = &self.view.backend_error {
            lines.push(Line::from(Span::styled(
                format!("Backend error: {error}"),
                Style::default().fg(Color::Red),
            )));
        }
        if let Some(selected) = self
            .view
            .status
            .as_ref()
            .and_then(|status| status.selected_config.as_deref())
            && let Ok(identity) =
                nbfc_config::identity_report(selected, self.view.sensor_config.as_ref())
        {
            lines.push(Line::from(format!(
                "NBFC identity: {} logical channel(s) in {}",
                identity.fans.len(),
                identity.selected_config
            )));
            for fan in &identity.fans {
                lines.push(Line::from(format!(
                    "  {}: role {}, {}, sensors {}, readback {}",
                    fan.name,
                    fan.role.to_string().to_ascii_lowercase(),
                    fan.register_pair,
                    if fan.sensor_sources.is_empty() {
                        "none".to_string()
                    } else {
                        fan.sensor_sources.join(",")
                    },
                    if fan.readback_configured {
                        "configured"
                    } else {
                        "invalid"
                    }
                )));
            }
        }
        lines.push(Line::from(format!(
            "Supervisor: {}",
            self.supervisor.state.label()
        )));
        if let Some(health) = self.supervisor.health {
            lines.push(Line::from(format!(
                "Health: {}",
                if health { "healthy" } else { "degraded" }
            )));
        }
        if let Some(reason) = &self.supervisor.reason {
            lines.push(Line::from(format!("Reason: {reason}")));
        }
        if let Some(error) = &self.supervisor.error {
            lines.push(Line::from(format!("IPC detail: {error}")));
        }
        if let Some(custom) = self
            .supervisor
            .status
            .as_ref()
            .and_then(|status| status.get("custom"))
        {
            lines.push(Line::from(format!(
                "Custom profiles: {} ({})",
                custom
                    .get("profiles")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0),
                custom
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("path unavailable")
            )));
            if let Some(error) = custom.get("error").and_then(Value::as_str) {
                lines.push(Line::from(Span::styled(
                    format!("Custom store error: {error}"),
                    Style::default().fg(Color::Red),
                )));
            }
            for warning in custom
                .get("warnings")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                lines.push(Line::from(Span::styled(
                    format!("Custom store warning: {warning}"),
                    Style::default().fg(Color::Yellow),
                )));
            }
        }
        lines.push(Line::from(format!(
            "Socket: {} (mode {}, uid {}, gid {})",
            self.supervisor.socket.display(),
            self.supervisor.socket_mode.as_deref().unwrap_or("n/a"),
            self.supervisor
                .socket_uid
                .map(|v| v.to_string())
                .unwrap_or_else(|| "n/a".into()),
            self.supervisor
                .socket_gid
                .map(|v| v.to_string())
                .unwrap_or_else(|| "n/a".into())
        )));
        lines.push(Line::from(format!(
            "Next: {}",
            self.supervisor.state.action()
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Warnings",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        let warnings = warnings_from_view(&self.view, Some(&self.supervisor));
        if warnings.is_empty() {
            lines.push(Line::from(Span::styled(
                "[OK] no active warnings",
                Style::default().fg(Color::Green),
            )));
        }
        for warning in warnings {
            let color = match warning.severity {
                Severity::Critical => Color::Red,
                Severity::Warning => Color::Yellow,
                Severity::Info => Color::Cyan,
                Severity::Success => Color::Green,
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("[{}] ", warning.severity.label()),
                    Style::default().fg(color),
                ),
                Span::styled(warning.title, Style::default().fg(color)),
            ]));
            lines.push(Line::from(format!(
                "  {} — next: {}",
                warning.detail, warning.action
            )));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" System / supervisor "),
                )
                .wrap(Wrap { trim: true })
                .scroll((self.scroll, 0)),
            area,
        );
    }

    fn render_diagnostics(&self, frame: &mut ratatui::Frame, area: Rect) {
        let Some(report) = &self.diagnostics else {
            frame.render_widget(
                Paragraph::new("Diagnostics not loaded. Press r.").block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Diagnostics "),
                ),
                area,
            );
            return;
        };
        let mut lines = Vec::new();
        lines.push(Line::from(format!(
            "{} · kernel {} · {} {}",
            report.environment.os.as_deref().unwrap_or("unknown OS"),
            report.environment.kernel.as_deref().unwrap_or("?"),
            report.environment.vendor.as_deref().unwrap_or(""),
            report.environment.model.as_deref().unwrap_or("")
        )));
        let summary = &report.summary;
        lines.push(Line::from(vec![
            Span::styled("Operational: ", Style::default().fg(Color::Cyan)),
            Span::styled(
                if summary.operational { "yes" } else { "no" },
                Style::default().fg(if summary.operational {
                    Color::Green
                } else {
                    Color::Red
                }),
            ),
            Span::raw(format!(
                "  ·  Supervisor: {}  ·  Fan control: {}  ·  Thermal: {}",
                summary.supervisor.label(),
                if summary.fan_control_available {
                    "available"
                } else {
                    "blocked"
                },
                if summary.thermal_reliable {
                    "reliable"
                } else {
                    "degraded"
                }
            )),
        ]));
        lines.push(Line::from(format!(
            "Critical: {}  ·  Warnings: {}  ·  Overall: {:?}",
            summary.critical_issues, summary.warning_issues, summary.overall
        )));

        let criticals: Vec<_> = report
            .checks
            .iter()
            .filter(|check| check.status == CheckStatus::Fail)
            .collect();
        if !criticals.is_empty() {
            lines.push(Line::from(Span::styled(
                "Critical issues / blockers",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
            for check in criticals {
                push_check(&mut lines, check);
            }
        }

        for group in [
            CheckGroup::Backend,
            CheckGroup::Hardware,
            CheckGroup::Thermal,
            CheckGroup::Fans,
            CheckGroup::Control,
            CheckGroup::Security,
        ] {
            let checks: Vec<_> = report
                .checks_in(group)
                .filter(|check| check.status != CheckStatus::Fail)
                .collect();
            if checks.is_empty() {
                continue;
            }
            lines.push(Line::from(Span::styled(
                group.label(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            for check in checks {
                push_check(&mut lines, check);
            }
        }
        lines.push(Line::from(
            "Use `neosense diagnose --json` for the complete machine-readable report.",
        ));
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Diagnostics "),
                )
                .wrap(Wrap { trim: true })
                .scroll((self.scroll, 0)),
            area,
        );
    }

    fn render_footer(&self, frame: &mut ratatui::Frame, area: Rect) {
        let mut line = if area.width < 60 {
            " q · Tab · r · a auto · ? ".to_string()
        } else if area.width < 100 {
            " q quit · Tab views · r · a auto · ? ".to_string()
        } else {
            " q quit  ·  Tab/←→ views  ·  r refresh  ·  a auto  ·  ? help ".to_string()
        };
        if self.pending_auto {
            line = if area.width < 70 {
                " Apply NBFC auto? y/n ".to_string()
            } else {
                " Return to NBFC automatic mode: y confirm  ·  n cancel ".to_string()
            };
        } else if self.pending_preset.is_some() {
            line = if area.width < 60 {
                " Apply fixed preset? y/n ".to_string()
            } else {
                format!(
                    " {} ",
                    self.message
                        .as_deref()
                        .unwrap_or("Apply fixed preset? y = yes, n = no")
                )
            };
        } else if self.pending_profile.is_some() {
            line = if area.width < 60 {
                " Apply profile? y/n ".to_string()
            } else {
                format!(
                    " {} ",
                    self.message
                        .as_deref()
                        .unwrap_or("Confirm profile? y = yes, n = no")
                )
            };
        } else if let Some(message) = &self.message {
            line = format!(
                " {} ",
                truncate_for_width(message, area.width.saturating_sub(2) as usize)
            );
        }
        if !self.pending_auto
            && self.pending_profile.is_none()
            && self.pending_preset.is_none()
            && self.message.is_none()
        {
            if !self.supervisor.state.is_connected() {
                line.push_str(if area.width < 60 {
                    " · RO"
                } else {
                    "  ·  read-only "
                });
            }
            if area.width >= 70 && matches!(self.tab, Tab::System | Tab::Diagnostics) {
                line.push_str(if area.width < 100 {
                    "  ·  PgUp/Dn"
                } else {
                    "  ·  PgUp/PgDn scroll"
                });
            }
        }
        frame.render_widget(
            Paragraph::new(line)
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().borders(Borders::ALL)),
            area,
        );
    }

    fn render_help(&self, frame: &mut ratatui::Frame, area: Rect) {
        let popup = centered_rect(72, 60, area);
        frame.render_widget(Clear, popup);
        let text = vec![
            Line::from(Span::styled(
                "NeoSense — help",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("q / Esc         quit"),
            Line::from("Tab / arrows    change view"),
            Line::from("1–6             dashboard, sensors, fans, profiles, system, diagnostics"),
            Line::from("7               fixed presets"),
            Line::from("e               edit the selected custom curve ($EDITOR)"),
            Line::from("n               create a custom curve, then edit it in $EDITOR"),
            Line::from("r               refresh now"),
            Line::from("a               return to NBFC automatic mode (confirmation)"),
            Line::from("PgUp/PgDn, j/k   scroll System and Diagnostics"),
            Line::from("Home            scroll to top"),
            Line::from("? / Esc         close help"),
            Line::from(""),
            Line::from("EC speeds are not RPM. The TUI stays read-only without a supervisor."),
        ];
        frame.render_widget(
            Paragraph::new(text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Help ")
                        .border_style(Style::default().fg(Color::Cyan)),
                )
                .wrap(Wrap { trim: true }),
            popup,
        );
    }
}

fn truncate_for_width(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let mut output: String = text.chars().take(width - 3).collect();
    output.push_str("...");
    output
}

fn fan_mode_label(mode: Option<bool>) -> &'static str {
    match mode {
        Some(true) => "auto",
        Some(false) => "manual",
        None => "unknown",
    }
}

fn supervisor_short(state: SupervisorState) -> &'static str {
    match state {
        SupervisorState::Healthy => "healthy",
        SupervisorState::Connected => "connected",
        SupervisorState::Degraded => "degraded",
        SupervisorState::NotRunning => "offline",
        SupervisorState::PermissionDenied => "no access",
        SupervisorState::StaleSocket => "stale",
        SupervisorState::ProtocolError => "protocol",
    }
}

fn supervisor_style(state: SupervisorState) -> (Color, &'static str) {
    match state {
        SupervisorState::Healthy => (Color::Green, "healthy"),
        SupervisorState::Connected => (Color::Cyan, "connected"),
        SupervisorState::Degraded => (Color::Yellow, "degraded"),
        SupervisorState::NotRunning => (Color::Red, "not running"),
        SupervisorState::PermissionDenied => (Color::Red, "permission denied"),
        SupervisorState::StaleSocket => (Color::Red, "stale socket"),
        SupervisorState::ProtocolError => (Color::Red, "protocol error"),
    }
}

fn push_check(lines: &mut Vec<Line<'static>>, check: &crate::diagnostics::DiagnosticCheck) {
    let (label, color) = match check.status {
        CheckStatus::Ok => ("OK  ", Color::Green),
        CheckStatus::Info => ("INFO", Color::Cyan),
        CheckStatus::Warning => ("WARN", Color::Yellow),
        CheckStatus::Fail => ("CRIT", Color::Red),
    };
    lines.push(Line::from(vec![
        Span::styled(format!("[{label}] "), Style::default().fg(color)),
        Span::raw(format!("{:<26} {}", check.title, check.message)),
    ]));
}

fn sensor_row(sensor: &SensorSample) -> Row<'static> {
    Row::new(vec![
        Cell::from(sensor.role.to_string()),
        Cell::from(sensor.display_value()),
        Cell::from(format!("{:?}", sensor.health.trust)),
        Cell::from(format!("{:?}", sensor.status)),
        Cell::from(sensor.provider.to_string()),
        Cell::from(
            sensor
                .path
                .as_deref()
                .or(sensor.label.as_deref())
                .unwrap_or("-")
                .to_string(),
        ),
    ])
}

fn opt_percent(value: Option<f64>) -> String {
    value
        .filter(|v| *v >= 0.0)
        .map(|v| format!("{v:.1}%"))
        .unwrap_or_else(|| "n/a".into())
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let [middle] = Layout::vertical([Constraint::Percentage(percent_y)])
        .flex(ratatui::layout::Flex::Center)
        .areas(r);
    let [area] = Layout::horizontal([Constraint::Percentage(percent_x)])
        .flex(ratatui::layout::Flex::Center)
        .areas(middle);
    area
}

pub fn run(socket: Option<PathBuf>) -> Result<(), String> {
    if !stdout().is_terminal() {
        return Err("the TUI requires an interactive terminal".to_string());
    }
    enable_raw_mode().map_err(|e| format!("raw mode: {e}"))?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen).map_err(|e| format!("alternate screen: {e}"))?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = disable_raw_mode();
            let _ = execute!(stdout(), LeaveAlternateScreen);
            return Err(format!("terminal: {error}"));
        }
    };
    let mut app = TuiApp::new(socket);

    let result = run_loop(&mut terminal, &mut app);
    let _ = disable_raw_mode();
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .map_err(|e| format!("leave alternate screen: {e}"))?;
    let _ = terminal.show_cursor();
    result
}

fn run_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut TuiApp) -> Result<(), String> {
    loop {
        terminal
            .draw(|frame| app.render(frame))
            .map_err(|e| format!("TUI draw: {e}"))?;
        app.maybe_refresh();
        if app.should_quit {
            return Ok(());
        }
        if event::poll(Duration::from_millis(100)).map_err(|e| format!("TUI event: {e}"))? {
            match event::read().map_err(|e| format!("event read: {e}"))? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key.code),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
        if let Some(request) = app.pending_editor.take() {
            let editor = std::env::var("VISUAL")
                .or_else(|_| std::env::var("EDITOR"))
                .unwrap_or_else(|_| "vi".to_string());
            let _ = disable_raw_mode();
            let _ = execute!(stdout(), LeaveAlternateScreen);
            let _ = terminal.show_cursor();
            let status = std::process::Command::new(&editor)
                .arg(&request.path)
                .status();
            let _ = enable_raw_mode();
            let _ = execute!(stdout(), EnterAlternateScreen);
            let _ = terminal.clear();
            match status {
                Ok(status) if status.success() => app.finish_editor(request),
                Ok(_) => {
                    let _ = std::fs::remove_file(&request.path);
                    app.message = Some(format!("Editor '{editor}' exited unsuccessfully"));
                }
                Err(error) => {
                    let _ = std::fs::remove_file(&request.path);
                    app.message = Some(format!("Cannot run editor '{editor}': {error}"));
                }
            }
        }
    }
}

// The helper is kept here to make the TUI render tests independent from the
// real terminal and hardware.
#[cfg(test)]
fn buffer_text(terminal: &Terminal<ratatui::backend::TestBackend>) -> String {
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SensorSnapshot;
    use crate::nbfc::{NbfcFan, NbfcStatus};
    use crate::service::ServiceStatus;

    fn empty_view() -> RuntimeView {
        RuntimeView {
            snapshot: SensorSnapshot::empty(),
            version: None,
            capabilities: None,
            status: Some(NbfcStatus {
                version: None,
                read_only: Some(false),
                selected_config: Some("test".into()),
                fans: vec![NbfcFan {
                    index: 0,
                    name: "CPU fan".into(),
                    temperature: Some(50.0),
                    critical_temperature: Some(95.0),
                    auto_mode: Some(true),
                    critical: Some(false),
                    current_speed: Some(0.0),
                    target_speed: Some(0.0),
                    requested_speed: Some(-1.0),
                    speed_steps: Some(100),
                }],
                warnings: Vec::new(),
            }),
            service: ServiceStatus::default(),
            sensor_config: None,
            backend_error: None,
            supervisor: crate::supervisor::probe(std::path::Path::new(
                "/nonexistent-neosense-test.sock",
            )),
        }
    }

    fn render_once(app: &mut TuiApp) -> String {
        render_size(app, 100, 32)
    }

    fn render_size(app: &mut TuiApp, width: u16, height: u16) -> String {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        buffer_text(&terminal)
    }

    #[test]
    fn dashboard_and_tabs_render() {
        let mut app = TuiApp::from_view(empty_view());
        let text = render_once(&mut app);
        assert!(text.contains("NeoSense"));
        assert!(text.contains("Dashboard"));
        for tab in Tab::ALL {
            app.tab = tab;
            let text = render_once(&mut app);
            assert!(!text.is_empty());
        }
    }

    #[test]
    fn tab_navigation_and_auto_confirmation() {
        let mut app = TuiApp::from_view(empty_view());
        app.handle_key(KeyCode::Tab);
        assert_eq!(app.tab, Tab::Sensors);
        app.handle_key(KeyCode::BackTab);
        assert_eq!(app.tab, Tab::Dashboard);
        app.handle_key(KeyCode::Char('a'));
        assert!(!app.pending_auto); // no supervisor in fixture
        app.show_help = true;
        app.handle_key(KeyCode::Esc);
        assert!(!app.show_help);
    }

    #[test]
    fn small_and_large_terminal_sizes_render() {
        let mut app = TuiApp::from_view(empty_view());
        for (width, height) in [(80, 24), (40, 20), (160, 48)] {
            for tab in Tab::ALL {
                app.tab = tab;
                let text = render_size(&mut app, width, height);
                assert!(!text.is_empty(), "empty render at {width}x{height}");
            }
        }
    }

    #[test]
    fn profile_selection_requires_explicit_confirmation() {
        let mut app = TuiApp::from_view(empty_view());
        app.tab = Tab::Profiles;
        app.handle_key(KeyCode::Down);
        assert_eq!(app.profile_index, 1);
        app.handle_key(KeyCode::Enter);
        assert_eq!(app.pending_profile, Some(1));
        app.handle_key(KeyCode::Char('n'));
        assert_eq!(app.pending_profile, None);
    }

    #[test]
    fn preview_profile_confirmation_does_not_write() {
        let mut app = TuiApp::from_view(empty_view());
        app.tab = Tab::Profiles;
        app.profile_index = 1; // quiet
        app.handle_key(KeyCode::Enter);
        app.handle_key(KeyCode::Char('y'));
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("No supervisor socket configured")
        );
    }

    #[test]
    fn profile_table_shows_a_preview_target() {
        let mut app = TuiApp::from_view(empty_view());
        app.tab = Tab::Profiles;
        let text = render_size(&mut app, 100, 32);
        assert!(text.contains("Target"));
        assert!(text.contains("n/a") || text.contains('%'));
    }

    #[test]
    fn header_never_leaks_html_entities_or_full_ipc_errors() {
        let mut app = TuiApp::from_view(empty_view());
        app.supervisor.state = SupervisorState::PermissionDenied;
        app.supervisor.error = Some("&#x20; very long IPC detail ".repeat(20));
        for width in [40, 80, 100, 160] {
            let text = render_size(&mut app, width, 32);
            assert!(!text.contains("&#x20;"), "HTML entity at width {width}");
            assert!(
                !text.contains("very long IPC detail"),
                "full IPC error leaked into header at width {width}"
            );
            assert!(text.contains("NeoSense"));
        }
    }

    #[test]
    fn empty_sensor_state_is_explicit() {
        let mut app = TuiApp::from_view(empty_view());
        app.tab = Tab::Sensors;
        let text = render_size(&mut app, 80, 24);
        assert!(text.contains("No sensors detected"));
    }

    #[test]
    fn scroll_hint_is_visible_on_paragraph_views() {
        let mut app = TuiApp::from_view(empty_view());
        app.tab = Tab::System;
        let text = render_size(&mut app, 80, 24);
        assert!(text.contains("PgUp/Dn"));
        let text = render_size(&mut app, 100, 32);
        assert!(text.contains("PgUp/PgDn scroll"));
    }

    #[test]
    fn profile_confirmation_is_visible_in_the_footer() {
        let mut app = TuiApp::from_view(empty_view());
        app.tab = Tab::Profiles;
        app.pending_profile = Some(0);
        app.message = Some("Apply safe-auto (blocked for safety)? y = yes, n = no".to_string());
        let text = render_size(&mut app, 80, 24);
        assert!(text.contains("Apply safe-auto"));
    }

    #[test]
    fn action_message_is_visible_in_the_footer() {
        let mut app = TuiApp::from_view(empty_view());
        app.message = Some("NBFC automatic mode confirmed".to_string());
        let text = render_size(&mut app, 80, 24);
        assert!(text.contains("NBFC automatic mode confirmed"));
    }

    #[test]
    fn footer_truncation_keeps_short_messages() {
        assert_eq!(truncate_for_width("short", 10), "short");
        assert_eq!(truncate_for_width("abcdefgh", 6), "abc...");
        assert_eq!(truncate_for_width("abcdefgh", 2), "..");
    }

    #[test]
    fn compact_paragraph_views_advertise_scrolling() {
        let mut app = TuiApp::from_view(empty_view());
        app.tab = Tab::System;
        let text = render_size(&mut app, 40, 20);
        assert!(text.contains("j/k"));
    }

    #[test]
    fn fixed_presets_render_and_request_confirmation() {
        let mut app = TuiApp::from_view(empty_view());
        app.tab = Tab::Presets;
        let text = render_size(&mut app, 80, 24);
        assert!(text.contains("Silent"));
        assert!(text.contains("Turbo"));
        assert!(text.contains("25%"));
        app.handle_key(KeyCode::Enter);
        assert_eq!(app.pending_preset, Some(0));
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("fixed preset silent")
        );
        app.handle_key(KeyCode::Char('y'));
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("No supervisor socket")
        );
    }
    fn custom_status_value(name: &str) -> Value {
        serde_json::json!({
            "custom": {
                "schema": "neosense.custom_profiles_status/v1",
                "path": "/tmp/test-profiles.json",
                "profiles": [{
                    "name": name,
                    "title": "Custom test",
                    "description": "test",
                    "points": [
                        {"temperature_c": 40.0, "percent": 25.0},
                        {"temperature_c": 80.0, "percent": 80.0}
                    ],
                    "min_percent": 20.0,
                    "max_percent": 100.0,
                    "hysteresis_c": 3.0,
                    "max_step_percent": 10.0,
                    "stale_after_ms": 3000,
                    "fan_roles": ["cpu"]
                }],
                "warnings": [],
                "error": null,
                "recovered_from_backup": false
            }
        })
    }

    #[test]
    fn custom_profiles_are_inspected_in_the_profile_view() {
        let mut app = TuiApp::from_view(empty_view());
        app.supervisor.status = Some(custom_status_value("mine"));
        app.tab = Tab::Profiles;
        let text = render_size(&mut app, 100, 30);
        assert!(text.contains("mine"));
        assert!(text.contains("custom"));
        let evaluations = app.evaluations();
        let index = evaluations
            .iter()
            .position(|evaluation| evaluation.profile.name == "mine")
            .unwrap();
        app.profile_index = index;
        app.tab = Tab::System;
        let system = render_size(&mut app, 100, 30);
        assert!(system.contains("Custom profiles: 1"));
        app.tab = Tab::Profiles;
        app.handle_key(KeyCode::Char('e'));
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("through the supervisor")
        );
    }

    #[test]
    fn new_custom_curve_opens_a_valid_editor_template() {
        let mut app = TuiApp::from_view(empty_view());
        app.socket = Some(PathBuf::from("/nonexistent-neosense-test.sock"));
        app.supervisor.state = SupervisorState::Connected;
        app.begin_new_custom();
        let request = app.pending_editor.clone().expect("editor request");
        let profile: CustomProfile =
            serde_json::from_str(&std::fs::read_to_string(&request.path).unwrap()).unwrap();
        assert!(profile.validate().is_ok());
        assert_eq!(profile.name, "my-curve");
        assert!(matches!(request.action, EditorAction::Create));
        let _ = std::fs::remove_file(request.path);
    }

    #[test]
    fn edited_custom_curve_is_validated_and_sent_through_ipc() {
        use crate::ipc::{Request, Response, read_message, write_message};
        use std::io::BufReader;
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("tui-edit.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let request: Request = read_message(&mut reader).unwrap();
            assert_eq!(request.method, "custom.create");
            assert_eq!(request.params["profile"]["name"], "edited");
            let mut stream = stream;
            write_message(
                &mut stream,
                &Response::ok(request.id, serde_json::json!({"ok": true})),
            )
            .unwrap();
        });

        let path = dir.path().join("edited.json");
        let profile = CustomProfile {
            name: "edited".to_string(),
            title: "Edited".to_string(),
            description: String::new(),
            points: vec![
                crate::regulation::CurvePoint {
                    temperature_c: 40.0,
                    percent: 25.0,
                },
                crate::regulation::CurvePoint {
                    temperature_c: 80.0,
                    percent: 80.0,
                },
            ],
            min_percent: 20.0,
            max_percent: 100.0,
            hysteresis_c: 3.0,
            max_step_percent: 10.0,
            stale_after_ms: 3_000,
            fan_roles: vec![FanRole::Cpu],
        };
        std::fs::write(&path, serde_json::to_string(&profile).unwrap()).unwrap();

        let mut app = TuiApp::from_view(empty_view());
        app.socket = Some(socket);
        app.supervisor.state = SupervisorState::Connected;
        app.finish_editor(EditorRequest {
            path: path.clone(),
            action: EditorAction::Create,
        });
        server.join().unwrap();
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("created through the supervisor"),
            "{:?}",
            app.message
        );
        assert!(!path.exists());
    }

    #[test]
    fn invalid_edited_curve_is_not_sent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.json");
        std::fs::write(&path, b"{not json").unwrap();
        let mut app = TuiApp::from_view(empty_view());
        app.socket = Some(dir.path().join("unused.sock"));
        app.finish_editor(EditorRequest {
            path: path.clone(),
            action: EditorAction::Create,
        });
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("not valid JSON")
        );
        assert!(!path.exists());
    }

    #[test]
    fn help_overlay_renders() {
        let mut app = TuiApp::from_view(empty_view());
        app.show_help = true;
        let text = render_once(&mut app);
        assert!(text.contains("NeoSense — help"));
        assert!(text.contains("quit"));
    }
}
