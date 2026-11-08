//! Read-only-first TUI for NeoSense.
//!
//! The TUI never writes fan state directly. The only control action currently
//! exposed is a confirmed `fan.auto` request through the supervisor IPC socket;
//! if the supervisor is unavailable, the interface stays read-only.

use crate::diagnostics::{CheckStatus, DiagnosticReport, diagnose};
use crate::ipc::Client;
use crate::model::SensorSample;
use crate::nbfc::NbfcCli;
use crate::policy::fan_role;
use crate::profiles::{ProfileAction, ProfileContext, evaluate_all};
use crate::runtime::{RuntimeView, host_info, read_view};
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
}

impl Tab {
    const ALL: [Tab; 6] = [
        Tab::Dashboard,
        Tab::Sensors,
        Tab::Fans,
        Tab::Profiles,
        Tab::System,
        Tab::Diagnostics,
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
        }
    }

    fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    fn previous(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, Default)]
struct SupervisorView {
    connected: bool,
    health: Option<bool>,
    reason: Option<String>,
    error: Option<String>,
}

pub struct TuiApp {
    tab: Tab,
    view: RuntimeView,
    supervisor: SupervisorView,
    diagnostics: Option<DiagnosticReport>,
    socket: Option<PathBuf>,
    last_refresh: Instant,
    refresh_interval: Duration,
    message: Option<String>,
    show_help: bool,
    pending_auto: bool,
    profile_index: usize,
    pending_profile: Option<usize>,
    should_quit: bool,
}

impl TuiApp {
    pub fn new(socket: Option<PathBuf>) -> Self {
        let view = read_view();
        let supervisor = read_supervisor(socket.as_deref());
        let mut app = Self {
            tab: Tab::Dashboard,
            view,
            supervisor,
            diagnostics: None,
            socket,
            last_refresh: Instant::now(),
            refresh_interval: Duration::from_secs(2),
            message: None,
            show_help: false,
            pending_auto: false,
            profile_index: 0,
            pending_profile: None,
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
            supervisor: SupervisorView::default(),
            diagnostics: None,
            socket: None,
            last_refresh: Instant::now(),
            refresh_interval: Duration::from_secs(2),
            message: None,
            show_help: false,
            pending_auto: false,
            profile_index: 0,
            pending_profile: None,
            should_quit: false,
        }
    }

    fn refresh(&mut self) {
        self.view = read_view();
        self.supervisor = read_supervisor(self.socket.as_deref());
        if self.tab == Tab::Diagnostics {
            self.refresh_diagnostics();
        }
    }

