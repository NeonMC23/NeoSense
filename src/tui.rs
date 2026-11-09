//! Read-only-first TUI for NeoSense.
//!
//! The TUI never writes fan state directly. The only control action currently
//! exposed is a confirmed `fan.auto` request through the supervisor IPC socket;
//! if the supervisor is unavailable, the interface stays read-only.

use crate::diagnostics::{CheckGroup, CheckStatus, DiagnosticReport, diagnose};
use crate::ipc::Client;
use crate::model::SensorSample;
use crate::nbfc::NbfcCli;
use crate::policy::fan_role;
use crate::profiles::{ProfileAction, ProfileContext, evaluate_all};
use crate::runtime::{RuntimeView, host_info, read_view};
use crate::supervisor::{SupervisorProbe, SupervisorState, probe as probe_supervisor};
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
    scroll: u16,
    should_quit: bool,
}

impl TuiApp {
    pub fn new(socket: Option<PathBuf>) -> Self {
        let view = read_view();
        let socket_path = socket
            .clone()
            .unwrap_or_else(crate::daemon::socket_path_from_env);
        let supervisor = probe_supervisor(&socket_path);
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
            supervisor: probe_supervisor(std::path::Path::new("/nonexistent-neosense-test.sock")),
            diagnostics: None,
            socket: None,
            last_refresh: Instant::now(),
            refresh_interval: Duration::from_secs(2),
            message: None,
            show_help: false,
            pending_auto: false,
            profile_index: 0,
            pending_profile: None,
            scroll: 0,
            should_quit: false,
        }
    }

    fn refresh(&mut self) {
        self.view = read_view();
        self.supervisor = probe_supervisor(
            self.socket
                .as_deref()
                .unwrap_or_else(|| std::path::Path::new("/nonexistent-neosense-test.sock")),
        );
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
                let count = evaluate_all(&ProfileContext::from_view(&self.view)).len();
                self.profile_index = (self.profile_index + 1).min(count.saturating_sub(1));
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
            KeyCode::Char('1') => self.select_tab(Tab::Dashboard),
            KeyCode::Char('2') => self.select_tab(Tab::Sensors),
            KeyCode::Char('3') => self.select_tab(Tab::Fans),
            KeyCode::Char('4') => self.select_tab(Tab::Profiles),
            KeyCode::Char('5') => self.select_tab(Tab::System),
            KeyCode::Char('6') => self.select_tab(Tab::Diagnostics),
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
                    "  ·  {}/{}  ·  Tab",
                    self.tab.index() + 1,
                    Tab::ALL.len()
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
                    format!("{:?}", sensor.health.trust),
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
            if compact {
                Row::new(vec![
                    evaluation.profile.name.to_string(),
                    evaluation.compatibility.label().to_string(),
                ])
                .style(style)
            } else {
                Row::new(vec![
                    evaluation.profile.name.to_string(),
                    evaluation.profile.title.to_string(),
                    evaluation.compatibility.label().to_string(),
                    evaluation.profile.reason.unwrap_or("").to_string(),
                ])
                .style(style)
            }
        });
        let (header, widths) = if compact {
            (
                Row::new(["Name", "Compatibility"]),
                vec![Constraint::Length(14), Constraint::Min(20)],
            )
        } else {
            (
                Row::new(["Name", "Title", "Compatibility", "Notes"]),
                vec![
                    Constraint::Length(12),
                    Constraint::Length(20),
                    Constraint::Length(24),
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
                    .title(if area.width < 70 {
                        " Profiles — Up/Down, Enter to apply "
                    } else {
                        " Thermal profiles — Up/Down select, Enter preview/apply "
                    }),
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
        if !self.pending_auto && self.pending_profile.is_none() && self.message.is_none() {
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
    fn help_overlay_renders() {
        let mut app = TuiApp::from_view(empty_view());
        app.show_help = true;
        let text = render_once(&mut app);
        assert!(text.contains("NeoSense — help"));
        assert!(text.contains("quit"));
    }
}