    fn refresh_diagnostics(&mut self) {
        if self.tab == Tab::Diagnostics {
            let backend = NbfcCli::default();
            self.diagnostics = Some(diagnose(&backend, &self.view.snapshot));
        }
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

        match code {
            KeyCode::Esc | KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Tab | KeyCode::Right => {
                self.tab = self.tab.next();
                self.refresh_diagnostics();
            }
            KeyCode::BackTab | KeyCode::Left => {
                self.tab = self.tab.previous();
                self.refresh_diagnostics();
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
                self.profile_index = (self.profile_index + 1).min(4);
            }
            KeyCode::Enter if self.tab == Tab::Profiles => {
                let evaluations = evaluate_all(&ProfileContext::from_view(&self.view));
                if let Some(evaluation) = evaluations.get(self.profile_index) {
                    self.pending_profile = Some(self.profile_index);
                    self.message = Some(format!(
                        "Apply {} ({})? y = yes, n = no",
                        evaluation.profile.name,
                        evaluation.compatibility.label()
                    ));
                } else {
                    self.message = Some("Profile no longer available".to_string());
                }
            }
            KeyCode::Char('1') => self.tab = Tab::Dashboard,
            KeyCode::Char('2') => self.tab = Tab::Sensors,
            KeyCode::Char('3') => self.tab = Tab::Fans,
            KeyCode::Char('4') => self.tab = Tab::Profiles,
            KeyCode::Char('5') => self.tab = Tab::System,
            KeyCode::Char('6') => {
                self.tab = Tab::Diagnostics;
                self.refresh_diagnostics();
            }
            KeyCode::Char('a') => {
                if self.supervisor.connected {
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

    fn confirm_profile(&mut self, index: usize) {
        self.pending_profile = None;
        let evaluations = evaluate_all(&ProfileContext::from_view(&self.view));
        let Some(evaluation) = evaluations.get(index) else {
            self.message = Some("Profile no longer available".to_string());
            return;
        };
        if !evaluation.compatibility.can_apply()
            || evaluation.profile.action != ProfileAction::SafeAuto
        {
            let mut message = format!("Profile unavailable: {}", evaluation.compatibility.label());
            if !evaluation.reasons.is_empty() {
                message.push_str(" — ");
                message.push_str(&evaluation.reasons.join("; "));
            }
            self.message = Some(message);
            return;
        }
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
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(6),
            Constraint::Length(2),
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
        }
        self.render_footer(frame, footer);
        if self.show_help {
            self.render_help(frame, area);
        }
    }

    fn render_header(&self, frame: &mut ratatui::Frame, area: Rect) {
        let host = host_info();
        let host = format!(
            "{} {}",
            host.vendor.as_deref().unwrap_or("Machine"),
            host.model.as_deref().unwrap_or("unknown")
        );
        let nbfc = self
            .view
            .version
            .map(|v| format!("NBFC {v}"))
            .unwrap_or_else(|| "NBFC unavailable".to_string());
        let supervisor = if self.supervisor.connected {
            match self.supervisor.health {
                Some(true) => "supervisor healthy".to_string(),
                Some(false) => format!(
                    "supervisor degraded: {}",
                    self.supervisor
                        .reason
                        .as_deref()
                        .unwrap_or("unknown reason")
                ),
                None => "supervisor connected".to_string(),
            }
        } else {
            format!(
                "supervisor absent (read-only): {}",
                self.supervisor.error.as_deref().unwrap_or("no connection")
            )
        };
        let line = Line::from(vec![
            Span::styled(
                format!(" NeoSense {} ", crate::VERSION),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {host}  ·  {nbfc}  ·  {supervisor}")),
        ]);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));
        frame.render_widget(Paragraph::new(line).block(block), area);
    }

    fn render_tabs(&self, frame: &mut ratatui::Frame, area: Rect) {
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
        let [cpu_area, gpu_area] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(top);

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
        for warning in warnings_from_view(&self.view, self.supervisor.connected)
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
                    Some(false) => "manuel",
                    None => "unknown",
                };
                let target = fan
                    .target_speed
                    .map(|v| format!("{v:.1}%"))
                    .unwrap_or_else(|| "?".into());
                let current = fan
                    .current_speed
                    .map(|v| format!("{v:.1}%"))
                    .unwrap_or_else(|| "?".into());
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
                    .unwrap_or_else(|| "?".into());
                let implausible = matches!(
                    (fan.current_speed, fan.target_speed),
                    (Some(cur), Some(tgt)) if (cur - tgt).abs() > 20.0
                );
                lines.push(Line::from(format!(
                    "{:<10} {:<7} cible {:<6} EC {:<6}{} RPM {:<8} source {}",
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
            lines.push(Line::from("Status NBFC unavailable."));
        }
        if let Some(error) = &self.view.backend_error {
            lines.push(Line::from(Span::styled(
                format!("Erreur: {error}"),
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
        let rows = self.view.snapshot.sensors.iter().map(sensor_row);
        let header = Row::new(["Role", "Value", "Trust", "Status", "Source", "Path"]).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
        let table = Table::new(
            rows,
            [
                Constraint::Length(18),
                Constraint::Length(12),
                Constraint::Length(11),
                Constraint::Length(10),
                Constraint::Length(9),
                Constraint::Min(20),
            ],
        )
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Discovered sensors "),
        );
        frame.render_widget(table, area);
    }

    fn render_fans(&self, frame: &mut ratatui::Frame, area: Rect) {
        let mut rows = Vec::new();
        if let Some(status) = &self.view.status {
            for fan in &status.fans {
                let source = self
                    .view
                    .sensor_config
                    .as_ref()
                    .and_then(|config| config.fans.iter().find(|f| f.index == fan.index))
                    .map(|f| f.sensors.join(","))
                    .unwrap_or_else(|| "?".into());
                rows.push(Row::new(vec![
                    format!("#{}", fan.index),
                    fan.name.clone(),
                    fan_role(&fan.name).to_string(),
                    format!("{:?}", fan.auto_mode),
                    opt_percent(fan.requested_speed),
                    opt_percent(fan.target_speed),
                    opt_percent(fan.current_speed),
                    source,
                ]));
            }
        }
        let header = Row::new([
            "Index",
            "Name",
            "Role",
            "Mode",
            "Requested",
            "Target",
            "EC",
            "Source",
        ])
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
        let table = Table::new(
            rows,
            [
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
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Fans NBFC — EC n'est pas un RPM "),
        );
        frame.render_widget(table, area);
    }

    fn render_profiles(&self, frame: &mut ratatui::Frame, area: Rect) {
        let evaluations = evaluate_all(&ProfileContext::from_view(&self.view));
        let selected = self.profile_index.min(evaluations.len().saturating_sub(1));
        let rows = evaluations.iter().enumerate().map(|(index, evaluation)| {
            let style = if index == selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Row::new(vec![
                evaluation.profile.name.to_string(),
                evaluation.profile.title.to_string(),
                evaluation.compatibility.label().to_string(),
                evaluation.profile.reason.unwrap_or("").to_string(),
            ])
            .style(style)
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(12),
                Constraint::Length(20),
                Constraint::Length(24),
                Constraint::Min(20),
            ],
        )
        .header(
            Row::new(["Name", "Title", "Compatibility", "Notes"]).style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Thermal profiles — Up/Down select, Enter preview/apply confirmation "),
        );
        frame.render_widget(table, area);
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
        let supervisor = if self.supervisor.connected {
            match self.supervisor.health {
                Some(true) => "connected, healthy".to_string(),
                Some(false) => format!(
                    "connected, degraded: {}",
                    self.supervisor
                        .reason
                        .as_deref()
                        .unwrap_or("unknown reason")
                ),
                None => "connected, health unknown".to_string(),
            }
        } else {
            format!(
                "disconnected: {}",
                self.supervisor.error.as_deref().unwrap_or("no connection")
            )
        };
        lines.push(Line::from(format!("Supervisor: {supervisor}")));
        lines.push(Line::from(
            "Socket: configured supervisor socket; IPC is read-only when disconnected.",
        ));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Warnings",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        let warnings = warnings_from_view(&self.view, self.supervisor.connected);
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
                .wrap(Wrap { trim: true }),
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
        for check in &report.checks {
            let (label, color) = match check.status {
                CheckStatus::Ok => ("OK  ", Color::Green),
                CheckStatus::Info => ("INFO", Color::Cyan),
                CheckStatus::Warning => ("WARN", Color::Yellow),
                CheckStatus::Fail => ("FAIL", Color::Red),
            };
            lines.push(Line::from(vec![
                Span::styled(format!("[{label}] "), Style::default().fg(color)),
                Span::raw(format!("{:<28} {}", check.title, check.message)),
            ]));
        }
        lines.push(Line::from(format!("Result: {:?}", report.overall())));
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Diagnostics "),
                )
                .wrap(Wrap { trim: true }),
            area,
        );
    }

    fn render_footer(&self, frame: &mut ratatui::Frame, area: Rect) {
        let mut line = " q quit  ·  Tab/←→ views  ·  r refresh  ·  a auto  ·  ? help ".to_string();
        if self.pending_auto {
            line = " Return to NBFC automatic mode: y confirm  ·  n cancel ".to_string();
        }
        if !self.supervisor.connected {
            line.push_str("  ·  read-only ");
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
            Line::from("r               refresh now"),
            Line::from("a               return to NBFC automatic mode (confirmation)"),
            Line::from("? / Esc         close help"),
            Line::from(""),
            Line::from("EC speeds are not RPM. The TUI stays read-only without a supervisor."),
        ];
        frame.render_widget(
            Paragraph::new(text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Aide ")
                        .border_style(Style::default().fg(Color::Cyan)),
                )
                .wrap(Wrap { trim: true }),
            popup,
        );
    }
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
        .map(|v| format!("{v:.1}%"))
        .unwrap_or_else(|| "?".into())
}

fn read_supervisor(socket: Option<&std::path::Path>) -> SupervisorView {
    let Some(socket) = socket else {
        return SupervisorView {
            connected: false,
            error: Some("no socket configured".to_string()),
            ..SupervisorView::default()
        };
    };
    match Client::connect(socket) {
        Ok(mut client) => match client.call("status.get", serde_json::json!({})) {
            Ok(value) => {
                let health = value.get("health");
                SupervisorView {
                    connected: true,
                    health: health
                        .and_then(|h| h.get("healthy"))
                        .and_then(Value::as_bool),
                    reason: health
                        .and_then(|h| h.get("reason"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    error: None,
                }
            }
            Err(error) => SupervisorView {
                connected: false,
                error: Some(error.message),
                ..SupervisorView::default()
            },
        },
        Err(error) => SupervisorView {
            connected: false,
            error: Some(error.message),
            ..SupervisorView::default()
        },
    }
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
        assert!(app.message.as_deref().unwrap_or("").contains("unavailable"));
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
